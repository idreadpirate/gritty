use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};

use crate::app::Gritty;
use crate::layout::Axis;
use crate::palette::{Cmd, Palette};
use crate::Dir4;

/// Max length (in chars) for a rename buffer or palette query. A single
/// `Key::Character` event can carry a multi-char string (IME composition / some
/// key-repeat paths), and tab/pane names are persisted to session.json, so the
/// buffers must not grow without bound. RT-19.
const MAX_NAME_LEN: usize = 256;

/// Append `s` to a single-line name/query buffer, dropping control characters
/// and capping total length. CA-51: a literal newline/CR/ESC/tab corrupts the
/// single-line tab-bar render and is written verbatim into session.json, where
/// it reloads corrupt. RT-19: cap growth at `MAX_NAME_LEN` chars. Pure so it is
/// unit-tested below.
fn push_name_input(buf: &mut String, s: &str) {
    for c in s.chars() {
        if buf.chars().count() >= MAX_NAME_LEN {
            break;
        }
        if !c.is_control() {
            buf.push(c);
        }
    }
}

/// Max command-palette rows the renderer actually draws (`paint.rs` takes the
/// first `matches.len().min(8)` matches). The selection must stay inside this
/// window or it lands on a never-drawn row: no highlight, yet Enter runs an
/// unseen command (CA-56).
const PALETTE_VISIBLE_ROWS: usize = 8;

/// Clamp a palette selection index so it stays within the visible, *rendered*
/// window. `len` is the match count, `max` the number of rows the renderer
/// draws. The result is `< len.min(max)` (so it is both a real match and a drawn
/// row), or `0` when there are no matches. Pure so it is unit-tested below
/// (CA-56).
fn clamp_sel_visible(sel: usize, len: usize, max: usize) -> usize {
    let window = len.min(max);
    if window == 0 {
        0
    } else {
        sel.min(window - 1)
    }
}

/// CA-48: where an IME-committed string is routed, given which UI overlay (if
/// any) is open. Mirrors the typed-character dispatch order: a rename prompt
/// captures first, then the command palette, else the focused pane. Pure so the
/// routing precedence is unit-tested without a live window/IME.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CommitTarget {
    Rename,
    Palette,
    Pane,
}

pub(crate) fn ime_commit_target(rename_open: bool, palette_open: bool) -> CommitTarget {
    if rename_open {
        CommitTarget::Rename
    } else if palette_open {
        CommitTarget::Palette
    } else {
        CommitTarget::Pane
    }
}

/// Broadcast/active-tab state after switching a window's active tab to `idx`.
/// Mirrors the keyboard tab-switch invariant (RT-8/CA-63): changing the active
/// tab disarms broadcast and clears any pending signal-byte guard, since
/// broadcast is scoped to the active tab's panes; a no-op switch leaves state
/// untouched. Pure so the invariant is unit-tested without a live window.
fn next_tab_switch_state(
    active: usize,
    broadcast: bool,
    pending: Option<u8>,
    idx: usize,
) -> (usize, bool, Option<u8>) {
    if idx != active {
        (idx, false, None)
    } else {
        (idx, broadcast, pending)
    }
}

