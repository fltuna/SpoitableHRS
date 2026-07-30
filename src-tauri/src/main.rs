#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ble;
mod config;
mod hds;
mod osc;
mod recorder;
mod session;
mod ws;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::State;

pub struct AppState {
    pub heart_rate: Arc<Mutex<u16>>,
    pub connected: Arc<Mutex<bool>>,
    /// Which source currently owns the output pipeline (BLE or HDS).
    pub active_source: Arc<Mutex<Option<session::HrSource>>>,
    pub osc_enabled: Arc<Mutex<bool>>,
    pub osc_port: Arc<Mutex<u16>>,
    pub osc_params: Arc<Mutex<osc::OscParamNames>>,
    pub ble_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    pub stop_flag: Arc<AtomicBool>,
    pub ws_broadcaster: Arc<ws::WsBroadcaster>,
    pub ws_enabled: Arc<AtomicBool>,
    pub ws_port: Arc<Mutex<u16>>,
    pub always_on_top: Arc<AtomicBool>,
    pub start_minimized: Arc<AtomicBool>,
    pub language: Arc<Mutex<String>>,
    pub graph_interval_ms: Arc<Mutex<u64>>,
    pub recording_enabled: Arc<AtomicBool>,
    pub record_interval_ms: Arc<Mutex<u64>>,
    pub flush_interval_ms: Arc<Mutex<u64>>,
    pub recorder: Arc<recorder::Recorder>,
    pub auto_reconnect_enabled: Arc<AtomicBool>,
    pub auto_reconnect_interval_secs: Arc<Mutex<u64>>,
    /// Set on manual disconnect so auto-reconnect doesn't immediately undo it.
    /// Cleared on manual connect, app start, or re-enabling auto-reconnect.
    pub auto_reconnect_suspended: Arc<AtomicBool>,
    pub remembered_devices: Arc<Mutex<Vec<config::RememberedDevice>>>,
    pub hds_enabled: Arc<AtomicBool>,
    pub hds_port: Arc<Mutex<u16>>,
    pub hds_session: Arc<Mutex<Option<hds::HdsSessionHandle>>>,
}

/// Snapshot the shared handles an HR session needs. Used by both the BLE
/// connect path and the HDS receiver.
pub fn session_deps(state: &AppState) -> session::SessionDeps {
    session::SessionDeps {
        heart_rate: state.heart_rate.clone(),
        connected: state.connected.clone(),
        active_source: state.active_source.clone(),
        osc_enabled: state.osc_enabled.clone(),
        osc_port: state.osc_port.clone(),
        osc_params: state.osc_params.clone(),
        ws_broadcaster: state.ws_broadcaster.clone(),
        ws_enabled: state.ws_enabled.clone(),
        graph_interval_ms: state.graph_interval_ms.clone(),
    }
}

fn save_config(state: &AppState) {
    let cfg = config::AppConfig {
        osc_enabled: *state.osc_enabled.lock().unwrap(),
        osc_port: *state.osc_port.lock().unwrap(),
        osc_params: state.osc_params.lock().unwrap().clone(),
        ws_enabled: state.ws_enabled.load(Ordering::Relaxed),
        ws_port: *state.ws_port.lock().unwrap(),
        always_on_top: state.always_on_top.load(Ordering::Relaxed),
        start_minimized: state.start_minimized.load(Ordering::Relaxed),
        language: state.language.lock().unwrap().clone(),
        graph_interval_ms: *state.graph_interval_ms.lock().unwrap(),
        recording_enabled: state.recording_enabled.load(Ordering::Relaxed),
        record_interval_ms: *state.record_interval_ms.lock().unwrap(),
        flush_interval_ms: *state.flush_interval_ms.lock().unwrap(),
        auto_reconnect_enabled: state.auto_reconnect_enabled.load(Ordering::Relaxed),
        auto_reconnect_interval_secs: *state.auto_reconnect_interval_secs.lock().unwrap(),
        remembered_devices: state.remembered_devices.lock().unwrap().clone(),
        hds_enabled: state.hds_enabled.load(Ordering::Relaxed),
        hds_port: *state.hds_port.lock().unwrap(),
    };
    config::save(&cfg);
}

