mod client;
mod daemon;
mod protocol;
mod scrollback;

use clap::{Parser, Subcommand};

/// Find the best available shell on this system.
fn default_shell() -> String {
    // Prefer pwsh (PowerShell 7+), fall back to powershell (5.1), then cmd.
    for candidate in ["pwsh", "powershell", "cmd"] {
        if which(candidate).is_some() {
            return candidate.to_string();
        }
    }
    "cmd".to_string()
}

/// Check if a command exists on PATH.
fn which(name: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT".to_string());
    let extensions: Vec<&str> = exts.split(';').collect();
    for dir in std::env::split_paths(&path_var) {
        // Try exact name first.
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        // Try with each extension.
        for ext in &extensions {
            let with_ext = dir.join(format!("{}{}", name, ext));
            if with_ext.is_file() {
                return Some(with_ext);
            }
        }
    }
    None
}

#[derive(Parser)]
#[command(name = "mm", about = "Minimal terminal session persistence for Windows")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Shell to launch (default: auto-detect pwsh > powershell > cmd).
    #[arg(long, default_value_t = default_shell())]
    shell: String,

    /// Scrollback buffer size in lines.
    #[arg(long, default_value_t = 1000)]
    scrollback: usize,

    /// Internal: run as the daemon process (not for direct use).
    #[arg(long, hide = true)]
    daemon_mode: bool,

    /// Internal: terminal columns (used by daemon-mode).
    #[arg(long, hide = true, default_value_t = 120)]
    cols: u16,

    /// Internal: terminal rows (used by daemon-mode).
    #[arg(long, hide = true, default_value_t = 30)]
    rows: u16,
}

#[derive(Subcommand)]
enum Commands {
    /// Show session status.
    Status,
    /// Kill the running session.
    Kill,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Internal daemon mode: run the daemon loop directly.
    if cli.daemon_mode {
        return daemon::run_daemon(&cli.shell, cli.cols, cli.rows);
    }

    match cli.command {
        Some(Commands::Status) => {
            daemon::print_status()?;
        }
        Some(Commands::Kill) => {
            daemon::kill_daemon()?;
        }
        None => {
            // Default: attach to existing session, or create one.
            if daemon::is_daemon_running()?.is_none() {
                println!("Starting new session...");
                daemon::start_daemon_background(&cli.shell, cli.scrollback)?;
            }
            client::attach()?;
        }
    }

    Ok(())
}
