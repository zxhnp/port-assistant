mod frame;
mod session;

use serde::{Deserialize, Serialize};
pub use session::{Session, connect};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_FRAME: usize = 1024 * 1024;
const MAX_DELIMITER: usize = 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    host: String,
    port: u16,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Connection {
    Serial {
        port: String,
        baud_rate: u32,
        data_bits: u8,
        parity: Parity,
        stop_bits: u8,
        flow_control: FlowControl,
    },
    Udp {
        local: Address,
        #[serde(default)]
        mode: UdpMode,
        group: Option<String>,
        interface: Option<String>,
        multicast_ttl: Option<u8>,
    },
    TcpClient {
        remote: Address,
    },
    TcpServer {
        local: Address,
    },
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UdpMode {
    #[default]
    Unicast,
    Broadcast,
    Multicast,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Parity {
    None,
    Odd,
    Even,
    Mark,
    Space,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlowControl {
    None,
    Software,
    Hardware,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Framing {
    header: Vec<u8>,
    tail: Vec<u8>,
}

impl Framing {
    fn validate(&self) -> Result<(), String> {
        if self.header.is_empty()
            || self.tail.is_empty()
            || self.header.len() > MAX_DELIMITER
            || self.tail.len() > MAX_DELIMITER
        {
            return Err("帧头和帧尾各需 1～1024 个字节".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Packet {
    bytes: Vec<u8>,
    header_len: usize,
    tail_len: usize,
}

impl Packet {
    fn validate(&self) -> Result<(), String> {
        if self.bytes.is_empty() || self.bytes.len() > MAX_FRAME {
            return Err("发送内容需为 1 字节至 1 MiB".into());
        }
        if self.header_len > MAX_DELIMITER
            || self.tail_len > MAX_DELIMITER
            || self.header_len + self.tail_len > self.bytes.len()
        {
            return Err("帧头或帧尾长度无效".into());
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Target {
    Peer { id: u64 },
    Udp { host: String, port: u16 },
}

#[derive(Clone, Debug, Serialize)]
pub struct Peer {
    id: u64,
    address: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum Direction {
    Rx,
    Tx,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    direction: Direction,
    timestamp: u64,
    peer: Option<String>,
    packet: Packet,
}

impl Record {
    fn new(direction: Direction, peer: Option<String>, packet: Packet) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            direction,
            timestamp,
            peer,
            packet,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    Status {
        connected: bool,
        endpoint: String,
        clients: Vec<Peer>,
    },
    Messages {
        rows: Vec<Record>,
    },
    Timer {
        running: bool,
    },
    Fault {
        message: String,
    },
}

pub fn list_ports() -> Result<Vec<String>, String> {
    let mut ports: Vec<_> = tokio_serial::available_ports()
        .map_err(|error| format!("无法枚举串口：{error}"))?
        .into_iter()
        .map(|port| port.port_name)
        .collect();
    ports.sort_unstable_by_key(|port| {
        (
            port.strip_prefix("COM").and_then(|n| n.parse::<u32>().ok()),
            port.clone(),
        )
    });
    Ok(ports)
}