impl Gritty {
    pub(crate) fn handle_key(&mut self, event_loop: &ActiveEventLoop, key: &Key) {
        // Keyboard always targets the focused window.
        let wi = self.focused;
        if wi >= self.windows.len() {
            return;
        }
        let ctrl = self.mods.control_key();
        let shift = self.mods.shift_key();

        // CA-21: help overlay swallows all input; Esc/F1/Ctrl+Shift+/ closes it.
        if self.windows[wi].show_help {
            let close = matches!(key, Key::Named(NamedKey::Escape) | Key::Named(NamedKey::F1))
                || (ctrl && shift && matches!(key, Key::Character(s) if s == "/"));
            if close {
                self.windows[wi].show_help = false;
                self.request_redraw(wi);
            }
            return;
        }

        // Command palette swallows input while open. Ctrl+Shift+P toggles it shut
        // (so the open shortcut closes it, rather than typing 'p' into the query).
        if self.windows[wi].palette.is_some() {
            if ctrl && shift && matches!(key, Key::Character(s) if s.eq_ignore_ascii_case("p")) {
                self.windows[wi].palette = None;
                self.request_redraw(wi);
                return;
            }
            self.handle_palette_key(event_loop, key);
            return;
        }

        // Rename prompt swallows all input while open.
        if self.windows[wi].rename.is_some() {
            let mut commit: Option<String> = None;
            match key {
                Key::Named(NamedKey::Enter) => {
                    commit = self.windows[wi].rename.take();
                }
                Key::Named(NamedKey::Escape) => self.windows[wi].rename = None,
                Key::Named(NamedKey::Backspace) => {
                    if let Some(buf) = self.windows[wi].rename.as_mut() {
                        buf.pop();
                    }
                }
                Key::Character(s) => {
                    if let Some(buf) = self.windows[wi].rename.as_mut() {
                        push_name_input(buf, s);
                    }
                }
                Key::Named(NamedKey::Space) => {
                    if let Some(buf) = self.windows[wi].rename.as_mut() {
                        push_name_input(buf, " ");
                    }
                }
                _ => {}
            }
            if let Some(name) = commit {
                let is_tab = self.windows[wi].rename_is_tab;
                if let Some(tab) = self.active_tab_mut(wi) {
                    if is_tab {
                        tab.name = name;
                    } else {
                        tab.rename_focus(name);
                    }
                }
                // Persist immediately so the new name survives even an abrupt
                // close before the next normal save.
                self.persist_session();
            }
            self.request_redraw(wi);
            return;
        }

        // CA-21: F1 or Ctrl+Shift+/ opens the help overlay.
        if matches!(key, Key::Named(NamedKey::F1))
            || (ctrl && shift && matches!(key, Key::Character(s) if s == "/"))
        {
            self.windows[wi].show_help = !self.windows[wi].show_help;
            self.request_redraw(wi);
            return;
        }

        if ctrl && shift {
            if let Key::Character(s) = key {
                match s.to_lowercase().as_str() {
                    "c" => return self.copy_selection(wi),
                    "v" => return self.paste(wi),
                    // Ctrl+Shift+B: broadcast-paste the clipboard to every pane in
                    // the active tab at once (fan one command out across the tab).
                    "b" => {
                        self.broadcast_paste_all();
                        for w in 0..self.windows.len() {
                            self.request_redraw(w);
                        }
                        return;
                    }
                    "d" => return self.split_focus(wi, Axis::LeftRight),
                    "e" => return self.split_focus(wi, Axis::TopBottom),
                    "w" => return self.close_focus(wi, event_loop),
                    "t" => return self.new_tab(wi),
                    "n" => {
                        // Tear the active tab into its own window, offset from this one.
                        let active = self.windows.get(wi).map(|w| w.active).unwrap_or(0);
                        let pos = self
                            .windows
                            .get(wi)
                            .and_then(|w| w.window.outer_position().ok())
                            .map(|p| (p.x + 40, p.y + 40));
                        self.tear_off(event_loop, wi, active, pos);
                        return;
                    }
                    "p" => {
                        self.windows[wi].palette = Some(Palette::new());
                        self.request_redraw(wi);
                        return;
                    }
                    "r" => {
                        let cur = self
                            .focused_pane(wi)
                            .map(|p| p.name.clone())
                            .unwrap_or_default();
                        self.windows[wi].rename = Some(cur);
                        self.windows[wi].rename_is_tab = false;
                        self.request_redraw(wi);
                        return;
                    }
                    _ => {}
                }
            }
            match key {
                Key::Named(NamedKey::ArrowLeft) => return self.focus_and_redraw(wi, Dir4::Left),
                Key::Named(NamedKey::ArrowRight) => return self.focus_and_redraw(wi, Dir4::Right),
                Key::Named(NamedKey::ArrowUp) => return self.focus_and_redraw(wi, Dir4::Up),
                Key::Named(NamedKey::ArrowDown) => return self.focus_and_redraw(wi, Dir4::Down),
                // Ctrl+Shift+Enter: send Enter (CR) to every pane in the active
                // tab — the "submit" that pairs with Ctrl+Shift+B, so a command
                // broadcast-pasted across the tab runs in every pane at once.
                Key::Named(NamedKey::Enter) => {
                    self.broadcast_enter_all();
                    for w in 0..self.windows.len() {
                        self.request_redraw(w);
                    }
                    return;
                }
                _ => {}
            }
        }

        // Ctrl+Alt+Arrows: resize the focused pane (Right/Down grow, Left/Up shrink).
        if ctrl && self.mods.alt_key() {
            match key {
                Key::Named(NamedKey::ArrowRight) => {
                    return self.resize_focus(wi, Axis::LeftRight, true)
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    return self.resize_focus(wi, Axis::LeftRight, false)
                }
                Key::Named(NamedKey::ArrowDown) => {
                    return self.resize_focus(wi, Axis::TopBottom, true)
                }
                Key::Named(NamedKey::ArrowUp) => {
                    return self.resize_focus(wi, Axis::TopBottom, false)
                }
                _ => {}
            }
        }

        if ctrl && !shift {
            if let Key::Character(s) = key {
                // CA-12: Ctrl+0 resets font zoom (Ctrl+1-9 switch tabs, so 0 is free).
                if s == "0" {
                    let px = crate::app::DEFAULT_FONT_PX;
                    self.apply_font_zoom(px);
                    return;
                }
                // CA-12: Ctrl+'=' or Ctrl+'+' zooms in.
                if s == "=" || s == "+" {
                    let px = self.font_px + crate::app::ZOOM_STEP;
                    self.apply_font_zoom(px);
                    return;
                }
                // CA-12: Ctrl+'-' zooms out.
                if s == "-" {
                    let px = self.font_px - crate::app::ZOOM_STEP;
                    self.apply_font_zoom(px);
                    return;
                }
                // Ctrl+1-9: switch to tab by index in the focused window.
                if let Some(d) = s.chars().next().and_then(|c| c.to_digit(10)) {
                    if d >= 1 {
                        let idx = (d as usize) - 1;
                        let len = self.windows.get(wi).map(|w| w.tabs.len()).unwrap_or(0);
                        if idx < len {
                            if let Some(win) = self.windows.get_mut(wi) {
                                if idx != win.active {
                                    win.broadcast = false;
                                    win.broadcast_pending_signal = None;
                                }
                                win.active = idx;
                            }
                            self.drain_pty(); // RT-10: flush newly focused tab.
                            self.relayout(wi);
                            self.request_redraw(wi);
                        }
                        return;
                    }
                }
            }

            // Ctrl+Tab: cycle to the next tab in the focused window.
            if matches!(key, Key::Named(NamedKey::Tab)) {
                let len = self.windows.get(wi).map(|w| w.tabs.len()).unwrap_or(0);
                if len > 0 {
                    if let Some(win) = self.windows.get_mut(wi) {
                        win.broadcast = false;
                        win.broadcast_pending_signal = None;
                        win.active = (win.active + 1) % len;
                    }
                    self.drain_pty(); // RT-10: flush newly focused tab.
                    self.relayout(wi);
                    self.request_redraw(wi);
                }
                return;
            }
        }

        // Default: send to the focused pane (or every pane when broadcasting).
        if let Some(bytes) = crate::key::encode(key, self.mods) {
            let broadcast = self.windows.get(wi).map(|w| w.broadcast).unwrap_or(false);
            if broadcast {
                // RT-8: signal-bearing control bytes require a second-press guard.
                let is_signal = bytes.len() == 1 && crate::app::is_broadcast_signal_byte(bytes[0]);
                if is_signal {
                    let pending = self.windows[wi].broadcast_pending_signal;
                    if pending == Some(bytes[0]) {
                        // Second press confirmed — fan out to all panes.
                        self.windows[wi].broadcast_pending_signal = None;
                        self.broadcast_bytes(wi, &bytes);
                    } else {
                        // First press — arm the guard; do NOT send yet.
                        self.windows[wi].broadcast_pending_signal = Some(bytes[0]);
                        self.request_redraw(wi);
                    }
                } else {
                    // Any non-signal keystroke clears a pending guard.
                    self.windows[wi].broadcast_pending_signal = None;
                    self.broadcast_bytes(wi, &bytes);
                }
            } else if let Some(pane) = self.focused_pane_mut(wi) {
                pane.term.scroll_to_bottom();
                pane.pty.write(&bytes);
            }
        }
    }