/// Upsert a device into the remembered list and persist. Called from the BLE
/// receive loop when the first HR packet arrives (= connection actually works).
pub fn remember_device(app: &tauri::AppHandle, id: &str, name: &str) {
    use tauri::{Emitter, Manager};
    let state = app.state::<AppState>();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    {
        let mut devs = state.remembered_devices.lock().unwrap();
        if let Some(d) = devs.iter_mut().find(|d| d.id == id) {
            if !name.is_empty() {
                d.name = name.to_string();
            }
            d.last_connected = now;
        } else {
            devs.push(config::RememberedDevice {
                id: id.to_string(),
                name: name.to_string(),
                last_connected: now,
            });
        }
    }
    save_config(&state);
    let _ = app.emit("remembered-devices-changed", ());
}

#[tauri::command]
async fn scan_devices() -> Result<Vec<ble::DeviceInfo>, String> {
    ble::scan().await.map_err(|e| e.to_string())
}

/// Stop any existing BLE session and spawn a new one for the given device.
/// Shared by the manual connect command and the auto-reconnect loop.
fn start_connection(app: &tauri::AppHandle, device_id: String, device_name: String) {
    use tauri::Manager;
    let state = app.state::<AppState>();

    // Manual BLE connect is an explicit user action — it takes over from a
    // live HDS (Apple Watch) session.
    if let Some(handle) = state.hds_session.lock().unwrap().take() {
        handle.stop();
    }

    if let Some(handle) = state.ble_handle.lock().unwrap().take() {
        state.stop_flag.store(true, Ordering::Relaxed);
        handle.abort();
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    state.stop_flag.store(false, Ordering::Relaxed);

    let deps = session_deps(&state);
    let stop = state.stop_flag.clone();
    let task_app = app.clone();

    let handle = tokio::spawn(async move {
        if let Err(e) =
            ble::connect_and_subscribe(&device_id, &device_name, deps, task_app.clone(), stop)
                .await
        {
            ble::emit_log(&task_app, &format!("BLE error: {e}"), "error");
        }
    });

    *state.ble_handle.lock().unwrap() = Some(handle);
}

#[tauri::command]
async fn connect_device(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    device_id: String,
    device_name: Option<String>,
) -> Result<(), String> {
    state
        .auto_reconnect_suspended
        .store(false, Ordering::Relaxed);
    start_connection(&app, device_id, device_name.unwrap_or_default());
    Ok(())
}

#[tauri::command]
async fn disconnect_device(state: State<'_, AppState>) -> Result<(), String> {
    state.auto_reconnect_suspended.store(true, Ordering::Relaxed);
    state.stop_flag.store(true, Ordering::Relaxed);
    if let Some(handle) = state.ble_handle.lock().unwrap().take() {
        handle.abort();
    }
    if let Some(handle) = state.hds_session.lock().unwrap().take() {
        handle.stop();
    }
    *state.active_source.lock().unwrap() = None;
    *state.connected.lock().unwrap() = false;
    *state.heart_rate.lock().unwrap() = 0;

    if *state.osc_enabled.lock().unwrap() {
        let port = *state.osc_port.lock().unwrap();
        let params = state.osc_params.lock().unwrap().clone();
        let hr_state = osc::HrState {
            hr: 0,
            is_connected: false,
            is_active: false,
            beat_toggle: false,
        };
        let _ = osc::send_hr_params(port, &params, &hr_state);
    }

    Ok(())
}

#[tauri::command]
fn get_heart_rate(state: State<'_, AppState>) -> u16 {
    *state.heart_rate.lock().unwrap()
}

#[tauri::command]
fn is_connected(state: State<'_, AppState>) -> bool {
    *state.connected.lock().unwrap()
}

#[tauri::command]
fn set_osc_enabled(state: State<'_, AppState>, enabled: bool) {
    *state.osc_enabled.lock().unwrap() = enabled;
    save_config(&state);
}

#[tauri::command]
fn set_osc_port(state: State<'_, AppState>, port: u16) {
    *state.osc_port.lock().unwrap() = port;
    save_config(&state);
}

#[tauri::command]
fn get_osc_port(state: State<'_, AppState>) -> u16 {
    *state.osc_port.lock().unwrap()
}

#[tauri::command]
fn get_osc_enabled(state: State<'_, AppState>) -> bool {
    *state.osc_enabled.lock().unwrap()
}

#[tauri::command]
fn get_osc_params(state: State<'_, AppState>) -> osc::OscParamNames {
    state.osc_params.lock().unwrap().clone()
}

#[tauri::command]
fn set_osc_params(state: State<'_, AppState>, params: osc::OscParamNames) {
    *state.osc_params.lock().unwrap() = params;
    save_config(&state);
}

#[tauri::command]
fn set_ws_enabled(state: State<'_, AppState>, enabled: bool) {
    state.ws_enabled.store(enabled, Ordering::Relaxed);
    save_config(&state);
}

#[tauri::command]
fn get_ws_enabled(state: State<'_, AppState>) -> bool {
    state.ws_enabled.load(Ordering::Relaxed)
}

#[tauri::command]
fn set_ws_port(state: State<'_, AppState>, port: u16) {
    *state.ws_port.lock().unwrap() = port;
    save_config(&state);
}

#[tauri::command]
fn get_ws_port(state: State<'_, AppState>) -> u16 {
    *state.ws_port.lock().unwrap()
}

#[tauri::command]
fn set_always_on_top(state: State<'_, AppState>, enabled: bool) {
    state.always_on_top.store(enabled, Ordering::Relaxed);
    save_config(&state);
}

#[tauri::command]
fn get_always_on_top(state: State<'_, AppState>) -> bool {
    state.always_on_top.load(Ordering::Relaxed)
}

#[tauri::command]
fn set_start_minimized(state: State<'_, AppState>, enabled: bool) {
    state.start_minimized.store(enabled, Ordering::Relaxed);
    save_config(&state);
}

#[tauri::command]
fn get_start_minimized(state: State<'_, AppState>) -> bool {
    state.start_minimized.load(Ordering::Relaxed)
}

#[tauri::command]
fn set_language(state: State<'_, AppState>, language: String) {
    *state.language.lock().unwrap() = language;
    save_config(&state);
}

#[tauri::command]
fn get_language(state: State<'_, AppState>) -> String {
    state.language.lock().unwrap().clone()
}

#[tauri::command]
fn set_graph_interval(state: State<'_, AppState>, interval: u64) {
    let clamped = interval.clamp(100, 5000);
    *state.graph_interval_ms.lock().unwrap() = clamped;
    save_config(&state);
}

#[tauri::command]
fn get_graph_interval(state: State<'_, AppState>) -> u64 {
    *state.graph_interval_ms.lock().unwrap()
}

#[tauri::command]
fn set_recording_enabled(state: State<'_, AppState>, enabled: bool) {
    state.recording_enabled.store(enabled, Ordering::Relaxed);
    save_config(&state);
}

#[tauri::command]
fn get_recording_enabled(state: State<'_, AppState>) -> bool {
    state.recording_enabled.load(Ordering::Relaxed)
}

#[tauri::command]
fn set_record_interval(state: State<'_, AppState>, interval: u64) {
    let clamped = interval.clamp(250, 60_000);
    *state.record_interval_ms.lock().unwrap() = clamped;
    save_config(&state);
}

#[tauri::command]
fn get_record_interval(state: State<'_, AppState>) -> u64 {
    *state.record_interval_ms.lock().unwrap()
}

#[tauri::command]
fn set_flush_interval(state: State<'_, AppState>, interval: u64) {
    let clamped = interval.clamp(500, 300_000);
    *state.flush_interval_ms.lock().unwrap() = clamped;
    save_config(&state);
}

#[tauri::command]
fn get_flush_interval(state: State<'_, AppState>) -> u64 {
    *state.flush_interval_ms.lock().unwrap()
}

#[tauri::command]
fn set_auto_reconnect_enabled(state: State<'_, AppState>, enabled: bool) {
    state
        .auto_reconnect_enabled
        .store(enabled, Ordering::Relaxed);
    if enabled {
        // Turning it on is an explicit "reconnect for me" — lift any suspension.
        state
            .auto_reconnect_suspended
            .store(false, Ordering::Relaxed);
    }
    save_config(&state);
}

#[tauri::command]
fn get_auto_reconnect_enabled(state: State<'_, AppState>) -> bool {
    state.auto_reconnect_enabled.load(Ordering::Relaxed)
}

#[tauri::command]
fn set_auto_reconnect_interval(state: State<'_, AppState>, interval: u64) {
    let clamped = interval.clamp(1, 10);
    *state.auto_reconnect_interval_secs.lock().unwrap() = clamped;
    save_config(&state);
}

#[tauri::command]
fn get_auto_reconnect_interval(state: State<'_, AppState>) -> u64 {
    *state.auto_reconnect_interval_secs.lock().unwrap()
}

#[tauri::command]
fn set_hds_enabled(state: State<'_, AppState>, enabled: bool) {
    state.hds_enabled.store(enabled, Ordering::Relaxed);
    if !enabled {
        // Kill a live watch session right away instead of waiting for timeout.
        if let Some(handle) = state.hds_session.lock().unwrap().take() {
            handle.stop();
        }
    }
    save_config(&state);
}

#[tauri::command]
fn get_hds_enabled(state: State<'_, AppState>) -> bool {
    state.hds_enabled.load(Ordering::Relaxed)
}

/// Saved immediately, but the server socket is bound at startup — the new
/// port takes effect after an app restart (same as the overlay WS port).
#[tauri::command]
fn set_hds_port(state: State<'_, AppState>, port: u16) {
    *state.hds_port.lock().unwrap() = port;
    save_config(&state);
}

#[tauri::command]
fn get_hds_port(state: State<'_, AppState>) -> u16 {
    *state.hds_port.lock().unwrap()
}

/// Primary LAN IPv4 of this machine — what the user types into the watch app.
/// UDP connect doesn't send packets; it just resolves the outbound interface.
#[tauri::command]
fn get_lan_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip().to_string())
}

