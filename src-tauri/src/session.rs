use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Emitter;
use tokio::sync::mpsc;

use crate::ble::emit_log;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HrSource {
    Ble,
    Hds,
}

impl HrSource {
    pub fn as_str(self) -> &'static str {
        match self {
            HrSource::Ble => "ble",
            HrSource::Hds => "hds",
        }
    }

    fn label(self) -> &'static str {
        match self {
            HrSource::Ble => "BLE",
            HrSource::Hds => "HDS",
        }
    }
}

/// Shared state handles a session needs to drive the output pipeline
/// (UI events, OSC, overlay WS). Sources only feed HR values in; everything
/// downstream of the channel lives here.
pub struct SessionDeps {
    pub heart_rate: Arc<Mutex<u16>>,
    pub connected: Arc<Mutex<bool>>,
    pub active_source: Arc<Mutex<Option<HrSource>>>,
    pub osc_enabled: Arc<Mutex<bool>>,
    pub osc_port: Arc<Mutex<u16>>,
    pub osc_params: Arc<Mutex<crate::osc::OscParamNames>>,
    pub ws_broadcaster: Arc<crate::ws::WsBroadcaster>,
    pub ws_enabled: Arc<AtomicBool>,
    pub graph_interval_ms: Arc<Mutex<u64>>,
}

/// How often the receive loop wakes to check the stop flag while idle.
const STOP_POLL_MS: u64 = 200;

