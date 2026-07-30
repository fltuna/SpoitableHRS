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
use windows::Devices::Bluetooth::BluetoothLEDevice;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristic, GattClientCharacteristicConfigurationDescriptorValue,
    GattCommunicationStatus, GattValueChangedEventArgs,
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

/// Passive wait for any remembered device to advertise. Returns the device id
/// when one is seen, or None when cancelled (auto-reconnect disabled/suspended,
/// or a connection was established elsewhere).
pub async fn watch_for_devices(
    device_ids: Vec<String>,
    enabled: Arc<AtomicBool>,
    suspended: Arc<AtomicBool>,
    connected: Arc<Mutex<bool>>,
) -> Option<String> {
    let addresses: Vec<u64> = device_ids
        .iter()
        .filter_map(|id| parse_address(id).ok())
        .collect();
    if addresses.is_empty() {
        return None;
    }

    tokio::task::spawn_blocking(move || {
        let watcher = BluetoothLEAdvertisementWatcher::new().ok()?;
        watcher
            .SetScanningMode(BluetoothLEScanningMode::Active)
            .ok()?;

        let found: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
        let found_clone = found.clone();

        let handler = TypedEventHandler::<
            BluetoothLEAdvertisementWatcher,
            BluetoothLEAdvertisementReceivedEventArgs,
        >::new(move |_, args| {
            let Some(args) = &*args else { return Ok(()) };
            let address = args.BluetoothAddress()?;
            if addresses.contains(&address) {
                *found_clone.lock().unwrap() = Some(address);
            }
            Ok(())
        });

        watcher.Received(&handler).ok()?;
        watcher.Start().ok()?;

        let result = loop {
            if let Some(addr) = *found.lock().unwrap() {
                break Some(addr);
            }
            if !enabled.load(Ordering::Relaxed)
                || suspended.load(Ordering::Relaxed)
                || *connected.lock().unwrap()
            {
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        };

        let _ = watcher.Stop();
        result.map(format_address)
    })
    .await
    .ok()
    .flatten()
}

/// Try GATT connection. Returns Ok(true) if GATT is running, Ok(false) if it failed and we should fallback.
fn try_gatt(
    address: u64,
    hr_tx: mpsc::UnboundedSender<u16>,
    log_tx: mpsc::UnboundedSender<(String, String)>,
    app: tauri::AppHandle,
    stop: Arc<AtomicBool>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let log = |msg: &str, level: &str| {
        let _ = log_tx.send((msg.to_string(), level.to_string()));
    };

    log("[GATT] Attempting GATT connection...", "info");

    let device = match BluetoothLEDevice::FromBluetoothAddressAsync(address)
        .and_then(|op| op.get())
    {
        Ok(dev) => dev,
        Err(e) => {
            log(
                &format!("[GATT] Device not reachable: {e}, falling back to broadcast"),
                "warn",
            );
            return Ok(false);
        }
    };

    let hrs_uuid = ble_uuid(0x180D);
    let gatt_result = device
        .GetGattServicesForUuidAsync(hrs_uuid)?
        .get()?;

    if gatt_result.Status()? != GattCommunicationStatus::Success {
        log("[GATT] Cannot access HR Service, falling back to broadcast", "warn");
        return Ok(false);
    }

    let services = gatt_result.Services()?;
    if services.Size()? == 0 {
        log("[GATT] HR Service not found, falling back to broadcast", "warn");
        return Ok(false);
    }

    let hr_service = services.GetAt(0)?;
    let hr_char_uuid = ble_uuid(0x2A37);
    let char_result = hr_service
        .GetCharacteristicsForUuidAsync(hr_char_uuid)?
        .get()?;

    if char_result.Status()? != GattCommunicationStatus::Success {
        log("[GATT] Cannot access HR characteristic, falling back to broadcast", "warn");
        return Ok(false);
    }

    let chars = char_result.Characteristics()?;
    if chars.Size()? == 0 {
        log("[GATT] HR characteristic not found, falling back to broadcast", "warn");
        return Ok(false);
    }

    let hr_char = chars.GetAt(0)?;

    // Enable notifications
    let notify_status = hr_char
        .WriteClientCharacteristicConfigurationDescriptorAsync(
            GattClientCharacteristicConfigurationDescriptorValue::Notify,
        )?
        .get()?;

    if notify_status != GattCommunicationStatus::Success {
        log("[GATT] Failed to enable HR notifications, falling back to broadcast", "warn");
        return Ok(false);
    }

    log("[GATT] Connected! Receiving HR via GATT", "info");
    let _ = app.emit("connection-mode", "gatt");

    // Subscribe to HR notifications
    let hr_tx_clone = hr_tx.clone();
    let hr_handler =
        TypedEventHandler::<GattCharacteristic, GattValueChangedEventArgs>::new(move |_, args| {
            let Some(args) = &*args else { return Ok(()) };
            let value = args.CharacteristicValue()?;
            let reader = DataReader::FromBuffer(&value)?;
            let len = reader.UnconsumedBufferLength()? as usize;
            if len < 2 {
                return Ok(());
            }
            let mut bytes = vec![0u8; len];
            reader.ReadBytes(&mut bytes)?;

            let hr = parse_hrs_measurement(&bytes);
            if let Some(hr) = hr {
                let _ = hr_tx_clone.send(hr);
            }
            Ok(())
        });
    hr_char.ValueChanged(&hr_handler)?;

    // Try to read battery level
    try_read_battery(&device, &log_tx, &app);

    // Battery polling loop (every 60s)
    let bat_device_addr = address;
    let bat_log_tx = log_tx.clone();
    let bat_app = app.clone();
    let bat_stop = stop.clone();
    std::thread::spawn(move || {
        loop {
            if bat_stop.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(60));
            if bat_stop.load(Ordering::Relaxed) {
                break;
            }
            // Re-read battery from existing device
            if let Ok(dev) = BluetoothLEDevice::FromBluetoothAddressAsync(bat_device_addr)
                .and_then(|op| op.get())
            {
                try_read_battery(&dev, &bat_log_tx, &bat_app);
            }
        }
    });

    // Wait until stopped
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // Disable notifications
    let _ = hr_char.WriteClientCharacteristicConfigurationDescriptorAsync(
        GattClientCharacteristicConfigurationDescriptorValue::None,
    );

    log("[GATT] Disconnected", "info");
    Ok(true)
}