#[tauri::command]
fn get_remembered_devices(state: State<'_, AppState>) -> Vec<config::RememberedDevice> {
    state.remembered_devices.lock().unwrap().clone()
}

#[tauri::command]
fn remove_remembered_device(state: State<'_, AppState>, id: String) {
    state
        .remembered_devices
        .lock()
        .unwrap()
        .retain(|d| d.id != id);
    save_config(&state);
}

/// Background loop: while disconnected (and not suspended), keep a passive
/// advertisement watcher running for remembered devices and connect as soon as
/// one shows up. After triggering a connection attempt, wait the configured
/// interval before watching again so failed attempts don't spin.
fn spawn_auto_reconnect(app: tauri::AppHandle) {
    use tauri::{Emitter, Manager};
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;

            let (enabled, suspended, connected, ids, interval) = {
                let state = app.state::<AppState>();
                (
                    state.auto_reconnect_enabled.clone(),
                    state.auto_reconnect_suspended.clone(),
                    state.connected.clone(),
                    state
                        .remembered_devices
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|d| d.id.clone())
                        .collect::<Vec<_>>(),
                    *state.auto_reconnect_interval_secs.lock().unwrap(),
                )
            };

            if !enabled.load(Ordering::Relaxed)
                || suspended.load(Ordering::Relaxed)
                || *connected.lock().unwrap()
                || ids.is_empty()
            {
                continue;
            }

            let Some(found_id) = ble::watch_for_devices(ids, enabled, suspended, connected).await
            else {
                continue;
            };

            let name = {
                let state = app.state::<AppState>();
                let devs = state.remembered_devices.lock().unwrap();
                devs.iter()
                    .find(|d| d.id == found_id)
                    .map(|d| d.name.clone())
                    .unwrap_or_default()
            };

            ble::emit_log(
                &app,
                &format!("Auto-reconnect: {name} ({found_id}) detected, connecting..."),
                "info",
            );
            let _ = app.emit(
                "auto-connect",
                ble::DeviceInfo {
                    id: found_id.clone(),
                    name: name.clone(),
                },
            );
            start_connection(&app, found_id, name);

            tokio::time::sleep(std::time::Duration::from_secs(interval.max(1))).await;
        }
    });
}

