use std::io::{Read, Write};
use std::thread;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal;

use crate::protocol::{self, ClientMsg, DaemonMsg, PIPE_NAME};

/// Connect to the daemon and relay I/O until detach or session end.
pub fn attach() -> Result<()> {
    let pipe = connect_to_daemon().context("Failed to connect to daemon")?;
    let mut pipe_writer = pipe.try_clone()?;
    let mut pipe_reader = pipe;

    // Send initial terminal size.
    let (cols, rows) = terminal::size().unwrap_or((120, 30));
    let resize_msg = protocol::encode(&ClientMsg::Resize { cols, rows })?;
    pipe_writer.write_all(&resize_msg)?;

    // Enter raw mode.
    terminal::enable_raw_mode()?;
    let _raw_guard = RawModeGuard;

    let mut stdout = std::io::stdout();

    // Thread: read daemon messages → stdout.
    let daemon_reader = thread::spawn(move || -> Result<()> {
        let mut buf = vec![0u8; 8192];
        let mut msg_buf = Vec::new();
        loop {
            match pipe_reader.read(&mut buf) {
                Ok(0) => break, // Pipe closed.
                Ok(n) => {
                    msg_buf.extend_from_slice(&buf[..n]);
                    loop {
                        match protocol::decode::<DaemonMsg>(&msg_buf) {
                            Ok(Some((msg, consumed))) => {
                                msg_buf.drain(..consumed);
                                match msg {
                                    DaemonMsg::Output(data) => {
                                        stdout.write_all(&data)?;
                                        stdout.flush()?;
                                    }
                                    DaemonMsg::ScrollbackReplay(data) => {
                                        stdout.write_all(&data)?;
                                        stdout.flush()?;
                                    }
                                    DaemonMsg::SessionEnded => {
                                        return Ok(());
                                    }
                                }
                            }
                            Ok(None) => break,
                            Err(_) => return Ok(()),
                        }
                    }
                }
                Err(_) => break,
            }
        }
        Ok(())
    });

    // Main thread: read terminal input → daemon.
    loop {
        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(KeyEvent {
                    code: KeyCode::Char('d'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                }) => {
                    // Ctrl+D with no input: detach.
                    let msg = protocol::encode(&ClientMsg::Detach)?;
                    let _ = pipe_writer.write_all(&msg);
                    break;
                }
                Event::Key(key_event) => {
                    if let Some(bytes) = key_event_to_bytes(&key_event) {
                        let msg = protocol::encode(&ClientMsg::Input(bytes))?;
                        if pipe_writer.write_all(&msg).is_err() {
                            break;
                        }
                    }
                }
                Event::Resize(cols, rows) => {
                    let msg = protocol::encode(&ClientMsg::Resize { cols, rows })?;
                    if pipe_writer.write_all(&msg).is_err() {
                        break;
                    }
                }
                _ => {}
            }
        }

        // Check if daemon reader thread has finished.
        if daemon_reader.is_finished() {
            break;
        }
    }

    Ok(())
}

/// RAII guard to restore terminal mode on drop.
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

/// Convert a crossterm key event to the bytes that should be sent to the PTY.
fn key_event_to_bytes(event: &KeyEvent) -> Option<Vec<u8>> {
    let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
    match event.code {
        KeyCode::Char(c) => {
            if ctrl {
                // Ctrl+A = 0x01, Ctrl+B = 0x02, ...
                let byte = (c as u8).wrapping_sub(b'a').wrapping_add(1);
                Some(vec![byte])
            } else {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                Some(s.as_bytes().to_vec())
            }
        }
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        KeyCode::F(n) => {
            let seq = match n {
                1 => "\x1bOP",
                2 => "\x1bOQ",
                3 => "\x1bOR",
                4 => "\x1bOS",
                5 => "\x1b[15~",
                6 => "\x1b[17~",
                7 => "\x1b[18~",
                8 => "\x1b[19~",
                9 => "\x1b[20~",
                10 => "\x1b[21~",
                11 => "\x1b[23~",
                12 => "\x1b[24~",
                _ => return None,
            };
            Some(seq.as_bytes().to_vec())
        }
        _ => None,
    }
}

#[cfg(windows)]
fn connect_to_daemon() -> Result<std::fs::File> {
    use std::fs::OpenOptions;

    // Try connecting to the named pipe.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE_NAME)
        .context("Could not connect to daemon pipe")?;
    Ok(file)
}

#[cfg(not(windows))]
fn connect_to_daemon() -> Result<std::fs::File> {
    anyhow::bail!("Named pipes only supported on Windows");
}
