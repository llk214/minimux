use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::protocol::{self, ClientMsg, DaemonMsg, PIPE_NAME};
use crate::scrollback::Scrollback;

/// State shared between the PTY reader thread and client handler threads.
struct SharedState {
    scrollback: Scrollback,
    /// Current client pipe writer, if any client is attached.
    client_writer: Option<PipeWriter>,
    /// Whether the shell has exited.
    shell_exited: bool,
}

/// A thin wrapper around a named pipe handle for writing.
struct PipeWriter {
    handle: std::fs::File,
}

impl PipeWriter {
    fn send(&mut self, msg: &DaemonMsg) -> Result<()> {
        let frame = protocol::encode(msg)?;
        std::io::Write::write_all(&mut self.handle, &frame)?;
        Ok(())
    }
}

/// Run the daemon: create the PTY, start the shell, listen for clients.
pub fn run_daemon(shell: &str, cols: u16, rows: u16) -> Result<()> {
    // Write PID file.
    let pid_dir = get_state_dir()?;
    std::fs::create_dir_all(&pid_dir)?;
    let pid_path = pid_dir.join("daemon.pid");
    std::fs::write(&pid_path, std::process::id().to_string())?;

    // Create PTY.
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("Failed to open PTY")?;

    // Spawn shell.
    let cmd = CommandBuilder::new(shell);
    let _child = pair
        .slave
        .spawn_command(cmd)
        .context("Failed to spawn shell")?;

    // We must drop the slave after spawning, otherwise reads on the master
    // will block forever on Windows.
    drop(pair.slave);

    let mut pty_writer = pair.master.take_writer()?;
    let mut pty_reader = pair.master.try_clone_reader()?;

    let state = Arc::new(Mutex::new(SharedState {
        scrollback: Scrollback::new(rows, cols),
        client_writer: None,
        shell_exited: false,
    }));

    // We need the master kept alive so the PTY stays open, but we also need
    // to be able to resize it. Wrap it in an Arc<Mutex<>>.
    let pty_master = Arc::new(Mutex::new(pair.master));

    // Thread: read PTY output → scrollback + client.
    let state_pty = Arc::clone(&state);
    let pty_read_thread = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match pty_reader.read(&mut buf) {
                Ok(0) => {
                    // PTY closed — shell exited.
                    let mut s = state_pty.lock().unwrap();
                    s.shell_exited = true;
                    if let Some(ref mut w) = s.client_writer {
                        let _ = w.send(&DaemonMsg::SessionEnded);
                    }
                    break;
                }
                Ok(n) => {
                    let data = &buf[..n];
                    let mut s = state_pty.lock().unwrap();
                    s.scrollback.feed(data);
                    if let Some(ref mut w) = s.client_writer {
                        if w.send(&DaemonMsg::Output(data.to_vec())).is_err() {
                            // Client disconnected.
                            s.client_writer = None;
                        }
                    }
                }
                Err(_) => {
                    let mut s = state_pty.lock().unwrap();
                    s.shell_exited = true;
                    break;
                }
            }
        }
    });

    // Main loop: accept client connections on the named pipe.
    loop {
        {
            let s = state.lock().unwrap();
            if s.shell_exited {
                break;
            }
        }

        // Create named pipe instance and wait for a client.
        let pipe = create_pipe_instance()?;
        // Wait for client to connect (blocking).
        let client_file = wait_for_client(pipe)?;

        // Set this client as the active writer.
        let writer_file = client_file.try_clone()?;
        {
            let mut s = state.lock().unwrap();

            // Send scrollback replay.
            let mut writer = PipeWriter {
                handle: writer_file,
            };
            let replay = s.scrollback.replay();
            if !replay.is_empty() {
                let _ = writer.send(&DaemonMsg::ScrollbackReplay(replay));
            }
            s.client_writer = Some(writer);
        }

        // Read from this client until it disconnects.
        let mut reader = std::io::BufReader::new(client_file);
        let mut read_buf = vec![0u8; 8192];
        let mut msg_buf = Vec::new();

        loop {
            let s = state.lock().unwrap();
            if s.shell_exited {
                break;
            }
            drop(s);

            match reader.read(&mut read_buf) {
                Ok(0) => {
                    // Client disconnected.
                    let mut s = state.lock().unwrap();
                    s.client_writer = None;
                    break;
                }
                Ok(n) => {
                    msg_buf.extend_from_slice(&read_buf[..n]);
                    // Process all complete messages in the buffer.
                    loop {
                        match protocol::decode::<ClientMsg>(&msg_buf) {
                            Ok(Some((msg, consumed))) => {
                                msg_buf.drain(..consumed);
                                match msg {
                                    ClientMsg::Input(data) => {
                                        let _ = pty_writer.write_all(&data);
                                    }
                                    ClientMsg::Resize { cols, rows } => {
                                        let mut s = state.lock().unwrap();
                                        s.scrollback.resize(rows, cols);
                                        if let Ok(master) = pty_master.lock() {
                                            let _ = master.resize(PtySize {
                                                rows,
                                                cols,
                                                pixel_width: 0,
                                                pixel_height: 0,
                                            });
                                        }
                                    }
                                    ClientMsg::Detach => {
                                        let mut s = state.lock().unwrap();
                                        s.client_writer = None;
                                        break;
                                    }
                                }
                            }
                            Ok(None) => break, // Need more data.
                            Err(_) => {
                                // Corrupt message — drop client.
                                let mut s = state.lock().unwrap();
                                s.client_writer = None;
                                break;
                            }
                        }
                    }
                }
                Err(_) => {
                    let mut s = state.lock().unwrap();
                    s.client_writer = None;
                    break;
                }
            }
        }
    }

    // Clean up PID file.
    let _ = std::fs::remove_file(&pid_path);
    pty_read_thread.join().ok();

    Ok(())
}

