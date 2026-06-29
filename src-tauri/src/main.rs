#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ble;
mod config;
mod osc;
mod ws;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::State;

pub struct AppState {
    pub heart_rate: Arc<Mutex<u16>>,
    pub connected: Arc<Mutex<bool>>,
    pub osc_enabled: Arc<Mutex<bool>>,
    pub osc_port: Arc<Mutex<u16>>,
    pub osc_params: Arc<Mutex<osc::OscParamNames>>,
    pub ble_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    pub stop_flag: Arc<AtomicBool>,
    pub beat_toggle: Arc<AtomicBool>,
    pub ws_broadcaster: Arc<ws::WsBroadcaster>,
    pub ws_enabled: Arc<AtomicBool>,
    pub ws_port: Arc<Mutex<u16>>,
    pub always_on_top: Arc<AtomicBool>,
    pub start_minimized: Arc<AtomicBool>,
    pub language: Arc<Mutex<String>>,
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
    };
    config::save(&cfg);
}

#[tauri::command]
async fn scan_devices() -> Result<Vec<ble::DeviceInfo>, String> {
    ble::scan().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn connect_device(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    device_id: String,
) -> Result<(), String> {
    if let Some(handle) = state.ble_handle.lock().unwrap().take() {
        state.stop_flag.store(true, Ordering::Relaxed);
        handle.abort();
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    state.stop_flag.store(false, Ordering::Relaxed);

    let hr = state.heart_rate.clone();
    let connected = state.connected.clone();
    let osc_enabled = state.osc_enabled.clone();
    let osc_port = state.osc_port.clone();
    let osc_params = state.osc_params.clone();
    let stop = state.stop_flag.clone();
    let beat_toggle = state.beat_toggle.clone();
    let ws_bc = state.ws_broadcaster.clone();
    let ws_enabled = state.ws_enabled.clone();

    let handle = tokio::spawn(async move {
        if let Err(e) = ble::connect_and_subscribe(
            &device_id, hr, connected, osc_enabled, osc_port, osc_params, beat_toggle,
            ws_bc, ws_enabled, app.clone(), stop,
        )
        .await
        {
            ble::emit_log(&app, &format!("BLE error: {e}"), "error");
        }
    });

    *state.ble_handle.lock().unwrap() = Some(handle);
    Ok(())
}

#[tauri::command]
async fn disconnect_device(state: State<'_, AppState>) -> Result<(), String> {
    state.stop_flag.store(true, Ordering::Relaxed);
    if let Some(handle) = state.ble_handle.lock().unwrap().take() {
        handle.abort();
    }
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
fn open_url(url: String) -> Result<(), String> {
    std::process::Command::new("cmd")
        .args(["/c", "start", "", &url])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
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

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
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

            Ok(())
        })
        .manage(AppState {
            heart_rate: Arc::new(Mutex::new(0)),
            connected: Arc::new(Mutex::new(false)),
            osc_enabled: Arc::new(Mutex::new(cfg.osc_enabled)),
            osc_port: Arc::new(Mutex::new(cfg.osc_port)),
            osc_params: Arc::new(Mutex::new(cfg.osc_params)),
            ble_handle: Arc::new(Mutex::new(None)),
            stop_flag: Arc::new(AtomicBool::new(false)),
            beat_toggle: Arc::new(AtomicBool::new(false)),
            ws_broadcaster,
            ws_enabled: Arc::new(AtomicBool::new(cfg.ws_enabled)),
            ws_port: Arc::new(Mutex::new(cfg.ws_port)),
            always_on_top: Arc::new(AtomicBool::new(cfg.always_on_top)),
            start_minimized: Arc::new(AtomicBool::new(cfg.start_minimized)),
            language: Arc::new(Mutex::new(cfg.language)),
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
            check_update,
            open_url,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                let state = app.state::<AppState>();
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
