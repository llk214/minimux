use std::collections::VecDeque;

/// Scrollback buffer backed by a vt100 terminal parser.
///
/// The live parser tracks the current terminal state (never mutated during
/// replay). A raw byte ring buffer is kept alongside so that a temporary
/// parser can reconstruct scrollback history on reattach.
pub struct Scrollback {
    /// Live parser — single source of truth for the current screen.
    parser: vt100::Parser,
    /// Raw PTY output ring buffer for scrollback reconstruction.
    raw: VecDeque<u8>,
    /// Maximum raw bytes to retain.
    capacity: usize,
}

const DEFAULT_RAW_CAPACITY: usize = 1024 * 1024; // 1 MiB
const SCROLLBACK_LINES: usize = 10_000;

impl Scrollback {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, 0),
            raw: VecDeque::with_capacity(DEFAULT_RAW_CAPACITY),
            capacity: DEFAULT_RAW_CAPACITY,
        }
    }

    /// Feed raw PTY output into the live parser and raw buffer.
    pub fn feed(&mut self, data: &[u8]) {
        // vt100 can panic on wide characters after a resize (upstream bug).
        // Catch the panic and recreate the parser to keep the daemon alive.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.parser.process(data);
        }));
        if result.is_err() {
            let (rows, cols) = self.parser.screen().size();
            self.parser = vt100::Parser::new(rows, cols, 0);
            // Re-feed recent raw data to rebuild state.
            let raw_copy: Vec<u8> = self.raw.iter().copied().collect();
            self.parser.process(&raw_copy);
        }

        // Append to raw buffer, evicting old data if needed.
        let overflow = (self.raw.len() + data.len()).saturating_sub(self.capacity);
        if overflow > 0 {
            self.raw.drain(..overflow.min(self.raw.len()));
        }
        self.raw.extend(data);
    }

    /// Produce bytes that, when written to a fresh terminal, reproduce
    /// the current screen and scrollback history.
    ///
    /// The live parser is NEVER mutated — only its current screen state
    /// is read. A temporary parser reconstructs scrollback from the raw
    /// buffer.
    pub fn replay(&self) -> Vec<u8> {
        let (rows, cols) = self.parser.screen().size();
        let mut output = Vec::new();

        // Build a temporary parser from the raw buffer for scrollback.
        let mut tmp = vt100::Parser::new(rows, cols, SCROLLBACK_LINES);
        let raw_bytes: Vec<u8> = self.raw.iter().copied().collect();
        tmp.process(&raw_bytes);

        if self.parser.screen().alternate_screen() {
            // TUI app (e.g. Claude Code) using alternate screen.
            //
            // Strategy:
            //   1. Extract main-screen scrollback from the temp parser
            //   2. Emit the main-screen content (shell state before TUI launched)
            //   3. Switch client to alternate screen
            //   4. Draw the current alt-screen content from the live parser

            if tmp.screen().alternate_screen() {
                // Exit alt screen on the TEMP parser to access main screen.
                tmp.process(b"\x1b[?1049l");
            }
            emit_scrollback(&mut tmp, cols, &mut output);
            // Main screen visible content.
            output.extend(b"\x1b[H\x1b[2J");
            output.extend(tmp.screen().state_formatted());

            // Switch to alt screen and draw from the live parser (accurate).
            output.extend(b"\x1b[?1049h");
            output.extend(self.parser.screen().state_formatted());
        } else {
            // Normal shell — scrollback from temp parser, screen from live.
            emit_scrollback(&mut tmp, cols, &mut output);
            output.extend(b"\x1b[H\x1b[2J");
            output.extend(self.parser.screen().state_formatted());
        }

        output
    }

    /// Resize the virtual terminal tracked by the parser.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
    }
}

/// Extract scrollback rows from a parser and append them to `output`.
/// The parser must NOT be in alternate screen mode.
fn emit_scrollback(parser: &mut vt100::Parser, cols: u16, output: &mut Vec<u8>) {
    let (rows, _) = parser.screen().size();
    let h = rows as usize;

    // Find total scrollback depth by clamping to max.
    parser.screen_mut().set_scrollback(usize::MAX);
    let total_sb = parser.screen().scrollback();

    if total_sb == 0 {
        return;
    }

    // Page through scrollback from oldest to newest.
    let mut offset = total_sb;
    while offset > 0 {
        parser.screen_mut().set_scrollback(offset);
        let sb_rows_in_view = offset.min(h);

        for row_bytes in parser.screen().rows_formatted(0, cols).take(sb_rows_in_view) {
            output.extend(&row_bytes);
            output.extend(b"\r\n");
        }

        offset = offset.saturating_sub(h);
    }

    parser.screen_mut().set_scrollback(0);
}