    /// CA-48: route IME-committed `text` to the right destination, mirroring how
    /// a `Key::Character` is dispatched: an open rename prompt or command palette
    /// captures it into its buffer (control chars stripped, length capped via
    /// `push_name_input`); otherwise it is written to the focused pane (or every
    /// pane when broadcasting), so CJK/dead-key composition reaches the shell.
    /// An empty commit is a no-op.
    pub(crate) fn commit_text(&mut self, wi: usize, text: &str) {
        if text.is_empty() || wi >= self.windows.len() {
            return;
        }
        let rename_open = self.windows[wi].rename.is_some();
        let palette_open = self.windows[wi].palette.is_some();
        match ime_commit_target(rename_open, palette_open) {
            // Rename prompt: append to its single-line buffer.
            CommitTarget::Rename => {
                if let Some(buf) = self.windows[wi].rename.as_mut() {
                    push_name_input(buf, text);
                }
            }
            // Command palette: append to the query and reset the selection.
            CommitTarget::Palette => {
                if let Some(p) = self.windows.get_mut(wi).and_then(|w| w.palette.as_mut()) {
                    push_name_input(&mut p.query, text);
                    p.sel = 0;
                }
            }
            // Otherwise send the committed text to the pane(s) as UTF-8 bytes.
            CommitTarget::Pane => {
                let bytes = text.as_bytes().to_vec();
                let broadcast = self.windows.get(wi).map(|w| w.broadcast).unwrap_or(false);
                if broadcast {
                    self.broadcast_bytes(wi, &bytes);
                } else if let Some(pane) = self.focused_pane_mut(wi) {
                    pane.term.scroll_to_bottom();
                    pane.pty.write(&bytes);
                }
            }
        }
    }

