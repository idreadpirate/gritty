// Wraps alacritty_terminal's VT engine: parse PTY bytes into a grid we can read.

use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
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

/// Captures OSC 0/2 window title events emitted by the VT parser.
///
/// `EventListener::send_event` takes `&self`, so interior mutability
/// (Arc<Mutex>) is required to update the title without `&mut self`.
/// The same `Arc` is held by `Terminal` so callers can read the current title.
#[derive(Clone)]
pub(crate) struct TitleListener {
    title: Arc<Mutex<String>>,
}

impl TitleListener {
    fn new(title: Arc<Mutex<String>>) -> Self {
        Self { title }
    }
}

impl EventListener for TitleListener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(new_title) => {
                if let Ok(mut guard) = self.title.lock() {
                    *guard = new_title;
                }
            }
            Event::ResetTitle => {
                if let Ok(mut guard) = self.title.lock() {
                    guard.clear();
                }
            }
            _ => {}
        }
    }
}

pub struct Terminal {
    pub term: Term<TitleListener>,
    parser: Processor,
    pub size: TermSize,
    // Shared with TitleListener; updated on every OSC 0/2 event.
    // `#[allow(dead_code)]` silences the lint for the binary crate where
    // `title()` may not be wired up yet.
    #[allow(dead_code)]
    title: Arc<Mutex<String>>,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        let size = TermSize { cols, rows };
        let config = Config {
            scrolling_history: 5000,
            ..Config::default()
        };
        let title = Arc::new(Mutex::new(String::new()));
        let listener = TitleListener::new(Arc::clone(&title));
        let term = Term::new(config, &size, listener);
        Self {
            term,
            parser: Processor::new(),
            size,
            title,
        }
    }

    /// Returns the latest window title set via OSC 0/2, or an empty string if
    /// none has been set (or after a `ResetTitle` event).
    // `dead_code` suppressed: callers will wire this up in app.rs/main.rs;
    // the method is `pub` but binary-crate reachability analysis still flags it
    // when no call site exists yet.
    #[allow(dead_code)]
    pub fn title(&self) -> String {
        self.title.lock().map(|g| g.clone()).unwrap_or_default()
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

    /// Scroll the viewport by `lines` (positive = up into history).
    pub fn scroll(&mut self, lines: i32) {
        use alacritty_terminal::grid::Scroll;
        self.term.scroll_display(Scroll::Delta(lines));
    }

    /// Jump back to the live bottom of the output.
    pub fn scroll_to_bottom(&mut self) {
        use alacritty_terminal::grid::Scroll;
        self.term.scroll_display(Scroll::Bottom);
    }

    /// True if the running program enabled bracketed paste mode.
    pub fn bracketed_paste(&self) -> bool {
        use alacritty_terminal::term::TermMode;
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }
}

/// Build the byte stream for pasting `text`.
///
/// Security (RT-4, RT-5b): clipboard content is untrusted — it may carry escape
/// sequences (OSC/title/CSI injection) or an embedded bracketed-paste end marker
/// that would terminate the paste and run the rest as live input. We normalize
/// newlines to CR and strip C0/C1 control bytes except TAB and the normalized CR.
/// Dropping ESC (0x1B) neutralizes all escape injection, including any embedded
/// `\x1b[201~`, so the markers we add ourselves are the only ones present.
pub fn wrap_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let mut cleaned = String::with_capacity(text.len());
    let mut prev_cr = false;
    for ch in text.chars() {
        match ch {
            '\r' => {
                cleaned.push('\r');
                prev_cr = true;
                continue;
            }
            '\n' => {
                if !prev_cr {
                    cleaned.push('\r'); // CRLF -> single CR, lone LF -> CR
                }
            }
            '\t' => cleaned.push('\t'),
            c if (c as u32) < 0x20 => {} // drop other C0 (incl. ESC)
            c if ('\u{80}'..='\u{9f}').contains(&c) => {} // drop C1
            c => cleaned.push(c),
        }
        prev_cr = false;
    }

    let mut data = Vec::with_capacity(cleaned.len() + 12);
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
    fn wrap_paste_strips_control_and_escapes() {
        // ESC and BEL dropped; printable SGR text kept; tab preserved.
        assert_eq!(
            wrap_paste("a\x1b[31mb\x07\tc", false),
            b"a[31mb\tc".to_vec()
        );
    }

    #[test]
    fn wrap_paste_neutralizes_embedded_end_marker() {
        // An embedded end marker must not appear in the payload (its ESC is gone),
        // so the only markers are the wrappers we add.
        let out = wrap_paste("x\x1b[201~y", true);
        assert_eq!(out, b"\x1b[200~x[201~y\x1b[201~".to_vec());
        // exactly one real end marker (the trailing wrapper)
        let needle = b"\x1b[201~";
        let count = out.windows(needle.len()).filter(|w| *w == needle).count();
        assert_eq!(count, 1);
    }

    #[test]
    fn scroll_up_then_bottom() {
        let mut t = Terminal::new(20, 5);
        for _ in 0..50 {
            t.feed(b"line\r\n");
        }
        t.scroll(3);
        assert!(
            t.display_offset() > 0,
            "scrolling up should leave the bottom"
        );
        t.scroll_to_bottom();
        assert_eq!(
            t.display_offset(),
            0,
            "scroll_to_bottom should return to live view"
        );
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

    #[test]
    fn resize_updates_dimensions() {
        let mut t = Terminal::new(80, 24);
        t.resize(120, 40);
        assert_eq!(t.size.cols, 120);
        assert_eq!(t.size.rows, 40);
        // the underlying grid reports the new geometry too
        assert_eq!(t.term.grid().columns(), 120);
        assert_eq!(t.term.grid().screen_lines(), 40);
    }

    #[test]
    fn bracketed_paste_reflects_terminal_mode() {
        let mut t = Terminal::new(80, 24);
        assert!(!t.bracketed_paste(), "off by default");
        // DECSET 2004 enables bracketed paste mode.
        t.feed(b"\x1b[?2004h");
        assert!(t.bracketed_paste(), "should be on after enabling");
        // DECRST 2004 disables it again.
        t.feed(b"\x1b[?2004l");
        assert!(!t.bracketed_paste(), "should be off after disabling");
    }

    #[test]
    fn term_size_reports_dimensions() {
        let s = TermSize { cols: 7, rows: 3 };
        assert_eq!(s.columns(), 7);
        assert_eq!(s.screen_lines(), 3);
        assert_eq!(s.total_lines(), 3);
    }

    #[test]
    fn osc_title_captured_via_osc0() {
        let mut t = Terminal::new(80, 24);
        // OSC 0 ; <title> ST — sets icon name *and* window title.
        t.feed(b"\x1b]0;hello\x07");
        assert_eq!(t.title(), "hello", "title should reflect OSC 0 payload");
    }

    #[test]
    fn osc_title_updated_and_reset() {
        let mut t = Terminal::new(80, 24);
        t.feed(b"\x1b]2;world\x07");
        assert_eq!(t.title(), "world");
        // OSC 104 (reset colors) → ResetTitle not triggered; OSC l (xterm iconName) - skip.
        // Drive a second OSC 0 to update:
        t.feed(b"\x1b]0;updated\x07");
        assert_eq!(t.title(), "updated");
    }
}