/// Run one HR session: mark connected, pump incoming HR values through the
/// output pipeline (UI event, OSC beat loop, overlay WS), and tear everything
/// down when the source stops for `no_signal_timeout_ms`, the channel closes,
/// or the stop flag is set. The timeout is per-source: BLE broadcasts every
/// second, but HDS delivery pauses for tens of seconds on wrist-down, so each
/// caller picks its own. `on_first_sample` fires once when the first HR
/// value actually arrives (used by BLE to remember the device).
pub async fn run_session(
    source: HrSource,
    mut rx: mpsc::UnboundedReceiver<u16>,
    deps: SessionDeps,
    app: tauri::AppHandle,
    stop_flag: Arc<AtomicBool>,
    no_signal_timeout_ms: u64,
    mut on_first_sample: Option<Box<dyn FnMut() + Send>>,
) {
    *deps.active_source.lock().unwrap() = Some(source);
    let _ = app.emit("source-changed", Some(source.as_str()));
    *deps.connected.lock().unwrap() = true;
    let _ = app.emit("connection-changed", true);

    let hr_sum: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let hr_count: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let hr_min: Arc<Mutex<u16>> = Arc::new(Mutex::new(u16::MAX));
    let hr_max: Arc<Mutex<u16>> = Arc::new(Mutex::new(0));

    // Beat loop: pulse is_hr_beat (ON 100ms → OFF) at HR-derived interval
    let beat_hr = deps.heart_rate.clone();
    let beat_osc_enabled = deps.osc_enabled.clone();
    let beat_osc_port = deps.osc_port.clone();
    let beat_osc_params = deps.osc_params.clone();
    let beat_stop = stop_flag.clone();
    let beat_app = app.clone();
    const BEAT_PULSE_MS: u64 = 100;
    let beat_task = tokio::spawn(async move {
        loop {
            if beat_stop.load(Ordering::Relaxed) {
                break;
            }
            let hr = *beat_hr.lock().unwrap();
            if hr > 0 && *beat_osc_enabled.lock().unwrap() {
                let cycle_ms = (60_000u64).checked_div(hr as u64).unwrap_or(750);
                let port = *beat_osc_port.lock().unwrap();
                let params = beat_osc_params.lock().unwrap().clone();

                // Beat ON
                let state_on = crate::osc::HrState {
                    hr, is_connected: true, is_active: true, beat_toggle: true,
                };
                if let Err(e) = crate::osc::send_hr_params(port, &params, &state_on) {
                    emit_log(&beat_app, &format!("OSC send error: {e}"), "error");
                }

                // Hold ON for pulse duration
                tokio::time::sleep(std::time::Duration::from_millis(BEAT_PULSE_MS)).await;

                // Beat OFF
                let state_off = crate::osc::HrState {
                    hr, is_connected: true, is_active: true, beat_toggle: false,
                };
                let _ = crate::osc::send_hr_params(port, &params, &state_off);

                // Wait remaining interval
                let remaining = cycle_ms.saturating_sub(BEAT_PULSE_MS);
                tokio::time::sleep(std::time::Duration::from_millis(remaining)).await;
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    });

    // WS broadcast loop: sends overlay data at configurable interval
    let ws_hr = deps.heart_rate.clone();
    let ws_sum = hr_sum.clone();
    let ws_count = hr_count.clone();
    let ws_min = hr_min.clone();
    let ws_max = hr_max.clone();
    let ws_stop = stop_flag.clone();
    let ws_interval = deps.graph_interval_ms.clone();
    let ws_enabled = deps.ws_enabled.clone();
    let ws_broadcaster = deps.ws_broadcaster.clone();
    let ws_task = tokio::spawn(async move {
        loop {
            if ws_stop.load(Ordering::Relaxed) {
                break;
            }
            if ws_enabled.load(Ordering::Relaxed) {
                let hr = *ws_hr.lock().unwrap();
                if hr > 0 {
                    let count = *ws_count.lock().unwrap();
                    let avg = if count > 0 {
                        (*ws_sum.lock().unwrap() / count) as u16
                    } else {
                        hr
                    };
                    let mn = *ws_min.lock().unwrap();
                    let mx = *ws_max.lock().unwrap();
                    let zone = if hr >= 140 {
                        "hard"
                    } else if hr >= 120 {
                        "moderate"
                    } else if hr >= 100 {
                        "light"
                    } else {
                        "rest"
                    };
                    let json = format!(
                        r#"{{"type":"hr_update","bpm":{hr},"zone":"{zone}","connected":true,"avg":{avg},"min":{mn},"max":{mx}}}"#
                    );
                    ws_broadcaster.send(&json);
                }
            }
            let interval = *ws_interval.lock().unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(interval)).await;
        }
    });

    // Receive loop: update shared state + emit UI event.
    // Short poll so a raised stop flag is honored quickly; a session ends on
    // its own only after no_signal_timeout_ms without a sample.
    let mut first_sample = true;
    let mut last_sample = std::time::Instant::now();
    loop {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }
        match tokio::time::timeout(
            std::time::Duration::from_millis(STOP_POLL_MS),
            rx.recv(),
        )
        .await
        {
            Ok(Some(hr)) => {
                last_sample = std::time::Instant::now();
                if first_sample {
                    first_sample = false;
                    if let Some(cb) = on_first_sample.as_mut() {
                        cb();
                    }
                }
                *hr_sum.lock().unwrap() += hr as u64;
                *hr_count.lock().unwrap() += 1;
                if hr < *hr_min.lock().unwrap() {
                    *hr_min.lock().unwrap() = hr;
                }
                if hr > *hr_max.lock().unwrap() {
                    *hr_max.lock().unwrap() = hr;
                }

                *deps.heart_rate.lock().unwrap() = hr;
                let _ = app.emit("heart-rate-update", hr);
            }
            Ok(None) => break,
            Err(_) => {
                if last_sample.elapsed().as_millis() as u64 >= no_signal_timeout_ms {
                    emit_log(
                        &app,
                        &format!(
                            "No {} signal for {}s — source lost",
                            source.label(),
                            no_signal_timeout_ms / 1000
                        ),
                        "warn",
                    );
                    break;
                }
            }
        }
    }

    beat_task.abort();
    ws_task.abort();

    // Another session may have taken over the shared state while we were
    // shutting down (e.g. manual BLE connect while an HDS session is live).
    // Only the current owner may clear the pipeline and announce disconnect.
    let still_owner = {
        let mut active = deps.active_source.lock().unwrap();
        if *active == Some(source) {
            *active = None;
            true
        } else {
            false
        }
    };

    if still_owner {
        if *deps.osc_enabled.lock().unwrap() {
            let port = *deps.osc_port.lock().unwrap();
            let params = deps.osc_params.lock().unwrap().clone();
            let _ = crate::osc::send_hr_params(
                port,
                &params,
                &crate::osc::HrState {
                    hr: 0,
                    is_connected: false,
                    is_active: false,
                    beat_toggle: false,
                },
            );
        }
        *deps.heart_rate.lock().unwrap() = 0;
        emit_log(&app, "Connection ended", "info");
        *deps.connected.lock().unwrap() = false;
        let _ = app.emit("connection-changed", false);
        let _ = app.emit("source-changed", None::<&str>);
    }
}