#[tauri::command]
fn read_hr_records(from: i64, to: i64) -> Vec<recorder::HrPoint> {
    recorder::read_range(from, to)
}

#[tauri::command]
fn open_records_dir() -> Result<(), String> {
    let dir = recorder::records_root();
    std::process::Command::new("explorer")
        .arg(dir)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn set_stats_expanded(app: tauri::AppHandle, expanded: bool) -> Result<(), String> {
    use tauri::Manager;
    if let Some(win) = app.get_webview_window("main") {
        let (w, h) = if expanded { (1060.0, 720.0) } else { (450.0, 720.0) };
        win.set_size(tauri::LogicalSize::new(w, h))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    std::process::Command::new("cmd")
        .args(["/c", "start", "", &url])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn debug_updater(app: tauri::AppHandle) -> Result<String, String> {
    use tauri_plugin_updater::UpdaterExt;
    let version = app.config().version.clone().unwrap_or_default();
    let target = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let pkg_version = app.package_info().version.to_string();
    let mut log = format!("app version: {version}\n");
    log.push_str(&format!("package_info version: {pkg_version}\n"));
    log.push_str(&format!("tauri target: {target}\n"));

    let url = format!("https://spoitable.update.f2a.dev/update/{target}/{version}");
    log.push_str(&format!("endpoint: {url}\n"));

    // Manual reqwest with same URL
    match reqwest::get(&url).await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            log.push_str(&format!("manual fetch: HTTP {status}\n"));
            log.push_str(&format!("response body: {}\n", &body[..body.len().min(200)]));

            // Manual version comparison
            if status == 200 {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                    if let Some(remote_ver) = json["version"].as_str() {
                        match (semver::Version::parse(remote_ver), semver::Version::parse(&version)) {
                            (Ok(remote), Ok(current)) => {
                                log.push_str(&format!("semver: remote={remote} current={current} newer={}\n", remote > current));
                            }
                            (Err(e1), _) => log.push_str(&format!("semver parse remote failed: {e1}\n")),
                            (_, Err(e2)) => log.push_str(&format!("semver parse current failed: {e2}\n")),
                        }
                    }
                }
            }
        }
        Err(e) => {
            log.push_str(&format!("manual fetch error: {e}\n"));
        }
    }

    match app.updater() {
        Ok(updater) => {
            log.push_str("updater created OK\n");
            match updater.check().await {
                Ok(Some(update)) => {
                    log.push_str(&format!(
                        "update found: version={} date={:?} body={:?}\n",
                        update.version, update.date, update.body
                    ));
                }
                Ok(None) => {
                    log.push_str("updater returned None (no update)\n");
                }
                Err(e) => {
                    log.push_str(&format!("updater check error: {e}\n"));
                }
            }
        }
        Err(e) => {
            log.push_str(&format!("updater creation error: {e}\n"));
        }
    }
    Ok(log)
}

