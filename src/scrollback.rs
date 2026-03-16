use std::collections::VecDeque;

/// Scrollback buffer that stores raw PTY output for replay on reattach.
///
/// A ring buffer keeps the last `capacity` bytes of raw output. On reattach
/// these bytes are replayed directly, letting the client terminal rebuild
/// the full screen state including scrollback history.
///
/// A vt100 parser is also maintained for the current visible screen, which
/// can be used for future features (e.g. pane rendering).
pub struct Scrollback {
    parser: vt100::Parser,
    /// Raw PTY output ring buffer for replay.
    raw: VecDeque<u8>,
    /// Maximum bytes to retain.
    capacity: usize,
}

const DEFAULT_RAW_CAPACITY: usize = 1024 * 1024; // 1 MiB

impl Scrollback {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, 0),
            raw: VecDeque::with_capacity(DEFAULT_RAW_CAPACITY),
            capacity: DEFAULT_RAW_CAPACITY,
        }
    }

    /// Feed raw PTY output into the parser and raw buffer.
    pub fn feed(&mut self, data: &[u8]) {
        self.parser.process(data);

        // Append to raw buffer, evicting old data if needed.
        let overflow = (self.raw.len() + data.len()).saturating_sub(self.capacity);
        if overflow > 0 {
            self.raw.drain(..overflow.min(self.raw.len()));
        }
        self.raw.extend(data);
    }

    /// Produce bytes that, when written to a fresh terminal, reproduce
    /// the full screen contents including scrollback history.
    pub fn replay(&self) -> Vec<u8> {
        self.raw.iter().copied().collect()
    }

    /// Resize the virtual terminal tracked by the parser.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
    }
}
