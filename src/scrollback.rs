/// Scrollback buffer backed by a VT100 terminal state parser.
///
/// All PTY output is fed through the parser so that on reattach we can
/// replay the current visible screen plus recent scrollback history.
pub struct Scrollback {
    parser: vt100::Parser,
}

impl Scrollback {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, 1000),
        }
    }

    /// Feed raw PTY output into the parser.
    pub fn feed(&mut self, data: &[u8]) {
        self.parser.process(data);
    }

    /// Produce bytes that, when written to a fresh terminal, reproduce
    /// the current screen contents (including scrollback).
    pub fn replay(&self) -> Vec<u8> {
        let screen = self.parser.screen();
        screen.contents_formatted()
    }

    /// Resize the virtual terminal tracked by the parser.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.set_size(rows, cols);
    }
}