/// Check if a daemon is currently running by reading the PID file and checking
/// whether the process is alive.
pub fn is_daemon_running() -> Result<Option<u32>> {
    let pid_path = get_state_dir()?.join("daemon.pid");
    if !pid_path.exists() {
        return Ok(None);
    }
    let pid_str = std::fs::read_to_string(&pid_path)?;
    let pid: u32 = pid_str.trim().parse()?;

    if is_process_alive(pid) {
        Ok(Some(pid))
    } else {
        // Stale PID file — clean up.
        let _ = std::fs::remove_file(&pid_path);
        Ok(None)
    }
}

/// Kill the daemon process.
pub fn kill_daemon() -> Result<()> {
    if let Some(pid) = is_daemon_running()? {
        kill_process(pid)?;
        let pid_path = get_state_dir()?.join("daemon.pid");
        let _ = std::fs::remove_file(&pid_path);
        println!("Daemon (PID {}) killed.", pid);
    } else {
        println!("No daemon running.");
    }
    Ok(())
}

/// Print daemon status.
pub fn print_status() -> Result<()> {
    match is_daemon_running()? {
        Some(pid) => {
            println!("Session: running");
            println!("PID:     {}", pid);
        }
        None => {
            println!("Session: not running");
        }
    }
    Ok(())
}

/// Start the daemon as a detached background process, then return.
pub fn start_daemon_background(shell: &str, scrollback: usize) -> Result<()> {
    let exe = std::env::current_exe()?;
    let (cols, rows) = crossterm::terminal::size().unwrap_or((120, 30));

    // Use Windows API to spawn a detached process.
    let mut cmd = std::process::Command::new(exe);
    cmd.args([
        "--daemon-mode",
        "--shell",
        shell,
        "--scrollback",
        &scrollback.to_string(),
        "--cols",
        &cols.to_string(),
        "--rows",
        &rows.to_string(),
    ]);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        cmd.creation_flags(0x00000008 | 0x00000200);
    }

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Failed to start daemon process")?;

    // Give the daemon a moment to start and create the pipe.
    thread::sleep(Duration::from_millis(500));
    Ok(())
}

// --- Platform-specific helpers ---

fn get_state_dir() -> Result<std::path::PathBuf> {
    let dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("minimux");
    Ok(dir)
}

#[cfg(windows)]
fn is_process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        CloseHandle(handle);
        true
    }
}

#[cfg(not(windows))]
fn is_process_alive(pid: u32) -> bool {
    // Fallback for non-Windows (shouldn't be used).
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn kill_process(pid: u32) -> Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            anyhow::bail!("Failed to open process {}", pid);
        }
        TerminateProcess(handle, 1);
        CloseHandle(handle);
    }
    Ok(())
}

#[cfg(not(windows))]
fn kill_process(pid: u32) -> Result<()> {
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    Ok(())
}

// --- Named pipe helpers ---

#[cfg(windows)]
fn create_pipe_instance() -> Result<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::System::Pipes::CreateNamedPipeW;

    let pipe_name = to_wide(PIPE_NAME);

    unsafe {
        let handle = CreateNamedPipeW(
            pipe_name.as_ptr(),
            0x00000003, // PIPE_ACCESS_DUPLEX
            0x00000000, // PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT
            1,          // max instances
            8192,       // out buffer
            8192,       // in buffer
            0,          // default timeout
            std::ptr::null(),
        );
        if handle.is_null() {
            anyhow::bail!("Failed to create named pipe");
        }
        Ok(std::fs::File::from_raw_handle(handle as _))
    }
}

#[cfg(windows)]
fn wait_for_client(pipe: std::fs::File) -> Result<std::fs::File> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Pipes::ConnectNamedPipe;

    unsafe {
        let handle = pipe.as_raw_handle();
        let _result = ConnectNamedPipe(handle, std::ptr::null_mut());
        // ConnectNamedPipe returns 0 on success when client is already connected
        // (ERROR_PIPE_CONNECTED) — either way, the pipe is usable.
    }
    Ok(pipe)
}

#[cfg(windows)]
fn to_wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(not(windows))]
fn create_pipe_instance() -> Result<std::fs::File> {
    anyhow::bail!("Named pipes only supported on Windows");
}

#[cfg(not(windows))]
fn wait_for_client(_pipe: std::fs::File) -> Result<std::fs::File> {
    anyhow::bail!("Named pipes only supported on Windows");
}