#[tauri::command]
async fn download_and_install_update(
    app: tauri::AppHandle,
    url: String,
    signature: String,
) -> Result<(), String> {
    let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Download failed: HTTP {}", resp.status().as_u16()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;

    let pubkey_b64 = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("pubkey"))
        .and_then(|v| v.as_str())
        .ok_or("No updater pubkey in config")?
        .to_string();

    let pubkey_decoded = String::from_utf8(
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &pubkey_b64)
            .map_err(|e| format!("pubkey decode: {e}"))?,
    )
    .map_err(|e| format!("pubkey utf8: {e}"))?;

    let pk = minisign_verify::PublicKey::from_base64(
        pubkey_decoded
            .lines()
            .find(|l| !l.starts_with("untrusted comment:"))
            .ok_or("Invalid pubkey format")?,
    )
    .map_err(|e| format!("pubkey parse: {e}"))?;

    let sig_decoded = String::from_utf8(
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &signature)
            .map_err(|e| format!("sig base64 decode: {e}"))?,
    )
    .map_err(|e| format!("sig utf8: {e}"))?;

    let sig = minisign_verify::Signature::decode(&sig_decoded)
        .map_err(|e| format!("sig decode: {e}"))?;

    pk.verify(&bytes, &sig, false)
        .map_err(|e| format!("Signature verification failed: {e}"))?;

    let temp_dir = std::env::temp_dir();
    let installer_path = temp_dir.join("SpoitableHRS-update-setup.exe");
    std::fs::write(&installer_path, &bytes).map_err(|e| e.to_string())?;

    {
        use tauri::Manager;
        let state = app.state::<AppState>();
        if *state.osc_enabled.lock().unwrap() {
            let port = *state.osc_port.lock().unwrap();
            let params = state.osc_params.lock().unwrap().clone();
            let _ = osc::send_hr_params(port, &params, &osc::HrState {
                hr: 0, is_connected: false, is_active: false, beat_toggle: false,
            });
        }
        save_config(&state);
    }

    let app_exe = dirs::data_local_dir()
        .unwrap()
        .join("SpoitableHRS\\spoitable-hrs.exe");

    let bat_path = temp_dir.join("spoitable-update.cmd");
    std::fs::write(
        &bat_path,
        format!(
            "@echo off\r\n\
             timeout /t 1 /nobreak >nul\r\n\
             taskkill /f /im spoitable-hrs.exe >nul 2>&1\r\n\
             timeout /t 1 /nobreak >nul\r\n\
             \"{installer}\" /S\r\n\
             timeout /t 3 /nobreak >nul\r\n\
             start \"\" \"{app}\"\r\n\
             del \"%~f0\"\r\n",
            installer = installer_path.display(),
            app = app_exe.display(),
        ),
    )
    .map_err(|e| e.to_string())?;

    use std::os::windows::process::CommandExt;
    std::process::Command::new("cmd")
        .args(["/c", &bat_path.to_string_lossy().to_string()])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .spawn()
        .map_err(|e| e.to_string())?;

    std::process::exit(0);
}

