use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Emitter;
use tokio::sync::mpsc;
use windows::core::GUID;
use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
    BluetoothLEScanningMode,
};
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::DataReader;

const POLAR_COMPANY_ID: u16 = 0x006B;

fn ble_uuid(short: u16) -> GUID {
    GUID {
        data1: short as u32,
        data2: 0x0000,
        data3: 0x1000,
        data4: [0x80, 0x00, 0x00, 0x80, 0x5F, 0x9B, 0x34, 0xFB],
    }
}

fn format_address(addr: u64) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        (addr >> 40) & 0xFF,
        (addr >> 32) & 0xFF,
        (addr >> 24) & 0xFF,
        (addr >> 16) & 0xFF,
        (addr >> 8) & 0xFF,
        addr & 0xFF,
    )
}

fn parse_address(addr: &str) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let hex: String = addr.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    Ok(u64::from_str_radix(&hex, 16)?)
}

#[derive(Debug, Serialize, Clone)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Clone)]
pub struct LogEntry {
    pub message: String,
    pub level: String,
}

pub fn emit_log(app: &tauri::AppHandle, message: &str, level: &str) {
    let _ = app.emit(
        "ble-log",
        LogEntry {
            message: message.to_string(),
            level: level.to_string(),
        },
    );
}

pub async fn scan() -> Result<Vec<DeviceInfo>, Box<dyn std::error::Error + Send + Sync>> {
    let result = tokio::task::spawn_blocking(|| {
        let watcher = BluetoothLEAdvertisementWatcher::new()?;
        watcher.SetScanningMode(BluetoothLEScanningMode::Active)?;

        let devices: Arc<Mutex<HashMap<u64, (String, bool)>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let devices_clone = devices.clone();
        let hr_guid = ble_uuid(0x180D);

        let handler = TypedEventHandler::<
            BluetoothLEAdvertisementWatcher,
            BluetoothLEAdvertisementReceivedEventArgs,
        >::new(move |_, args| {
            let Some(args) = &*args else { return Ok(()) };
            let address = args.BluetoothAddress()?;
            let adv = args.Advertisement()?;
            let name = adv.LocalName()?.to_string();

            if !name.is_empty() {
                let service_uuids = adv.ServiceUuids()?;
                let mut has_hr = false;
                if let Ok(size) = service_uuids.Size() {
                    for i in 0..size {
                        if let Ok(uuid) = service_uuids.GetAt(i) {
                            if uuid == hr_guid {
                                has_hr = true;
                                break;
                            }
                        }
                    }
                }

                let mfr_data = adv.ManufacturerData();
                if let Ok(mfr) = mfr_data {
                    if let Ok(size) = mfr.Size() {
                        for i in 0..size {
                            if let Ok(d) = mfr.GetAt(i) {
                                if d.CompanyId().unwrap_or(0) == POLAR_COMPANY_ID {
                                    has_hr = true;
                                }
                            }
                        }
                    }
                }

                devices_clone.lock().unwrap().insert(address, (name, has_hr));
            }
            Ok(())
        });

        watcher.Received(&handler)?;
        watcher.Start()?;
        std::thread::sleep(std::time::Duration::from_secs(4));
        watcher.Stop()?;

        let devs = devices.lock().unwrap();
        let result: Vec<DeviceInfo> = devs
            .iter()
            .map(|(addr, (name, has_hr))| {
                let suffix = if *has_hr { " [HR]" } else { "" };
                DeviceInfo {
                    id: format_address(*addr),
                    name: format!("{name}{suffix}"),
                }
            })
            .collect();
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(result)
    })
    .await??;
    Ok(result)
}

