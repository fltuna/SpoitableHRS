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
    pub language: String,
    #[serde(default = "default_graph_interval")]
    pub graph_interval_ms: u64,
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
            language: detect_os_language(),
            graph_interval_ms: default_graph_interval(),
        }
    }
}

fn default_graph_interval() -> u64 {
    800
}

fn detect_os_language() -> String {
    let lang = std::env::var("LANG")
        .or_else(|_| std::env::var("LC_ALL"))
        .or_else(|_| std::env::var("LANGUAGE"))
        .unwrap_or_default();

    if lang.starts_with("ja") {
        return "ja".to_string();
    }

    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        if let Ok(out) = Command::new("powershell")
            .args(["-NoProfile", "-Command", "(Get-Culture).TwoLetterISOLanguageName"])
            .output()
        {
            let code = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if code == "ja" {
                return "ja".to_string();
            }
        }
    }

    "en".to_string()
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