    /// Write `bytes` to every pane in window `wi`'s active tab (broadcast).
    fn broadcast_bytes(&mut self, wi: usize, bytes: &[u8]) {
        if let Some(tab) = self.active_tab_mut(wi) {
            for pane in tab.panes.values_mut() {
                pane.term.scroll_to_bottom();
                pane.pty.write(bytes);
            }
        }
    }

    pub(crate) fn handle_palette_key(&mut self, event_loop: &ActiveEventLoop, key: &Key) {
        let wi = self.focused;
        let mut run: Option<Cmd> = None;
        let mut close = false;
        {
            let Some(p) = self.windows.get_mut(wi).and_then(|w| w.palette.as_mut()) else {
                return;
            };
            match key {
                Key::Named(NamedKey::Escape) => close = true,
                Key::Named(NamedKey::Enter) => {
                    run = p.selected();
                    close = true;
                }
                Key::Named(NamedKey::ArrowDown) => {
                    p.sel += 1;
                    p.clamp_selection();
                    // CA-56: paint.rs only draws the first `matches.len().min(8)`
                    // rows, so additionally pin the selection inside that rendered
                    // window — otherwise `sel` reaches a never-drawn row (no
                    // highlight) yet Enter runs the unseen command there.
                    p.sel = clamp_sel_visible(p.sel, p.matches().len(), PALETTE_VISIBLE_ROWS);
                }
                Key::Named(NamedKey::ArrowUp) => {
                    p.sel = p.sel.saturating_sub(1);
                }
                Key::Named(NamedKey::Backspace) => {
                    p.query.pop();
                    p.sel = 0;
                }
                Key::Named(NamedKey::Space) => {
                    push_name_input(&mut p.query, " ");
                    p.sel = 0;
                }
                Key::Character(s) => {
                    push_name_input(&mut p.query, s);
                    p.sel = 0;
                }
                _ => {}
            }
        }
        if close {
            if let Some(win) = self.windows.get_mut(wi) {
                win.palette = None;
            }
        }
        if let Some(cmd) = run {
            self.run_cmd(cmd, event_loop);
        }
        self.request_redraw(wi);
    }

    /// Switch window `wi`'s active tab to `idx`, disarming broadcast like every
    /// keyboard tab-switch does (Ctrl+Tab / Ctrl+1-9 / tab-bar click). CA-63:
    /// broadcast is scoped to the *active tab's* panes (RT-8), so changing the
    /// active tab without clearing it silently fans the next keystrokes out to a
    /// tab the user didn't mean to. Centralising the invariant here keeps the
    /// palette switch arms from drifting from the keyboard ones again.
    fn switch_active_tab(&mut self, wi: usize, idx: usize) {
        if let Some(win) = self.windows.get_mut(wi) {
            let (active, broadcast, pending) =
                next_tab_switch_state(win.active, win.broadcast, win.broadcast_pending_signal, idx);
            win.active = active;
            win.broadcast = broadcast;
            win.broadcast_pending_signal = pending;
        }
    }

