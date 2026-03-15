# minimux test plan

Manual testing checklist. Run on a real Windows machine (or via SSH).

## Setup
```
# Add mm.exe to PATH or use full path
set PATH=%PATH%;C:\Users\LLK\minimux\target\x86_64-pc-windows-msvc\release

# Clean state before testing
mm kill
```

## 1. Basic lifecycle

- [ ] `mm` — starts new session, shows shell prompt
- [ ] Type commands (`dir`, `echo hello`) — output appears immediately
- [ ] `Ctrl+D` — detaches cleanly, returns to outer shell
- [ ] `mm` — reattaches, scrollback shows previous commands and output
- [ ] `mm status` — shows "running" with PID
- [ ] `mm kill` — kills daemon, confirms
- [ ] `mm status` — shows "not running"

## 2. Shell variants

- [ ] `mm --shell pwsh` — PowerShell 7
- [ ] `mm --shell powershell` — PowerShell 5.1
- [ ] `mm --shell cmd` — Command Prompt
- [ ] Each shell: type a command, detach, reattach, verify scrollback

## 3. Abrupt disconnect

- [ ] Start `mm`, close the terminal window (click X)
- [ ] Open new terminal, `mm` — should reattach with scrollback
- [ ] Repeat 3 times in a row — should work every time

## 4. Rapid detach/reattach

- [ ] Start `mm`
- [ ] `Ctrl+D`, `mm`, `Ctrl+D`, `mm` — repeat 10 times quickly
- [ ] Session should remain stable, no errors

## 5. Large output

- [ ] Run `git log` on a large repo (1000+ commits)
- [ ] Run `type` or `cat` on a large file (1MB+)
- [ ] Detach during large output, reattach — should not hang or corrupt

## 6. Interactive / TUI apps

- [ ] `python` REPL — interactive input/output
- [ ] `node` REPL — same
- [ ] `vim` or `nvim` (if installed) — cursor movement, editing, saving
- [ ] `less` (if available) — scrolling, search
- [ ] `ssh` to another machine from inside mm — nested terminal

## 7. SSH into the test machine

- [ ] SSH into Windows machine from another computer
- [ ] Run `mm` over SSH — start session
- [ ] `Ctrl+D` to detach
- [ ] Disconnect SSH entirely (close SSH window)
- [ ] SSH back in, run `mm` — reattach to the session
- [ ] Verify scrollback and continued operation

## 8. Long-running processes

- [ ] Start `mm`, run `ping -t localhost` (continuous ping)
- [ ] Detach, wait 5 minutes, reattach — pings should have continued
- [ ] Start a long build/compile, detach during it, reattach — should complete

## 9. Resize

- [ ] Start `mm`, resize the terminal window by dragging edges
- [ ] Output should reflow correctly
- [ ] Detach, resize the outer terminal, reattach — session adapts to new size

## 10. Sleep/wake

- [ ] Start `mm`, run a command
- [ ] Close laptop lid / sleep the machine
- [ ] Wake up, reattach — session should still be alive

## 11. Unicode and special characters

- [ ] Type CJK characters: `echo 你好世界`
- [ ] Emoji: `echo 🚀`
- [ ] Check output renders correctly after detach/reattach

## 12. Concurrent clients

- [ ] Start `mm` in terminal A
- [ ] Open terminal B, run `mm` — what happens?
- [ ] Expected: second client takes over (or gets rejected cleanly)

## 13. Edge cases

- [ ] `mm kill` when no session running — should print "No daemon running."
- [ ] `mm` after `mm kill` — should start fresh session
- [ ] Kill daemon from Task Manager — `mm` should start a new one
- [ ] Run `exit` in the shell inside mm — session should end cleanly

## Results

Date: ___________
Tester: ___________

| Test | Pass/Fail | Notes |
|------|-----------|-------|
| 1. Basic lifecycle | | |
| 2. Shell variants | | |
| 3. Abrupt disconnect | | |
| 4. Rapid detach/reattach | | |
| 5. Large output | | |
| 6. Interactive/TUI apps | | |
| 7. SSH | | |
| 8. Long-running processes | | |
| 9. Resize | | |
| 10. Sleep/wake | | |
| 11. Unicode | | |
| 12. Concurrent clients | | |
| 13. Edge cases | | |
