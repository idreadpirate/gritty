// Wraps alacritty_terminal's VT engine: parse PTY bytes into a grid we can read.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;

/// Percent-decode a URI path component.  Only `%XX` sequences are decoded;
/// everything else is passed through unchanged.  Invalid sequences (non-hex
/// digits, truncated) are left as-is rather than producing replacement chars.
///
/// This is a pure, dependency-free helper — the only external encoding used
/// in OSC 7 URIs is `%XX` for non-ASCII/reserved bytes.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = bytes[i + 1];
            let lo = bytes[i + 2];
            if let (Some(h), Some(l)) = (hex_digit(hi), hex_digit(lo)) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // The path is guaranteed to be UTF-8 on shells that emit OSC 7 correctly.
    // Fall back to lossy conversion so malformed bytes never panic.
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parse an OSC 7 `file://host/path` URI into an OS path string.
///
/// The format is `file://[host]/path`; on Windows the path begins with a
/// leading `/` then the drive letter, e.g. `/C:/Users/…`.  We strip that
/// leading `/` and convert forward slashes to backslashes so the result is
/// usable directly as a `cwd` argument.
///
/// Returns `None` if the string does not start with `file://`.
fn osc7_uri_to_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    // Skip the (possibly empty) host component up to the first `/`.
    let slash = rest.find('/')?; // no '/' → no path at all
    let path_part = &rest[slash..]; // includes the leading '/'
    let decoded = percent_decode(path_part);

    // On Windows the URI path looks like `/C:/Users/foo`.
    // Strip the leading slash before the drive letter.
    #[cfg(windows)]
    let decoded = {
        // Strip leading '/' only when followed by a drive letter + ':'
        // (e.g. "/C:/…" → "C:\…").
        if decoded.len() >= 3
            && decoded.starts_with('/')
            && decoded.as_bytes()[1].is_ascii_alphabetic()
            && decoded.as_bytes()[2] == b':'
        {
            decoded[1..].replace('/', "\\")
        } else {
            decoded
        }
    };

    #[cfg(not(windows))]
    let decoded = decoded;

    Some(decoded)
}

/// Default lines of scrollback kept per pane when no config override applies.
/// Mirrors `config::Config::default().scrollback`.
pub const DEFAULT_SCROLLBACK: usize = 5000;

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

/// Captures OSC 0/2 window title events and BEL events emitted by the VT parser.
///
/// `EventListener::send_event` takes `&self`, so interior mutability is
/// required to update state without `&mut self`.  Title uses `Arc<Mutex>`;
/// the bell flag uses `Arc<AtomicBool>` for lock-free set/clear.
/// Both `Arc`s are also held by `Terminal` so callers can read the state.
#[derive(Clone)]
pub(crate) struct TitleListener {
    title: Arc<Mutex<String>>,
    bell: Arc<AtomicBool>,
}

