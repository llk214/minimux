use std::io::{Read, Write};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;

use crate::protocol::{self, ClientMsg, DaemonMsg, PIPE_NAME};

/// Connect to the daemon and relay I/O until detach or session end.
pub fn attach() -> Result<()> {
    let mut pipe = connect_to_daemon().context("Failed to connect to daemon")?;

    // Send initial terminal size.
    let (cols, rows) = terminal::size().unwrap_or((120, 30));
    let resize_msg = protocol::encode(&ClientMsg::Resize { cols, rows })?;
    pipe.write_all(&resize_msg)?;

    // Enable VT sequence processing on stdout so that escape sequences from
    // the ConPTY are rendered correctly (required on legacy consoles).
    enable_virtual_terminal_processing();

    // Enter raw mode.
    terminal::enable_raw_mode()?;
    let _raw_guard = RawModeGuard;

    let mut stdout = std::io::stdout();
    let mut msg_buf = Vec::new();
    let mut read_buf = vec![0u8; 8192];
    let mut prefix_mode = false;

    // Simple poll loop: drain pipe output, then wait for keyboard input with
    // a short timeout. The timeout ensures we check for new pipe data
    // frequently even when no keys are pressed.
    loop {
        // Drain any available pipe data → stdout.
        if drain_pipe(&mut pipe, &mut msg_buf, &mut read_buf, &mut stdout)? {
            break; // session ended or pipe closed
        }

        // Wait up to 5ms for a keyboard event (also serves as our sleep to
        // avoid busy-waiting). Pipe output latency is at most 5ms.
        if event::poll(std::time::Duration::from_millis(5))? {
            match event::read()? {
                // Ctrl+B: enter prefix mode (tmux-style).
                Event::Key(KeyEvent {
                    code: KeyCode::Char('b'),
                    modifiers: KeyModifiers::CONTROL,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    if prefix_mode {
                        // Ctrl+B Ctrl+B: send literal Ctrl+B to the PTY.
                        prefix_mode = false;
                        let msg = protocol::encode(&ClientMsg::Input(vec![0x02]))?;
                        if pipe.write_all(&msg).is_err() {
                            return Ok(());
                        }
                    } else {
                        prefix_mode = true;
                    }
                }
                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                    if prefix_mode {
                        prefix_mode = false;
                        match key_event.code {
                            // Ctrl+B d: detach.
                            KeyCode::Char('d') => {
                                let msg = protocol::encode(&ClientMsg::Detach)?;
                                let _ = pipe.write_all(&msg);
                                return Ok(());
                            }
                            // Unknown prefix command — ignore.
                            _ => {}
                        }
                    } else if let Some(bytes) = key_event_to_bytes(&key_event) {
                        let msg = protocol::encode(&ClientMsg::Input(bytes))?;
                        if pipe.write_all(&msg).is_err() {
                            return Ok(());
                        }
                    }
                }
                Event::Resize(cols, rows) => {
                    let msg = protocol::encode(&ClientMsg::Resize { cols, rows })?;
                    if pipe.write_all(&msg).is_err() {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Read all available data from the pipe and write decoded output to stdout.
/// Returns true if the session has ended (pipe closed or SessionEnded).
fn drain_pipe(
    pipe: &mut std::fs::File,
    msg_buf: &mut Vec<u8>,
    read_buf: &mut [u8],
    stdout: &mut std::io::Stdout,
) -> Result<bool> {
    loop {
        let available = match pipe_bytes_available(pipe) {
            Some(0) => return Ok(false),
            Some(n) => n,
            None => return Ok(true), // pipe broken — daemon gone
        };
        let to_read = available.min(read_buf.len());
        match pipe.read(&mut read_buf[..to_read]) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                msg_buf.extend_from_slice(&read_buf[..n]);
                loop {
                    match protocol::decode::<DaemonMsg>(msg_buf) {
                        Ok(Some((msg, consumed))) => {
                            msg_buf.drain(..consumed);
                            match msg {
                                DaemonMsg::Output(data) => {
                                    stdout.write_all(&data)?;
                                    stdout.flush()?;
                                }
                                DaemonMsg::ScrollbackReplay(data) => {
                                    // Reset terminal before replaying to avoid
                                    // mixing stale client state with the replay.
                                    stdout.write_all(b"\x1bc")?;
                                    stdout.write_all(&data)?;
                                    stdout.flush()?;
                                }
                                DaemonMsg::SessionEnded => return Ok(true),
                            }
                        }
                        Ok(None) => break,
                        Err(_) => return Ok(true),
                    }
                }
            }
            Err(_) => return Ok(true),
        }
    }
}

/// RAII guard to restore terminal mode on drop.
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

/// Enable ENABLE_VIRTUAL_TERMINAL_PROCESSING on stdout.
#[cfg(windows)]
fn enable_virtual_terminal_processing() {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, SetConsoleMode, ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
    };
    unsafe {
        let handle = std::io::stdout().as_raw_handle();
        let mut mode: u32 = 0;
        if GetConsoleMode(handle as _, &mut mode) != 0 {
            let new_mode = mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
            SetConsoleMode(handle as _, new_mode);
        }
    }
}

#[cfg(not(windows))]
fn enable_virtual_terminal_processing() {}

/// Return the number of bytes available to read from the pipe without blocking.
/// Returns `None` if the pipe is broken (daemon disconnected).
#[cfg(windows)]
fn pipe_bytes_available(pipe: &std::fs::File) -> Option<usize> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;
    let mut available: u32 = 0;
    let ok = unsafe {
        PeekNamedPipe(
            pipe.as_raw_handle() as _,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    };
    if ok != 0 { Some(available as usize) } else { None }
}

#[cfg(not(windows))]
fn pipe_bytes_available(_pipe: &std::fs::File) -> Option<usize> {
    Some(0)
}

/// Convert a crossterm key event to the bytes that should be sent to the PTY.
fn key_event_to_bytes(event: &KeyEvent) -> Option<Vec<u8>> {
    let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
    match event.code {
        KeyCode::Char(c) => {
            if ctrl {
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
    // Retry a few times — the pipe may not exist yet if the daemon just started
    // or is between client sessions (brief window after disconnect before the
    // next pipe instance is created).
    let mut last_err = None;
    for i in 0..20 {
        match OpenOptions::new().read(true).write(true).open(PIPE_NAME) {
            Ok(file) => return Ok(file),
            Err(e) => {
                // ERROR_PIPE_BUSY (231) means the pipe exists but another
                // client is already connected — no point retrying.
                if e.raw_os_error() == Some(231) {
                    anyhow::bail!(
                        "Another client is already attached to this session.\n\
                         Detach the other client first (Ctrl+D), then try again."
                    );
                }
                // ERROR_ACCESS_DENIED (5) means the pipe exists but this
                // session can't open it (e.g. daemon started from SSH,
                // client running from desktop). No point retrying.
                if e.raw_os_error() == Some(5) {
                    anyhow::bail!(
                        "Cannot access the daemon pipe (access denied).\n\
                         The daemon may have been started from a different session.\n\
                         Try: mm kill, then start a new session."
                    );
                }
                last_err = Some(e);
                if i < 19 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
    }
    Err(last_err.unwrap()).context("Could not connect to daemon pipe (timed out after 2s)")
}

#[cfg(not(windows))]
fn connect_to_daemon() -> Result<std::fs::File> {
    anyhow::bail!("Named pipes only supported on Windows");
}
