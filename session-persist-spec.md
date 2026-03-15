# Session Persist — Minimal Windows Terminal Session Persistence

## Overview

A lightweight, single-purpose terminal session persistence tool for Windows. Unlike psmux/tmux, this tool does **not** aim for terminal multiplexing, pane splitting, or tmux compatibility. It solves exactly one problem: **keeping a terminal session alive across SSH disconnects on native Windows, without WSL.**

## Motivation

- SSHing into a Windows desktop (via Tailscale + OpenSSH) from iPad/laptop
- Running long-lived processes like Claude Code, compilations, training jobs
- Disconnecting (closing Termius, losing WiFi, switching networks) without killing the process
- Reconnecting later and picking up exactly where you left off
- No WSL, no Cygwin — native Windows

## Target Platform

- Windows 10/11
- PowerShell 7+ as the default shell
- Accessed via SSH (OpenSSH Server on Windows)

## Core Architecture

```
┌─────────────┐          named pipe / socket          ┌──────────────┐
│   Client     │  ◄──────────────────────────────────►  │    Daemon     │
│  (foreground)│    stdin/stdout relay + control msgs   │  (background) │
└─────────────┘                                        │              │
                                                       │  ┌────────┐  │
                                                       │  │  PTY   │  │
                                                       │  │(shell) │  │
                                                       │  └────────┘  │
                                                       │              │
                                                       │  scrollback  │
                                                       │   buffer     │
                                                       └──────────────┘
```

- **Daemon**: Background process that owns the PTY and keeps the shell alive. Stores a scrollback buffer so the client can replay terminal state on reattach.
- **Client**: Foreground process that connects to the daemon. Relays keystrokes → daemon → PTY, and PTY output → daemon → client terminal. On disconnect, daemon continues running.

## User Stories

### US-1: Start a persistent session
**As** a user SSHing into my Windows desktop,
**I want** to run `sp` (or `session-persist`) and get a shell,
**So that** anything I run inside it survives SSH disconnects.

**Acceptance criteria:**
- Running `sp` starts a daemon (if not already running) and attaches
- The shell inside is PowerShell 7 by default
- If a session already exists, it reattaches instead of creating a new one

### US-2: Disconnect without losing work
**As** a user who closes Termius or loses network,
**I want** the running process inside the session to keep running,
**So that** I don't lose a Claude Code conversation or a long compilation.

**Acceptance criteria:**
- Closing the SSH connection does not kill the daemon or the PTY
- The process inside the PTY continues executing
- Daemon remains alive and accessible for reattach

### US-3: Reattach and see previous output
**As** a user reconnecting via SSH,
**I want** to reattach and see the recent terminal output,
**So that** I know what happened while I was away.

**Acceptance criteria:**
- Running `sp` again reattaches to the existing session
- The last N lines of terminal output are replayed (scrollback buffer)
- The terminal is interactive and responsive immediately after reattach

### US-4: Auto-attach on SSH login
**As** a user who doesn't want to type `sp` every time,
**I want** my PowerShell profile to auto-attach on SSH,
**So that** every SSH session is automatically persistent.

**Acceptance criteria:**
- Adding a snippet to `$PROFILE` that checks `$env:SSH_CONNECTION`
- Automatically runs `sp` on SSH login
- Does not trigger when opening a local terminal

### US-5: Kill a session
**As** a user who wants to cleanly end a session,
**I want** to run `sp kill` to stop the daemon,
**So that** I can start fresh or free resources.

**Acceptance criteria:**
- `sp kill` terminates the daemon and the PTY
- `sp kill` works even when not attached
- Running `sp` after kill starts a new session

### US-6: List session status
**As** a user who wants to check if a session is running,
**I want** to run `sp status` to see session info,
**So that** I know whether to attach or start fresh.

**Acceptance criteria:**
- Shows: session running (yes/no), PID, uptime, shell command
- Works without attaching

## Non-Goals (Explicit Exclusions)

- ❌ Multiple sessions / named sessions (v1 supports exactly one session)
- ❌ Pane splitting
- ❌ Window/tab management
- ❌ tmux command compatibility
- ❌ Config file / theming / status bar
- ❌ Mouse support
- ❌ Copy mode / vim keybindings
- ❌ Cross-platform support (Windows only)
- ❌ Plugin system

## Technical Requirements

### Language & Dependencies

- **Language**: Rust
- **Key crates**:
  - `portable-pty` — PTY creation and management on Windows
  - `crossterm` — raw terminal mode, input handling
  - `vt100` — terminal state parsing for scrollback replay
  - `windows-sys` or `tokio` — named pipe IPC on Windows
  - `clap` — CLI argument parsing

### CLI Interface

```
sp                  # Attach to existing session, or create + attach
sp status           # Show session info
sp kill             # Kill the daemon and session
sp --shell <path>   # Specify shell (default: pwsh)
sp --scrollback <n> # Set scrollback buffer size (default: 1000 lines)
```

### IPC Mechanism

- Windows Named Pipes (`\\.\pipe\session-persist`)
- Protocol: simple framed messages
  - Client → Daemon: `Input(bytes)`, `Resize(cols, rows)`, `Detach`
  - Daemon → Client: `Output(bytes)`, `SessionEnded`

### Scrollback Buffer

- Daemon maintains a VT100 parsed terminal state
- On reattach, daemon sends the current screen contents + recent scrollback
- Default: 1000 lines of history

### Daemon Lifecycle

- Daemon starts as a background process (detached from the client)
- PID stored in a known location (`%APPDATA%\session-persist\daemon.pid`)
- Daemon exits when: shell exits, `sp kill` is called, or system shuts down
- Daemon should handle multiple sequential client connections (attach, detach, reattach)

## Estimated Complexity

| Component | Estimated LOC | Difficulty |
|---|---|---|
| CLI parsing + entry point | ~100 | Easy |
| Daemon process management | ~200 | Medium |
| Named pipe IPC server/client | ~300 | Medium-Hard |
| PTY creation + I/O relay | ~200 | Medium |
| Scrollback buffer + VT100 replay | ~200 | Medium |
| Terminal raw mode + resize handling | ~100 | Easy-Medium |
| **Total** | **~1000-1200** | |

## Future Extensions (v2, Maybe)

- Named sessions (`sp -s work`, `sp -s claude`)
- Multiple concurrent sessions
- Session timeout / auto-kill after N hours idle
- Scrollback search
- Simple status line showing session name + uptime

## References

- [psmux](https://github.com/marlocarlo/psmux) — Full tmux alternative for Windows, inspiration for this project
- [portable-pty docs](https://docs.rs/portable-pty/latest) — PTY abstraction crate
- [tmux architecture](https://github.com/tmux/tmux) — Reference for session persistence design
- Windows Named Pipes — IPC mechanism for daemon ↔ client communication
