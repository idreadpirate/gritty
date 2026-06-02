// Wraps alacritty_terminal's VT engine: parse PTY bytes into a grid we can read.

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;

#[derive(Clone, Copy)]
pub struct TermSize {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Terminal {
    pub term: Term<VoidListener>,
    parser: Processor,
    pub size: TermSize,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        let size = TermSize { cols, rows };
        let config = Config {
            scrolling_history: 5000,
            ..Config::default()
        };
        let term = Term::new(config, &size, VoidListener);
        Self {
            term,
            parser: Processor::new(),
            size,
        }
    }

    /// Feed raw PTY output through the VT parser into the grid.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.size = TermSize { cols, rows };
        self.term.resize(self.size);
    }

    /// Lines scrolled up into history (0 = viewing the bottom).
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// True if the running program enabled bracketed paste mode.
    pub fn bracketed_paste(&self) -> bool {
        use alacritty_terminal::term::TermMode;
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }
}

/// Build the byte stream for pasting `text`, normalizing newlines to CR and
/// wrapping in bracketed-paste markers when the program requested them.
pub fn wrap_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let cleaned = text.replace("\r\n", "\r").replace('\n', "\r");
    let mut data = Vec::new();
    if bracketed {
        data.extend_from_slice(b"\x1b[200~");
    }
    data.extend_from_slice(cleaned.as_bytes());
    if bracketed {
        data.extend_from_slice(b"\x1b[201~");
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_paste_plain_normalizes_newlines() {
        assert_eq!(wrap_paste("a\r\nb\nc", false), b"a\rb\rc".to_vec());
    }

    #[test]
    fn wrap_paste_bracketed_wraps() {
        assert_eq!(wrap_paste("x", true), b"\x1b[200~x\x1b[201~".to_vec());
    }

    #[test]
    fn feed_writes_chars_into_grid() {
        let mut t = Terminal::new(80, 24);
        t.feed(b"hello");

        let content = t.term.renderable_content();
        let mut line0 = String::new();
        for item in content.display_iter {
            if item.point.line.0 == 0 {
                line0.push(item.cell.c);
            }
        }
        assert!(line0.starts_with("hello"), "grid line 0 was: {:?}", line0);
    }
}