impl TitleListener {
    fn new(title: Arc<Mutex<String>>, bell: Arc<AtomicBool>) -> Self {
        Self { title, bell }
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
            Event::Bell => {
                self.bell.store(true, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

pub struct Terminal {
    pub term: Term<TitleListener>,
    parser: Processor,
    pub size: TermSize,
    // Shared with TitleListener; updated on every OSC 0/2 event and read by
    // `title()` (CA-39).
    title: Arc<Mutex<String>>,
    /// Latest working-directory path announced via OSC 7, or `None` if none
    /// has been received yet.
    cwd: Option<String>,
    /// Set to `true` by `TitleListener` when a BEL (`\x07`) is received.
    /// Consumed (cleared) by `take_bell()`.
    bell: Arc<AtomicBool>,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize, scrollback: usize) -> Self {
        let size = TermSize { cols, rows };
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        let title = Arc::new(Mutex::new(String::new()));
        let bell = Arc::new(AtomicBool::new(false));
        let listener = TitleListener::new(Arc::clone(&title), Arc::clone(&bell));
        let term = Term::new(config, &size, listener);
        Self {
            term,
            parser: Processor::new(),
            size,
            title,
            cwd: None,
            bell,
        }
    }

    /// Returns the latest window title set via OSC 0/2, or an empty string if
    /// none has been set (or after a `ResetTitle` event).
    pub fn title(&self) -> String {
        self.title.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Returns the latest working directory announced via OSC 7, or `None`.
    pub fn cwd(&self) -> Option<String> {
        self.cwd.clone()
    }

    /// Feed raw PTY output through the VT parser into the grid.
    ///
    /// In addition to advancing the VT state machine, we scan `bytes` for
    /// OSC 7 sequences (`ESC ] 7 ; <uri> BEL` or `ESC ] 7 ; <uri> ST`).
    /// Alacritty's event model does not surface OSC 7, so we detect it in the
    /// raw byte stream ourselves — the same approach used by WezTerm, kitty,
    /// and foot.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
        self.scan_osc7(bytes);
    }

    /// Scan a raw PTY chunk for `ESC ] 7 ; <uri> BEL` or `ESC ] 7 ; <uri> ST`
    /// sequences and update `self.cwd` when found.
    ///
    /// We look for the literal prefix `\x1b]7;` and then collect bytes until
    /// BEL (`\x07`) or the two-byte string-terminator `\x1b\\`.  Multiple
    /// occurrences in one chunk are handled; only the last one wins (matches
    /// the order in which a shell emits them).
    fn scan_osc7(&mut self, bytes: &[u8]) {
        // The OSC 7 prefix as raw bytes: ESC ] 7 ;
        const PREFIX: &[u8] = b"\x1b]7;";
        let mut i = 0;
        while i + PREFIX.len() < bytes.len() {
            // Find the next prefix occurrence.
            let Some(offset) = bytes[i..].windows(PREFIX.len()).position(|w| w == PREFIX) else {
                break;
            };
            let start = i + offset + PREFIX.len(); // first byte of the URI

            // Collect the URI up to BEL or ST.
            let mut end = None;
            let mut j = start;
            while j < bytes.len() {
                if bytes[j] == b'\x07' {
                    // BEL terminator
                    end = Some((j, j + 1));
                    break;
                }
                if bytes[j] == b'\x1b' && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                    // ST (String Terminator) = ESC \
                    end = Some((j, j + 2));
                    break;
                }
                j += 1;
            }

            if let Some((uri_end, next_i)) = end {
                if let Ok(uri) = std::str::from_utf8(&bytes[start..uri_end]) {
                    if let Some(path) = osc7_uri_to_path(uri) {
                        self.cwd = Some(path);
                    }
                }
                i = next_i;
            } else {
                // No terminator found; skip past this prefix to avoid an
                // infinite loop, but stop scanning (partial sequence at end
                // of chunk — the shell will retransmit on the next prompt).
                break;
            }
        }
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

    /// Returns `true` once (and clears the flag) if a BEL (`\x07`) was
    /// received since the last call.  Subsequent calls return `false` until
    /// the next bell.  Safe to call from `&self` (interior-mutable).
    pub fn take_bell(&self) -> bool {
        self.bell.swap(false, Ordering::Relaxed)
    }
}

/// RT-20: hard cap on a single paste's payload (post-sanitization bytes). Guards
/// availability — a huge clipboard would otherwise be copied into a `String`,
/// then a `Vec`, then handed to one `write_all`, spiking memory and blocking the
/// UI thread. 4 MiB is far above any real paste.
const MAX_PASTE_BYTES: usize = 4 * 1024 * 1024;

/// Build the byte stream for pasting `text`.
///
/// Security (RT-4, RT-5b): clipboard content is untrusted — it may carry escape
/// sequences (OSC/title/CSI injection) or an embedded bracketed-paste end marker
/// that would terminate the paste and run the rest as live input. We normalize
/// newlines to CR and strip C0/C1 control bytes except TAB and the normalized CR.
/// Dropping ESC (0x1B) neutralizes all escape injection, including any embedded
/// `\x1b[201~`, so the markers we add ourselves are the only ones present.
pub fn wrap_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let mut cleaned = String::with_capacity(text.len().min(MAX_PASTE_BYTES));
    let mut prev_cr = false;
    for ch in text.chars() {
        // RT-20: cap the paste so a multi-hundred-MB clipboard can't spike RSS or
        // stall the UI thread during the copy + single `write_all`. A genuine
        // paste is far smaller; an oversize one is almost always accidental.
        if cleaned.len() >= MAX_PASTE_BYTES {
            break;
        }
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

    /// Data probe for the wheel-scroll "grey blank band" report: after scrolling
    /// up into history, the rendered viewport (what draw_pane_grid iterates) must
    /// be history TEXT, not blank cells. If this passes, the scroll+content path
    /// is sound and the bug is in rendering/geometry; if it fails, the scroll
    /// wrapper itself is wrong.
    #[test]
    fn scroll_up_reveals_history_not_blank() {
        use std::collections::BTreeMap;
        let mut t = Terminal::new(20, 5, 5000); // 5 visible rows, 5000 scrollback
        for i in 0..40 {
            t.feed(format!("L{i}\r\n").as_bytes());
        }
        t.scroll(8); // up 8 lines into scrollback
        assert!(t.display_offset() > 0, "scroll(8) should leave the bottom");

        let mut rows: BTreeMap<i32, String> = BTreeMap::new();
        let content = t.term.renderable_content();
        for item in content.display_iter {
            rows.entry(item.point.line.0).or_default().push(item.cell.c);
        }
        // The scrolled viewport is history text (e.g. L28). Note the line indices
        // are NEGATIVE here — that's expected; the renderer maps them to screen
        // rows via `screen_row(line, display_offset)` (see paint.rs). The bug was
        // draw_pane_grid skipping negative lines, blanking all scrolled content.
        let texts: Vec<String> = rows.values().map(|s| s.trim_end().to_string()).collect();
        let nonblank = texts.iter().filter(|s| !s.is_empty()).count();
        assert!(
            nonblank >= 3 && texts.iter().any(|s| s.contains("L28")),
            "scrolled viewport should show history text (e.g. L28); got {texts:?}"
        );
    }

    #[test]
    fn wrap_paste_caps_oversize_input() {
        // RT-20: a giant clipboard is truncated rather than copied wholesale.
        let big = "a".repeat(MAX_PASTE_BYTES + 1024);
        let out = wrap_paste(&big, false);
        assert!(
            out.len() <= MAX_PASTE_BYTES,
            "paste not capped: {}",
            out.len()
        );
        assert!(!out.is_empty());
    }

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
        let mut t = Terminal::new(20, 5, DEFAULT_SCROLLBACK);
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
    fn scrollback_param_caps_history() {
        // CA-37: the scrollback knob must actually reach the VT engine. A tiny
        // history (2 lines) on a 5-row grid means after writing far more lines than
        // fit, we can only scroll up by the history depth — not unbounded.
        let mut t = Terminal::new(20, 5, 2);
        for _ in 0..100 {
            t.feed(b"line\r\n");
        }
        t.scroll(1000); // ask to scroll way past the top
                        // display_offset is clamped to the available history (<= scrollback).
        assert!(
            t.display_offset() <= 2,
            "offset {} exceeds the 2-line scrollback cap",
            t.display_offset()
        );

        // A generous scrollback allows scrolling much further back.
        let mut big = Terminal::new(20, 5, 1000);
        for _ in 0..100 {
            big.feed(b"line\r\n");
        }
        big.scroll(1000);
        assert!(
            big.display_offset() > 2,
            "a 1000-line scrollback should allow scrolling past 2 lines, got {}",
            big.display_offset()
        );
    }

    #[test]
    fn feed_writes_chars_into_grid() {
        let mut t = Terminal::new(80, 24, DEFAULT_SCROLLBACK);
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
        let mut t = Terminal::new(80, 24, DEFAULT_SCROLLBACK);
        t.resize(120, 40);
        assert_eq!(t.size.cols, 120);
        assert_eq!(t.size.rows, 40);
        // the underlying grid reports the new geometry too
        assert_eq!(t.term.grid().columns(), 120);
        assert_eq!(t.term.grid().screen_lines(), 40);
    }

    #[test]
    fn bracketed_paste_reflects_terminal_mode() {
        let mut t = Terminal::new(80, 24, DEFAULT_SCROLLBACK);
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
        let mut t = Terminal::new(80, 24, DEFAULT_SCROLLBACK);
        // OSC 0 ; <title> ST — sets icon name *and* window title.
        t.feed(b"\x1b]0;hello\x07");
        assert_eq!(t.title(), "hello", "title should reflect OSC 0 payload");
    }

    #[test]
    fn osc_title_updated_and_reset() {
        let mut t = Terminal::new(80, 24, DEFAULT_SCROLLBACK);
        t.feed(b"\x1b]2;world\x07");
        assert_eq!(t.title(), "world");
        // OSC 104 (reset colors) → ResetTitle not triggered; OSC l (xterm iconName) - skip.
        // Drive a second OSC 0 to update:
        t.feed(b"\x1b]0;updated\x07");
        assert_eq!(t.title(), "updated");
    }

    // --- OSC 7 (shell cwd) tests ---

    #[test]
    fn osc7_bel_terminator_updates_cwd() {
        let mut t = Terminal::new(80, 24, DEFAULT_SCROLLBACK);
        assert!(t.cwd().is_none(), "cwd starts empty");
        // Windows-style URI: file://hostname/C:/Users/alice
        t.feed(b"\x1b]7;file://myhost/C:/Users/alice\x07");
        let cwd = t.cwd().expect("cwd should be set after OSC 7");
        // On Windows the leading '/' is stripped and slashes converted.
        #[cfg(windows)]
        assert_eq!(cwd, "C:\\Users\\alice");
        #[cfg(not(windows))]
        assert_eq!(cwd, "/C:/Users/alice");
    }

    #[test]
    fn osc7_st_terminator_updates_cwd() {
        let mut t = Terminal::new(80, 24, DEFAULT_SCROLLBACK);
        // Use ST (ESC \) as the terminator instead of BEL.
        t.feed(b"\x1b]7;file:///tmp/work\x1b\\");
        let cwd = t.cwd().expect("cwd should be set with ST terminator");
        assert_eq!(cwd, "/tmp/work");
    }

    #[test]
    fn osc7_last_sequence_wins() {
        let mut t = Terminal::new(80, 24, DEFAULT_SCROLLBACK);
        // Two OSC 7 sequences in one chunk; the last one should win.
        t.feed(b"\x1b]7;file:///first\x07\x1b]7;file:///second\x07");
        assert_eq!(t.cwd().as_deref(), Some("/second"));
    }

    #[test]
    fn osc7_percent_decode_in_path() {
        let mut t = Terminal::new(80, 24, DEFAULT_SCROLLBACK);
        // Space encoded as %20.
        t.feed(b"\x1b]7;file:///my%20dir\x07");
        assert_eq!(t.cwd().as_deref(), Some("/my dir"));
    }

    // --- BEL / visual bell tests ---

    #[test]
    fn bel_byte_registers_bell() {
        let mut t = Terminal::new(80, 24, DEFAULT_SCROLLBACK);
        assert!(!t.take_bell(), "no bell before BEL byte");
        t.feed(b"\x07");
        assert!(t.take_bell(), "bell should be registered after \\x07");
    }

    #[test]
    fn take_bell_clears_flag() {
        let mut t = Terminal::new(80, 24, DEFAULT_SCROLLBACK);
        t.feed(b"\x07");
        assert!(t.take_bell(), "first take returns true");
        assert!(!t.take_bell(), "second take returns false (consumed)");
    }

    // --- percent_decode unit tests ---

    #[test]
    fn percent_decode_plain_string() {
        assert_eq!(percent_decode("hello"), "hello");
    }

    #[test]
    fn percent_decode_space() {
        assert_eq!(percent_decode("my%20dir"), "my dir");
    }

    #[test]
    fn percent_decode_mixed() {
        assert_eq!(percent_decode("a%2Fb%20c"), "a/b c");
    }

    #[test]
    fn percent_decode_invalid_sequence_passed_through() {
        // %XX where XX are not valid hex — leave as-is.
        assert_eq!(percent_decode("a%ZZb"), "a%ZZb");
    }

    #[test]
    fn percent_decode_truncated_at_end() {
        // Percent sign at the very end with only one hex digit — leave as-is.
        assert_eq!(percent_decode("a%2"), "a%2");
    }
}
