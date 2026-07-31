//! Health Data Server (Apple Watch) compatible receiver.
//!
//! The HDS watch app pushes `PUT /` with a JSON body `{"data": "heartRate:72"}`
//! (`dataType:value`, one message per request) to `<pc-ip>:<port>` on the LAN.
//! This server is bound on 0.0.0.0 for the app's lifetime; the first heart
//! rate that arrives starts an HR session (= auto "connected"), and the
//! session ends by itself after 10s without data.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::mpsc;
use warp::Filter;

use crate::ble::emit_log;
use crate::session::HrSource;
use crate::AppState;

/// Feeder side of a live HDS session. Dropping `tx` (or raising `stop`) ends
/// the session's receive loop.
pub struct HdsSessionHandle {
    pub tx: mpsc::UnboundedSender<u16>,
    pub stop: Arc<AtomicBool>,
}

impl HdsSessionHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub async fn start_server(app: tauri::AppHandle, port: u16) {
    let app_filter = warp::any().map(move || app.clone());
    let put = warp::put()
        .and(warp::path::end())
        .and(warp::body::content_length_limit(64 * 1024))
        .and(warp::body::bytes())
        .and(app_filter)
        .map(|body: warp::hyper::body::Bytes, app: tauri::AppHandle| {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) {
                if let Some(data) = json.get("data").and_then(|d| d.as_str()) {
                    handle_data(&app, data);
                }
            }
            warp::reply()
        });

    warp::serve(put).run(([0, 0, 0, 0], port)).await;
}

/// Parse one watch message. Everything except heartRate is ignored on purpose.
fn handle_data(app: &tauri::AppHandle, data: &str) {
    let Some(value) = data.strip_prefix("heartRate:") else {
        return;
    };
    let Ok(bpm) = value.trim().parse::<f64>() else {
        return;
    };
    if !(1.0..=400.0).contains(&bpm) {
        return;
    }
    ingest_hr(app, bpm.round() as u16);
}

/// Feed one HR sample into the current HDS session, starting a session if
/// none is live. First-come-first-served: while a BLE session owns the
/// pipeline the sample is dropped (the watch still gets its 200).
fn ingest_hr(app: &tauri::AppHandle, bpm: u16) {
    let state = app.state::<AppState>();

    if !state.hds_enabled.load(Ordering::Relaxed) {
        return;
    }
    if *state.connected.lock().unwrap()
        && *state.active_source.lock().unwrap() != Some(HrSource::Hds)
    {
        return;
    }

    let mut session = state.hds_session.lock().unwrap();

    if let Some(handle) = session.as_ref() {
        if !handle.stop.load(Ordering::Relaxed) && handle.tx.send(bpm).is_ok() {
            return;
        }
    }

    // No live session — start one seeded with this sample.
    let (tx, rx) = mpsc::unbounded_channel();
    let _ = tx.send(bpm);
    let stop = Arc::new(AtomicBool::new(false));
    *session = Some(HdsSessionHandle {
        tx,
        stop: stop.clone(),
    });
    drop(session);

    emit_log(app, "HDS: Apple Watch connected", "info");

    let deps = crate::session_deps(&state);
    // Sparse by design: HealthKit delivers HR every ~5s and pauses for tens
    // of seconds on wrist-down, so the timeout is much longer than BLE's.
    let timeout_ms = *state.hds_timeout_secs.lock().unwrap() * 1000;
    let task_app = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::session::run_session(
            HrSource::Hds,
            rx,
            deps,
            task_app.clone(),
            stop.clone(),
            timeout_ms,
            None,
        )
        .await;
        let state = task_app.state::<AppState>();
        let mut session = state.hds_session.lock().unwrap();
        // Only clear our own handle — a newer session may already be in place.
        if session.as_ref().is_some_and(|h| Arc::ptr_eq(&h.stop, &stop)) {
            *session = None;
        }
    });
}
