use rosc::{OscMessage, OscPacket, OscType};
use serde::{Deserialize, Serialize};
use std::net::UdpSocket;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OscParamNames {
    pub ones_hr: String,
    pub tens_hr: String,
    pub hundreds_hr: String,
    pub is_hr_connected: String,
    pub is_hr_active: String,
    pub is_hr_beat: String,
    pub hr_percent: String,
    pub full_hr_percent: String,
    pub hr: String,
}

impl Default for OscParamNames {
    fn default() -> Self {
        Self {
            ones_hr: "onesHR".to_string(),
            tens_hr: "tensHR".to_string(),
            hundreds_hr: "hundredsHR".to_string(),
            is_hr_connected: "isHRConnected".to_string(),
            is_hr_active: "isHRActive".to_string(),
            is_hr_beat: "isHRBeat".to_string(),
            hr_percent: "HRPercent".to_string(),
            full_hr_percent: "FullHRPercent".to_string(),
            hr: "HR".to_string(),
        }
    }
}

pub struct HrState {
    pub hr: u16,
    pub is_connected: bool,
    pub is_active: bool,
    pub beat_toggle: bool,
}

pub fn send_hr_params(
    port: u16,
    params: &OscParamNames,
    state: &HrState,
) -> Result<(), Box<dyn std::error::Error>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let addr = format!("127.0.0.1:{port}");

    let ones = (state.hr % 10) as i32;
    let tens = ((state.hr / 10) % 10) as i32;
    let hundreds = ((state.hr / 100) % 10) as i32;
    let hr_percent = (state.hr as f32 / 255.0).clamp(0.0, 1.0);
    let full_hr_percent = hr_percent;

    let messages: Vec<OscMessage> = vec![
        OscMessage {
            addr: format!("/avatar/parameters/{}", params.ones_hr),
            args: vec![OscType::Int(ones)],
        },
        OscMessage {
            addr: format!("/avatar/parameters/{}", params.tens_hr),
            args: vec![OscType::Int(tens)],
        },
        OscMessage {
            addr: format!("/avatar/parameters/{}", params.hundreds_hr),
            args: vec![OscType::Int(hundreds)],
        },
        OscMessage {
            addr: format!("/avatar/parameters/{}", params.is_hr_connected),
            args: vec![OscType::Bool(state.is_connected)],
        },
        OscMessage {
            addr: format!("/avatar/parameters/{}", params.is_hr_active),
            args: vec![OscType::Bool(state.is_active)],
        },
        OscMessage {
            addr: format!("/avatar/parameters/{}", params.is_hr_beat),
            args: vec![OscType::Bool(state.beat_toggle)],
        },
        OscMessage {
            addr: format!("/avatar/parameters/{}", params.hr_percent),
            args: vec![OscType::Float(hr_percent)],
        },
        OscMessage {
            addr: format!("/avatar/parameters/{}", params.full_hr_percent),
            args: vec![OscType::Float(full_hr_percent)],
        },
        OscMessage {
            addr: format!("/avatar/parameters/{}", params.hr),
            args: vec![OscType::Int(state.hr as i32)],
        },
    ];

    for msg in messages {
        let packet = OscPacket::Message(msg);
        let buf = rosc::encoder::encode(&packet)?;
        socket.send_to(&buf, &addr)?;
    }

    Ok(())
}

pub fn send_beat(port: u16, param_name: &str, beat: bool) -> Result<(), Box<dyn std::error::Error>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let addr = format!("127.0.0.1:{port}");
    let msg = OscMessage {
        addr: format!("/avatar/parameters/{param_name}"),
        args: vec![OscType::Bool(beat)],
    };
    let buf = rosc::encoder::encode(&OscPacket::Message(msg))?;
    socket.send_to(&buf, &addr)?;
    Ok(())
}