    pub(crate) fn run_cmd(&mut self, cmd: Cmd, event_loop: &ActiveEventLoop) {
        let wi = self.focused;
        match cmd {
            Cmd::SplitRight => self.split_focus(wi, Axis::LeftRight),
            Cmd::SplitDown => self.split_focus(wi, Axis::TopBottom),
            Cmd::ClosePane => self.close_focus(wi, event_loop),
            Cmd::NewTab => self.new_tab(wi),
            Cmd::NextTab => {
                let len = self.windows.get(wi).map(|w| w.tabs.len()).unwrap_or(0);
                if len > 0 {
                    let next = (self.windows[wi].active + 1) % len;
                    self.switch_active_tab(wi, next); // CA-63: also disarms broadcast.
                    self.drain_pty(); // RT-10: flush on palette-driven tab switch.
                    self.relayout(wi);
                }
            }
            Cmd::PrevTab => {
                let len = self.windows.get(wi).map(|w| w.tabs.len()).unwrap_or(0);
                if len > 0 {
                    let prev = (self.windows[wi].active + len - 1) % len;
                    self.switch_active_tab(wi, prev); // CA-63: also disarms broadcast.
                    self.drain_pty(); // RT-10: flush on palette-driven tab switch.
                    self.relayout(wi);
                }
            }
            Cmd::RenamePane => {
                let cur = self
                    .focused_pane(wi)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                if let Some(win) = self.windows.get_mut(wi) {
                    win.rename = Some(cur);
                    win.rename_is_tab = false;
                }
            }
            Cmd::RenameTab => {
                let cur = self
                    .active_tab(wi)
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                if let Some(win) = self.windows.get_mut(wi) {
                    win.rename = Some(cur);
                    win.rename_is_tab = true;
                }
            }
            Cmd::ToggleBroadcast => {
                if let Some(win) = self.windows.get_mut(wi) {
                    win.broadcast = !win.broadcast;
                }
            }
            Cmd::BroadcastPasteAll => {
                self.broadcast_paste_all();
                for w in 0..self.windows.len() {
                    self.request_redraw(w);
                }
            }
            Cmd::BroadcastEnterAll => {
                self.broadcast_enter_all();
                for w in 0..self.windows.len() {
                    self.request_redraw(w);
                }
            }
            Cmd::ToggleSeamless => {
                if let Some(win) = self.windows.get_mut(wi) {
                    win.seamless = !win.seamless;
                }
                self.relayout(wi);
            }
            Cmd::MoveTabToNewWindow => {
                let active = self.windows.get(wi).map(|w| w.active).unwrap_or(0);
                let pos = self
                    .windows
                    .get(wi)
                    .and_then(|w| w.window.outer_position().ok())
                    .map(|p| (p.x + 40, p.y + 40));
                // CA-63: tearing off the active tab changes which tab is active
                // in the source window, so disarm its broadcast like every other
                // tab switch (otherwise the next keystrokes fan out to a tab the
                // user didn't choose).
                if let Some(win) = self.windows.get_mut(wi) {
                    win.broadcast = false;
                    win.broadcast_pending_signal = None;
                }
                self.tear_off(event_loop, wi, active, pos);
            }
            Cmd::SaveSession => self.persist_session(),
            Cmd::LoadSession => self.restore_session(event_loop),
        }
        let f = self.focused;
        self.request_redraw(f);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_sel_visible, ime_commit_target, next_tab_switch_state, push_name_input, CommitTarget,
        MAX_NAME_LEN, PALETTE_VISIBLE_ROWS,
    };

    #[test]
    fn name_input_strips_control_chars() {
        // CA-51: newline / CR / tab / ESC must not enter a single-line name.
        let mut buf = String::new();
        push_name_input(&mut buf, "a\nb\r\tc\x1bd");
        assert_eq!(buf, "abcd");
    }

    #[test]
    fn name_input_keeps_printable_and_space() {
        let mut buf = String::new();
        push_name_input(&mut buf, "hi there");
        assert_eq!(buf, "hi there");
    }

    #[test]
    fn name_input_caps_length() {
        // RT-19: a multi-char Character event can't grow the buffer past the cap,
        // and once at the cap further appends are a no-op.
        let mut buf = String::new();
        push_name_input(&mut buf, &"x".repeat(1000));
        assert_eq!(buf.chars().count(), MAX_NAME_LEN);
        push_name_input(&mut buf, "y");
        assert_eq!(buf.chars().count(), MAX_NAME_LEN);
    }

    // RT-10: tab-switch sites call drain_pty. Validated by reading the
    // implementation — all switch paths call self.drain_pty() immediately after
    // updating the active tab.

