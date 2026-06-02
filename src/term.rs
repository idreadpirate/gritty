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
}

#[cfg(test)]
mod tests {
    use super::*;

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