pub async fn connect_and_subscribe(
    device_id: &str,
    heart_rate: Arc<Mutex<u16>>,
    connected: Arc<Mutex<bool>>,
    osc_enabled: Arc<Mutex<bool>>,
    osc_port: Arc<Mutex<u16>>,
    osc_params: Arc<Mutex<crate::osc::OscParamNames>>,
    beat_toggle: Arc<AtomicBool>,
    ws_broadcaster: Arc<crate::ws::WsBroadcaster>,
    ws_enabled: Arc<AtomicBool>,
    graph_interval_ms: Arc<Mutex<u64>>,
    app: tauri::AppHandle,
    stop_flag: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let address = parse_address(device_id)?;

    let (log_tx, mut log_rx) = mpsc::unbounded_channel::<(String, String)>();
    let (hr_tx, mut rx) = mpsc::unbounded_channel::<u16>();

    let log_app = app.clone();
    let log_task = tokio::spawn(async move {
        while let Some((msg, level)) = log_rx.recv().await {
            emit_log(&log_app, &msg, &level);
        }
    });

    let stop = stop_flag.clone();
    let _ble_task = tokio::task::spawn_blocking(move || {
        let log = |msg: &str, level: &str| {
            let _ = log_tx.send((msg.to_string(), level.to_string()));
        };

        log("Starting broadcast mode (no exclusive connection needed)...", "info");

        let watcher = BluetoothLEAdvertisementWatcher::new()?;
        watcher.SetScanningMode(BluetoothLEScanningMode::Active)?;

        let hr_tx_clone = hr_tx.clone();
        let log_tx_ref = log_tx.clone();
        let first_packet = Arc::new(AtomicBool::new(true));

        let handler = TypedEventHandler::<
            BluetoothLEAdvertisementWatcher,
            BluetoothLEAdvertisementReceivedEventArgs,
        >::new(move |_, args| {
            let Some(args) = &*args else { return Ok(()) };

            if args.BluetoothAddress()? != address {
                return Ok(());
            }

            let adv = args.Advertisement()?;
            let mfr_data = adv.ManufacturerData()?;

            for i in 0..mfr_data.Size()? {
                let data = mfr_data.GetAt(i)?;
                if data.CompanyId()? != POLAR_COMPANY_ID {
                    continue;
                }

                let buffer = data.Data()?;
                let reader = DataReader::FromBuffer(&buffer)?;
                let len = reader.UnconsumedBufferLength()? as usize;
                let mut bytes = vec![0u8; len];
                reader.ReadBytes(&mut bytes)?;

                if first_packet.swap(false, Ordering::Relaxed) {
                    let hex: Vec<String> = bytes.iter().enumerate().map(|(i, b)| format!("[{i}]{b:02X}")).collect();
                    let _ = log_tx_ref.send((
                        format!("Broadcast data ({len} bytes): {}", hex.join(" ")),
                        "info".to_string(),
                    ));
                }

                if let Some(hr) = parse_polar_broadcast(&bytes) {
                    let _ = hr_tx_clone.send(hr);
                }
            }
            Ok(())
        });

        watcher.Received(&handler)?;
        watcher.Start()?;
        log("Listening for HR broadcasts...", "info");

        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        watcher.Stop()?;
        log("Broadcast receiver stopped", "info");
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });

    *connected.lock().unwrap() = true;
    let _ = app.emit("connection-changed", true);

    let hr_sum: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let hr_count: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let hr_min: Arc<Mutex<u16>> = Arc::new(Mutex::new(u16::MAX));
    let hr_max: Arc<Mutex<u16>> = Arc::new(Mutex::new(0));

    // Beat loop: toggles is_hr_beat + sends all OSC params at HR-derived interval
    let beat_hr = heart_rate.clone();
    let beat_flag = beat_toggle.clone();
    let beat_osc_enabled = osc_enabled.clone();
    let beat_osc_port = osc_port.clone();
    let beat_osc_params = osc_params.clone();
    let beat_stop = stop_flag.clone();
    let beat_app = app.clone();
    let beat_task = tokio::spawn(async move {
        loop {
            if beat_stop.load(Ordering::Relaxed) {
                break;
            }
            let hr = *beat_hr.lock().unwrap();
            if hr > 0 && *beat_osc_enabled.lock().unwrap() {
                let interval_ms = (60_000u64).checked_div(hr as u64).unwrap_or(750);
                let toggle = !beat_flag.fetch_xor(true, Ordering::Relaxed);
                let port = *beat_osc_port.lock().unwrap();
                let params = beat_osc_params.lock().unwrap().clone();
                let state = crate::osc::HrState {
                    hr,
                    is_connected: true,
                    is_active: true,
                    beat_toggle: toggle,
                };
                if let Err(e) = crate::osc::send_hr_params(port, &params, &state) {
                    emit_log(&beat_app, &format!("OSC send error: {e}"), "error");
                }
                tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    });

    // WS broadcast loop: sends overlay data at configurable interval
    let ws_hr = heart_rate.clone();
    let ws_sum = hr_sum.clone();
    let ws_count = hr_count.clone();
    let ws_min = hr_min.clone();
    let ws_max = hr_max.clone();
    let ws_stop = stop_flag.clone();
    let ws_interval = graph_interval_ms;
    let ws_task = tokio::spawn(async move {
        loop {
            if ws_stop.load(Ordering::Relaxed) {
                break;
            }
            if ws_enabled.load(Ordering::Relaxed) {
                let hr = *ws_hr.lock().unwrap();
                if hr > 0 {
                    let count = *ws_count.lock().unwrap();
                    let avg = if count > 0 { (*ws_sum.lock().unwrap() / count) as u16 } else { hr };
                    let mn = *ws_min.lock().unwrap();
                    let mx = *ws_max.lock().unwrap();
                    let zone = if hr >= 140 { "hard" } else if hr >= 120 { "moderate" } else if hr >= 100 { "light" } else { "rest" };
                    let json = format!(r#"{{"type":"hr_update","bpm":{hr},"zone":"{zone}","connected":true,"avg":{avg},"min":{mn},"max":{mx}}}"#);
                    ws_broadcaster.send(&json);
                }
            }
            let interval = *ws_interval.lock().unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(interval)).await;
        }
    });

    // BLE receive loop: update shared state + emit UI event
    while let Some(hr) = rx.recv().await {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        *hr_sum.lock().unwrap() += hr as u64;
        *hr_count.lock().unwrap() += 1;
        if hr < *hr_min.lock().unwrap() { *hr_min.lock().unwrap() = hr; }
        if hr > *hr_max.lock().unwrap() { *hr_max.lock().unwrap() = hr; }

        *heart_rate.lock().unwrap() = hr;
        let _ = app.emit("heart-rate-update", hr);
    }

    beat_task.abort();
    ws_task.abort();
    emit_log(&app, "Broadcast receiver stopped", "info");
    *connected.lock().unwrap() = false;
    let _ = app.emit("connection-changed", false);
    log_task.abort();
    Ok(())
}

fn parse_polar_broadcast(data: &[u8]) -> Option<u16> {
    // Polar Verity Sense: HR is the last byte of manufacturer data
    // Packet length varies (13, 16 bytes observed) but HR is always last
    if data.len() >= 10 {
        let hr = *data.last().unwrap() as u16;
        if hr >= 30 && hr <= 240 {
            return Some(hr);
        }
    }
    None
}