fn try_read_battery(
    device: &BluetoothLEDevice,
    log_tx: &mpsc::UnboundedSender<(String, String)>,
    app: &tauri::AppHandle,
) {
    let bat_uuid = ble_uuid(0x180F);
    let bat_char_uuid = ble_uuid(0x2A19);

    let Ok(bat_result) = device.GetGattServicesForUuidAsync(bat_uuid).and_then(|op| op.get())
    else {
        return;
    };
    if bat_result.Status().unwrap_or(GattCommunicationStatus::Unreachable)
        != GattCommunicationStatus::Success
    {
        return;
    }

    let Ok(bat_services) = bat_result.Services() else {
        return;
    };
    if bat_services.Size().unwrap_or(0) == 0 {
        return;
    }

    let Ok(bat_service) = bat_services.GetAt(0) else {
        return;
    };
    let Ok(char_result) = bat_service
        .GetCharacteristicsForUuidAsync(bat_char_uuid)
        .and_then(|op| op.get())
    else {
        return;
    };
    if char_result
        .Status()
        .unwrap_or(GattCommunicationStatus::Unreachable)
        != GattCommunicationStatus::Success
    {
        return;
    }

    let Ok(chars) = char_result.Characteristics() else {
        return;
    };
    if chars.Size().unwrap_or(0) == 0 {
        return;
    }

    let Ok(bat_char) = chars.GetAt(0) else {
        return;
    };

    if let Ok(read_result) = bat_char
        .ReadValueAsync()
        .and_then(|op| op.get())
    {
        if read_result
            .Status()
            .unwrap_or(GattCommunicationStatus::Unreachable)
            == GattCommunicationStatus::Success
        {
            if let Ok(value) = read_result.Value() {
                if let Ok(reader) = DataReader::FromBuffer(&value) {
                    if reader.UnconsumedBufferLength().unwrap_or(0) >= 1 {
                        if let Ok(level) = reader.ReadByte() {
                            let _ = log_tx.send((
                                format!("[GATT] Battery: {level}%"),
                                "info".to_string(),
                            ));
                            let _ = app.emit("battery-update", level as u16);
                        }
                    }
                }
            }
        }
    }
}