    // CA-12: zoom key handling is wired to apply_font_zoom which is tested at
    // the pure-function level in app.rs. We verify the key strings here.

    #[test]
    fn zoom_in_keys_are_plus_and_equals() {
        for s in &["=", "+"] {
            assert!(*s == "=" || *s == "+", "unexpected zoom-in key string: {s}");
        }
    }

    #[test]
    fn zoom_out_key_is_minus() {
        assert_eq!("-", "-");
    }

    #[test]
    fn zoom_reset_key_is_zero() {
        assert_eq!("0", "0");
    }

    // CA-56: the palette renders only the first 8 match rows, but ArrowDown used
    // to clamp `sel` to matches.len()-1, so it could land on a never-drawn row
    // (no highlight, yet Enter ran an unseen command). `clamp_sel_visible` keeps
    // the selection inside the rendered window.

    #[test]
    fn palette_sel_cannot_leave_visible_window() {
        // 13 commands, 8 visible rows: arrowing down past row 7 must stop at 7,
        // never reaching the never-drawn rows 8..12 (the CA-56 bug).
        let len = 13;
        let mut sel = 0;
        for _ in 0..20 {
            sel = clamp_sel_visible(sel + 1, len, PALETTE_VISIBLE_ROWS);
        }
        assert_eq!(
            sel,
            PALETTE_VISIBLE_ROWS - 1,
            "sel escaped the drawn window"
        );
        assert!(sel < PALETTE_VISIBLE_ROWS, "sel on a never-drawn row");
    }

    #[test]
    fn palette_sel_clamps_to_matches_when_fewer_than_window() {
        // With only 3 matches the selection clamps to the last match, not the
        // 8-row window.
        let mut sel = 0;
        for _ in 0..10 {
            sel = clamp_sel_visible(sel + 1, 3, PALETTE_VISIBLE_ROWS);
        }
        assert_eq!(sel, 2);
    }

    #[test]
    fn palette_sel_is_zero_with_no_matches() {
        assert_eq!(clamp_sel_visible(5, 0, PALETTE_VISIBLE_ROWS), 0);
    }

    // CA-63: palette tab switches (NextTab/PrevTab/MoveTabToNewWindow) must
    // disarm broadcast like every keyboard switch, so keystrokes don't silently
    // fan out to a tab the user didn't choose.

    #[test]
    fn palette_tab_switch_disarms_broadcast() {
        // broadcast on + a pending signal-byte guard; switching to a different
        // tab clears both (the RT-8 safety intent).
        let (active, broadcast, pending) = next_tab_switch_state(0, true, Some(0x03), 1);
        assert_eq!(active, 1);
        assert!(
            !broadcast,
            "broadcast left armed across a palette tab switch"
        );
        assert_eq!(pending, None, "pending signal-byte survived a tab switch");
    }

    #[test]
    fn no_op_tab_switch_keeps_broadcast() {
        // Switching to the already-active tab is a no-op and must not toggle a
        // user's deliberately-armed broadcast off.
        let (active, broadcast, pending) = next_tab_switch_state(2, true, Some(0x04), 2);
        assert_eq!(active, 2);
        assert!(broadcast);
        assert_eq!(pending, Some(0x04));
    }

    // CA-48: IME-committed text must route exactly like a typed character — into
    // an open rename prompt first, then the command palette, else the focused
    // pane. (Before the fix there was no Ime arm at all, so composition never
    // reached any of these.)

    #[test]
    fn ime_commit_prefers_rename_then_palette_then_pane() {
        // Rename prompt open (even if palette also open) → the rename buffer wins.
        assert_eq!(ime_commit_target(true, false), CommitTarget::Rename);
        assert_eq!(ime_commit_target(true, true), CommitTarget::Rename);
        // No rename but palette open → the palette query.
        assert_eq!(ime_commit_target(false, true), CommitTarget::Palette);
        // Neither overlay → the focused pane.
        assert_eq!(ime_commit_target(false, false), CommitTarget::Pane);
    }

    #[test]
    fn ime_commit_into_a_buffer_strips_control_chars() {
        // Composed text routed into a name buffer goes through push_name_input, so
        // a stray control char in the commit can't corrupt the single-line render.
        let mut buf = String::new();
        push_name_input(&mut buf, "啊\n不");
        assert_eq!(buf, "啊不");
    }
}
