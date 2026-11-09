use std::time::Duration;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug, Serialize, Deserialize)]
pub struct MinecraftPlayer {
    pub name: String,
    pub uuid: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MinecraftPingResult {
    pub players: Vec<MinecraftPlayer>,
    #[serde(rename = "maxPlayers")]
    pub max_players: u32,
    #[serde(rename = "onlinePlayers")]
    pub online_players: u32,
    pub description: String,
    pub version: String,
    pub online: bool,
}

fn write_varint(buf: &mut Vec<u8>, mut val: i32) {
    loop {
        let mut byte = (val & 0b0111_1111) as u8;
        val >>= 7;
        if val != 0 {
            byte |= 0b1000_0000;
        }
        buf.push(byte);
        if val == 0 {
            break;
        }
    }
}

async fn read_varint<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<i32, String> {
    let mut num_read = 0;
    let mut result = 0;
    loop {
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte).await.map_err(|e| e.to_string())?;
        let value = (byte[0] & 0b0111_1111) as i32;
        result |= value << (7 * num_read);
        num_read += 1;
        if num_read > 5 {
            return Err("VarInt is too big".to_string());
        }
        if (byte[0] & 0b1000_0000) == 0 {
            break;
        }
    }
    Ok(result)
}

pub async fn ping_minecraft_server(host: &str, port: u16, timeout_ms: u64) -> Result<MinecraftPingResult, String> {
    let addr = format!("{}:{}", host, port);
    let connect_future = TcpStream::connect(&addr);
    let mut stream = tokio::time::timeout(Duration::from_millis(timeout_ms), connect_future)
        .await
        .map_err(|_| "Connection timed out".to_string())?
        .map_err(|e| e.to_string())?;

    // 1. Handshake packet: Packet ID 0x00, Protocol version 47, Server address, Server port, Next state 1
    let mut handshake_data = Vec::new();
    write_varint(&mut handshake_data, 0x00); // Packet ID
    write_varint(&mut handshake_data, 47);   // Protocol version 1.8.x
    // String host
    let host_bytes = host.as_bytes();
    write_varint(&mut handshake_data, host_bytes.len() as i32);
    handshake_data.extend_from_slice(host_bytes);
    // Port
    handshake_data.extend_from_slice(&port.to_be_bytes());
    // Next state: 1 (status)
    write_varint(&mut handshake_data, 1);

    // Prefix with handshake packet length
    let mut handshake_packet = Vec::new();
    write_varint(&mut handshake_packet, handshake_data.len() as i32);
    handshake_packet.extend(handshake_data);

    // 2. Request packet: Packet ID 0x00 (length 1)
    let request_packet = vec![0x01, 0x00];

    stream.write_all(&handshake_packet).await.map_err(|e| e.to_string())?;
    stream.write_all(&request_packet).await.map_err(|e| e.to_string())?;

    // 3. Read response: length, packet ID, string length, JSON string
    let _packet_len = read_varint(&mut stream).await?;
    let packet_id = read_varint(&mut stream).await?;
    if packet_id != 0x00 {
        return Err("Unexpected packet ID in response".to_string());
    }

    let str_len = read_varint(&mut stream).await? as usize;
    let mut str_bytes = vec![0u8; str_len];
    stream.read_exact(&mut str_bytes).await.map_err(|e| e.to_string())?;

    let json_str = String::from_utf8(str_bytes).map_err(|e| e.to_string())?;
    let val: serde_json::Value = serde_json::from_str(&json_str).map_err(|e| e.to_string())?;

    let players_val = val.get("players");
    let max_players = players_val.and_then(|p| p.get("max")).and_then(|m| m.as_u64()).unwrap_or(0) as u32;
    let online_players = players_val.and_then(|p| p.get("online")).and_then(|o| o.as_u64()).unwrap_or(0) as u32;

    let mut players = Vec::new();
    if let Some(sample) = players_val.and_then(|p| p.get("sample")).and_then(|s| s.as_array()) {
        for p in sample {
            if let (Some(name), Some(id)) = (p.get("name").and_then(|n| n.as_str()), p.get("id").and_then(|i| i.as_str())) {
                players.push(MinecraftPlayer {
                    name: name.to_string(),
                    uuid: id.to_string(),
                });
            }
        }
    }

    let description = if let Some(desc) = val.get("description") {
        if let Some(s) = desc.as_str() {
            s.to_string()
        } else if let Some(t) = desc.get("text").and_then(|t| t.as_str()) {
            t.to_string()
        } else {
            "".to_string()
        }
    } else {
        "".to_string()
    };

    let version = val.get("version")
        .and_then(|v| v.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();

    Ok(MinecraftPingResult {
        players,
        max_players,
        online_players,
        description,
        version,
        online: true,
    })
}
