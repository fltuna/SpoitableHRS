use crate::osc::OscParamNames;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub osc_enabled: bool,
    pub osc_port: u16,
    pub osc_params: OscParamNames,
    pub ws_enabled: bool,
    pub ws_port: u16,
    pub always_on_top: bool,
    pub start_minimized: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            osc_enabled: true,
            osc_port: 9000,
            osc_params: OscParamNames::default(),
            ws_enabled: true,
            ws_port: 9100,
            always_on_top: false,
            start_minimized: false,
        }
    }
}

fn config_path() -> PathBuf {
    let dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("SpoitableHRS");
    fs::create_dir_all(&dir).ok();
    dir.join("config.json")
}

pub fn load() -> AppConfig {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => {
            let cfg = AppConfig::default();
            save(&cfg);
            cfg
        }
    }
}

pub fn save(cfg: &AppConfig) {
    let path = config_path();
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        fs::write(path, json).ok();
    }
}
