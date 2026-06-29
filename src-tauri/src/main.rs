#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ble;
mod osc;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::State;

pub struct AppState {
    pub heart_rate: Arc<Mutex<u16>>,
    pub connected: Arc<Mutex<bool>>,
    pub osc_enabled: Arc<Mutex<bool>>,
    pub osc_port: Arc<Mutex<u16>>,
    pub ble_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    pub stop_flag: Arc<AtomicBool>,
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
    let stop = state.stop_flag.clone();

    let handle = tokio::spawn(async move {
        if let Err(e) = ble::connect_and_subscribe(
            &device_id, hr, connected, osc_enabled, osc_port, app.clone(), stop,
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
}

#[tauri::command]
fn set_osc_port(state: State<'_, AppState>, port: u16) {
    *state.osc_port.lock().unwrap() = port;
}

#[tauri::command]
fn get_osc_port(state: State<'_, AppState>) -> u16 {
    *state.osc_port.lock().unwrap()
}

#[tauri::command]
fn get_osc_enabled(state: State<'_, AppState>) -> bool {
    *state.osc_enabled.lock().unwrap()
}

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            heart_rate: Arc::new(Mutex::new(0)),
            connected: Arc::new(Mutex::new(false)),
            osc_enabled: Arc::new(Mutex::new(true)),
            osc_port: Arc::new(Mutex::new(9000)),
            ble_handle: Arc::new(Mutex::new(None)),
            stop_flag: Arc::new(AtomicBool::new(false)),
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