#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<Option<serde_json::Value>, String> {
    let version = app.config().version.clone().unwrap_or_default();
    let url = format!(
        "https://spoitable.update.f2a.dev/update/windows-x86_64/{version}"
    );
    let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if resp.status().as_u16() == 204 {
        return Ok(None);
    }
    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(Some(data))
}

fn main() {
    let cfg = config::load();

    let ws_broadcaster = Arc::new(ws::WsBroadcaster::new());
    let ws_port = cfg.ws_port;

    let bc = ws_broadcaster.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(ws::start_server(ws_port, bc));
    });

    // Shared state used by both the Tauri commands and the recorder thread.
    let heart_rate = Arc::new(Mutex::new(0u16));
    let connected = Arc::new(Mutex::new(false));
    let recording_enabled = Arc::new(AtomicBool::new(cfg.recording_enabled));
    let record_interval_ms = Arc::new(Mutex::new(cfg.record_interval_ms));
    let flush_interval_ms = Arc::new(Mutex::new(cfg.flush_interval_ms));
    let recorder = Arc::new(recorder::Recorder::new());

    // CSV recorder: sample HR into a queue at the record interval, flush the
    // queue to hourly CSV files at the flush interval. Runs for the app's
    // lifetime; remaining rows are flushed synchronously on RunEvent::Exit.
    {
        let s_hr = heart_rate.clone();
        let s_conn = connected.clone();
        let s_enabled = recording_enabled.clone();
        let s_record_interval = record_interval_ms.clone();
        let s_flush_interval = flush_interval_ms.clone();
        let s_recorder = recorder.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let sampler = {
                    let hr = s_hr.clone();
                    let conn = s_conn.clone();
                    let enabled = s_enabled.clone();
                    let interval = s_record_interval.clone();
                    let rec = s_recorder.clone();
                    tokio::spawn(async move {
                        loop {
                            let ms = *interval.lock().unwrap();
                            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                            if !enabled.load(Ordering::Relaxed) || !*conn.lock().unwrap() {
                                continue;
                            }
                            let bpm = *hr.lock().unwrap();
                            if bpm > 0 {
                                rec.record(bpm);
                            }
                        }
                    })
                };
                let flusher = {
                    let interval = s_flush_interval.clone();
                    let rec = s_recorder.clone();
                    tokio::spawn(async move {
                        loop {
                            let ms = *interval.lock().unwrap();
                            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                            rec.flush();
                        }
                    })
                };
                let _ = tokio::join!(sampler, flusher);
            });
        });
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            use tauri::Manager;
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }))
        .setup(|app| {
            use tauri::Manager;
            use tauri::menu::{MenuBuilder, MenuItemBuilder};
            use tauri::tray::TrayIconBuilder;

            let quit = MenuItemBuilder::with_id("quit", "Exit").build(app)?;
            let menu = MenuBuilder::new(app).item(&quit).build()?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().unwrap())
                .tooltip("SpoitableHRS")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id() == "quit" {
                        app.exit(0);
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left, ..
                    } = event {
                        let app = tray.app_handle();
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.unminimize();
                            let _ = win.set_focus();
                        }
                    }
                })
                .build(app)?;

            spawn_auto_reconnect(app.handle().clone());

            // HDS (Apple Watch) receiver — always listening; sessions start
            // themselves when data arrives and hds_enabled is on.
            let hds_port = *app.state::<AppState>().hds_port.lock().unwrap();
            let hds_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                hds::start_server(hds_app, hds_port).await;
            });

            Ok(())
        })
        .manage(AppState {
            heart_rate,
            connected,
            active_source: Arc::new(Mutex::new(None)),
            osc_enabled: Arc::new(Mutex::new(cfg.osc_enabled)),
            osc_port: Arc::new(Mutex::new(cfg.osc_port)),
            osc_params: Arc::new(Mutex::new(cfg.osc_params)),
            ble_handle: Arc::new(Mutex::new(None)),
            stop_flag: Arc::new(AtomicBool::new(false)),
            ws_broadcaster,
            ws_enabled: Arc::new(AtomicBool::new(cfg.ws_enabled)),
            ws_port: Arc::new(Mutex::new(cfg.ws_port)),
            always_on_top: Arc::new(AtomicBool::new(cfg.always_on_top)),
            start_minimized: Arc::new(AtomicBool::new(cfg.start_minimized)),
            language: Arc::new(Mutex::new(cfg.language)),
            graph_interval_ms: Arc::new(Mutex::new(cfg.graph_interval_ms)),
            recording_enabled,
            record_interval_ms,
            flush_interval_ms,
            recorder,
            auto_reconnect_enabled: Arc::new(AtomicBool::new(cfg.auto_reconnect_enabled)),
            auto_reconnect_interval_secs: Arc::new(Mutex::new(cfg.auto_reconnect_interval_secs)),
            auto_reconnect_suspended: Arc::new(AtomicBool::new(false)),
            remembered_devices: Arc::new(Mutex::new(cfg.remembered_devices)),
            hds_enabled: Arc::new(AtomicBool::new(cfg.hds_enabled)),
            hds_port: Arc::new(Mutex::new(cfg.hds_port)),
            hds_session: Arc::new(Mutex::new(None)),
        })
        .invoke_handler(tauri::generate_handler![
            scan_devices,
            connect_device,
            disconnect_device,
            get_heart_rate,
            is_connected,
            set_osc_enabled,
            set_osc_port,
            get_osc_port,
            get_osc_enabled,
            get_osc_params,
            set_osc_params,
            set_ws_enabled,
            get_ws_enabled,
            set_ws_port,
            get_ws_port,
            set_always_on_top,
            get_always_on_top,
            set_start_minimized,
            get_start_minimized,
            set_language,
            get_language,
            set_graph_interval,
            get_graph_interval,
            set_recording_enabled,
            get_recording_enabled,
            set_record_interval,
            get_record_interval,
            set_flush_interval,
            get_flush_interval,
            set_auto_reconnect_enabled,
            get_auto_reconnect_enabled,
            set_auto_reconnect_interval,
            get_auto_reconnect_interval,
            set_hds_enabled,
            get_hds_enabled,
            set_hds_port,
            get_hds_port,
            get_lan_ip,
            get_remembered_devices,
            remove_remembered_device,
            read_hr_records,
            open_records_dir,
            set_stats_expanded,
            check_update,
            download_and_install_update,
            debug_updater,
            open_url,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                let state = app.state::<AppState>();
                // Flush any queued HR samples before the process exits.
                state.recorder.flush();
                save_config(&state);
                if *state.osc_enabled.lock().unwrap() {
                    let port = *state.osc_port.lock().unwrap();
                    let params = state.osc_params.lock().unwrap().clone();
                    let _ = osc::send_hr_params(
                        port,
                        &params,
                        &osc::HrState {
                            hr: 0,
                            is_connected: false,
                            is_active: false,
                            beat_toggle: false,
                        },
                    );
                }
            }
        });
}
