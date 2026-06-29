use rosc::{OscMessage, OscPacket, OscType};
use std::net::UdpSocket;

pub fn send_heart_rate(port: u16, hr: u16) -> Result<(), Box<dyn std::error::Error>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let msg = OscMessage {
        addr: "/avatar/parameters/HR".to_string(),
        args: vec![OscType::Int(hr as i32)],
    };
    let packet = OscPacket::Message(msg);
    let buf = rosc::encoder::encode(&packet)?;
    socket.send_to(&buf, format!("127.0.0.1:{port}"))?;
    Ok(())
}