fn start_broadcast(
    address: u64,
    hr_tx: mpsc::UnboundedSender<u16>,
    log_tx: mpsc::UnboundedSender<(String, String)>,
    app: tauri::AppHandle,
    stop: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let log = |msg: &str, level: &str| {
        let _ = log_tx.send((msg.to_string(), level.to_string()));
    };

    log("[Broadcast] Starting broadcast mode...", "info");

    let watcher = BluetoothLEAdvertisementWatcher::new()?;
    watcher.SetScanningMode(BluetoothLEScanningMode::Active)?;

    let hr_tx_clone = hr_tx.clone();
    let log_tx_ref = log_tx.clone();
    let first_packet = Arc::new(AtomicBool::new(true));
    let parse_fail_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let unsupported_warned = Arc::new(AtomicBool::new(false));
    let app_ref = app.clone();

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

        let mut matched = false;
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
                let hex: Vec<String> = bytes
                    .iter()
                    .enumerate()
                    .map(|(i, b)| format!("[{i}]{b:02X}"))
                    .collect();
                let _ = log_tx_ref.send((
                    format!("[Broadcast] Data ({len} bytes): {}", hex.join(" ")),
                    "info".to_string(),
                ));
            }

            if let Some(hr) = parse_polar_broadcast(&bytes) {
                let _ = hr_tx_clone.send(hr);
                parse_fail_count.store(0, Ordering::Relaxed);
                matched = true;
            }
        }

        if !matched {
            let fails = parse_fail_count.fetch_add(1, Ordering::Relaxed) + 1;
            if fails >= 5 && !unsupported_warned.swap(true, Ordering::Relaxed) {
                let _ = log_tx_ref.send((
                    "[Broadcast] Unknown data pattern — device may not be supported".to_string(),
                    "warn".to_string(),
                ));
                let _ = app_ref.emit("connection-mode", "unsupported");
            }
        }

        Ok(())
    });

    watcher.Received(&handler)?;
    watcher.Start()?;
    log("[Broadcast] Listening for HR broadcasts...", "info");

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    watcher.Stop()?;
    log("[Broadcast] Receiver stopped", "info");
    Ok(())
}

pub async fn connect_and_subscribe(
    device_id: &str,
    device_name: &str,
    deps: crate::session::SessionDeps,
    app: tauri::AppHandle,
    stop_flag: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let address = parse_address(device_id)?;

    let (log_tx, mut log_rx) = mpsc::unbounded_channel::<(String, String)>();
    let (hr_tx, rx) = mpsc::unbounded_channel::<u16>();

    let log_app = app.clone();
    let log_task = tokio::spawn(async move {
        while let Some((msg, level)) = log_rx.recv().await {
            emit_log(&log_app, &msg, &level);
        }
    });

    let stop = stop_flag.clone();
    let ble_log_tx = log_tx.clone();
    let ble_hr_tx = hr_tx.clone();
    let ble_app = app.clone();
    let _ble_task = tokio::task::spawn_blocking(move || {
        // Try GATT first, fall back to broadcast
        match try_gatt(address, ble_hr_tx.clone(), ble_log_tx.clone(), ble_app.clone(), stop.clone()) {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                let _ = ble_app.emit("connection-mode", "broadcast");
                let _ = start_broadcast(address, ble_hr_tx, ble_log_tx, ble_app, stop);
            }
        }
    });

    // First real HR packet = the connection actually works;
    // remember the device for auto-reconnect.
    let cb_app = app.clone();
    let cb_id = device_id.to_string();
    let cb_name = device_name.to_string();
    let on_first_sample = Box::new(move || {
        crate::remember_device(&cb_app, &cb_id, &cb_name);
    });

    crate::session::run_session(
        crate::session::HrSource::Ble,
        rx,
        deps,
        app,
        stop_flag,
        Some(on_first_sample),
    )
    .await;

    log_task.abort();
    Ok(())
}

fn parse_hrs_measurement(data: &[u8]) -> Option<u16> {
    if data.is_empty() {
        return None;
    }
    let flags = data[0];
    let hr_16bit = flags & 0x01 != 0;
    if hr_16bit {
        if data.len() < 3 {
            return None;
        }
        Some(u16::from_le_bytes([data[1], data[2]]))
    } else {
        if data.len() < 2 {
            return None;
        }
        Some(data[1] as u16)
    }
}

fn parse_polar_broadcast(data: &[u8]) -> Option<u16> {
    if data.len() >= 10 {
        let hr = *data.last().unwrap() as u16;
        if hr >= 30 && hr <= 240 {
            return Some(hr);
        }
    }
    None
}
