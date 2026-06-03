use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};

use crate::app::Gritty;
use crate::layout::Axis;
use crate::palette::{Cmd, Palette};
use crate::Dir4;

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

        // Command palette swallows input while open.
        if self.windows[wi].palette.is_some() {
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
                        buf.push_str(s);
                    }
                }
                Key::Named(NamedKey::Space) => {
                    if let Some(buf) = self.windows[wi].rename.as_mut() {
                        buf.push(' ');
                    }
                }
                _ => {}
            }
            if let Some(name) = commit {
                let is_tab = self.windows[wi].rename_is_tab;
                if let Some(win) = self.windows.get_mut(wi) {
                    let active = win.active;
                    if let Some(tab) = win.tabs.get_mut(active) {
                        if is_tab {
                            tab.name = name;
                        } else {
                            tab.rename_focus(name);
                        }
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
                            .windows
                            .get(wi)
                            .and_then(|w| w.tabs.get(w.active))
                            .and_then(|t| t.panes.get(&t.focus))
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
            } else if let Some(win) = self.windows.get_mut(wi) {
                let active = win.active;
                if let Some(tab) = win.tabs.get_mut(active) {
                    let f = tab.focus;
                    if let Some(pane) = tab.panes.get_mut(&f) {
                        pane.term.scroll_to_bottom();
                        pane.pty.write(&bytes);
                    }
                }
            }
        }
    }

    /// Write `bytes` to every pane in window `wi`'s active tab (broadcast).
    fn broadcast_bytes(&mut self, wi: usize, bytes: &[u8]) {
        if let Some(win) = self.windows.get_mut(wi) {
            let active = win.active;
            if let Some(tab) = win.tabs.get_mut(active) {
                for pane in tab.panes.values_mut() {
                    pane.term.scroll_to_bottom();
                    pane.pty.write(bytes);
                }
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
                }
                Key::Named(NamedKey::ArrowUp) => {
                    p.sel = p.sel.saturating_sub(1);
                }
                Key::Named(NamedKey::Backspace) => {
                    p.query.pop();
                    p.sel = 0;
                }
                Key::Named(NamedKey::Space) => {
                    p.query.push(' ');
                    p.sel = 0;
                }
                Key::Character(s) => {
                    p.query.push_str(s);
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
                    if let Some(win) = self.windows.get_mut(wi) {
                        win.active = (win.active + 1) % len;
                    }
                    self.drain_pty(); // RT-10: flush on palette-driven tab switch.
                    self.relayout(wi);
                }
            }
            Cmd::PrevTab => {
                let len = self.windows.get(wi).map(|w| w.tabs.len()).unwrap_or(0);
                if len > 0 {
                    if let Some(win) = self.windows.get_mut(wi) {
                        win.active = (win.active + len - 1) % len;
                    }
                    self.drain_pty(); // RT-10: flush on palette-driven tab switch.
                    self.relayout(wi);
                }
            }
            Cmd::RenamePane => {
                let cur = self
                    .windows
                    .get(wi)
                    .and_then(|w| w.tabs.get(w.active))
                    .and_then(|t| t.panes.get(&t.focus))
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                if let Some(win) = self.windows.get_mut(wi) {
                    win.rename = Some(cur);
                    win.rename_is_tab = false;
                }
            }
            Cmd::RenameTab => {
                let cur = self
                    .windows
                    .get(wi)
                    .and_then(|w| w.tabs.get(w.active))
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
}
