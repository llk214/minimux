use serde::{Deserialize, Serialize};

/// Messages sent from client to daemon.
#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Raw input bytes from the client terminal.
    Input(Vec<u8>),
    /// Terminal was resized.
    Resize { cols: u16, rows: u16 },
    /// Client is detaching gracefully.
    Detach,
}

/// Messages sent from daemon to client.
#[derive(Debug, Serialize, Deserialize)]
pub enum DaemonMsg {
    /// Raw output bytes from the PTY.
    Output(Vec<u8>),
    /// Scrollback replay on reattach.
    ScrollbackReplay(Vec<u8>),
    /// The shell inside the PTY has exited.
    SessionEnded,
}

/// Pipe name for IPC.
pub const PIPE_NAME: &str = r"\\.\pipe\minimux";

/// Encode a message as a length-prefixed bincode frame.
pub fn encode<T: Serialize>(msg: &T) -> anyhow::Result<Vec<u8>> {
    let payload = bincode::serialize(msg)?;
    let len = (payload.len() as u32).to_le_bytes();
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len);
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Read one length-prefixed frame from a byte buffer.
/// Returns (parsed message, bytes consumed) or None if not enough data.
pub fn decode<T: for<'de> Deserialize<'de>>(buf: &[u8]) -> anyhow::Result<Option<(T, usize)>> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if buf.len() < 4 + len {
        return Ok(None);
    }
    let msg: T = bincode::deserialize(&buf[4..4 + len])?;
    Ok(Some((msg, 4 + len)))
}
