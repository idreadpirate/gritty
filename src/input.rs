use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};

use crate::app::Gritty;
use crate::layout::Axis;
use crate::palette::{Cmd, Palette};
use crate::persist;
use crate::Dir4;

impl Gritty {
    pub(crate) fn handle_key(&mut self, event_loop: &ActiveEventLoop, key: &Key) {
        // Command palette swallows input while open.
        if self.palette.is_some() {
            self.handle_palette_key(event_loop, key);
            return;
        }

        // Rename prompt swallows all input while open.
        if let Some(buf) = self.rename.as_mut() {
            match key {
                Key::Named(NamedKey::Enter) => {
                    let name = std::mem::take(buf);
                    self.rename = None;
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        tab.rename_focus(name);
                    }
                }
                Key::Named(NamedKey::Escape) => self.rename = None,
                Key::Named(NamedKey::Backspace) => {
                    buf.pop();
                }
                Key::Character(s) => buf.push_str(s),
                Key::Named(NamedKey::Space) => buf.push(' '),
                _ => {}
            }
            self.request_redraw();
            return;
        }

        let ctrl = self.mods.control_key();
        let shift = self.mods.shift_key();

        if ctrl && shift {
            if let Key::Character(s) = key {
                match s.to_lowercase().as_str() {
                    "c" => return self.copy_selection(),
                    "v" => return self.paste(),
                    "d" => return self.split_focus(Axis::LeftRight),
                    "e" => return self.split_focus(Axis::TopBottom),
                    "w" => return self.close_focus(event_loop),
                    "t" => return self.new_tab(),
                    "p" => {
                        self.palette = Some(Palette::new());
                        self.request_redraw();
                        return;
                    }
                    "r" => {
                        let cur = self
                            .tabs
                            .get(self.active)
                            .and_then(|t| t.panes.get(&t.focus))
                            .map(|p| p.name.clone())
                            .unwrap_or_default();
                        self.rename = Some(cur);
                        self.request_redraw();
                        return;
                    }
                    _ => {}
                }
            }
            match key {
                Key::Named(NamedKey::ArrowLeft) => return self.focus_and_redraw(Dir4::Left),
                Key::Named(NamedKey::ArrowRight) => return self.focus_and_redraw(Dir4::Right),
                Key::Named(NamedKey::ArrowUp) => return self.focus_and_redraw(Dir4::Up),
                Key::Named(NamedKey::ArrowDown) => return self.focus_and_redraw(Dir4::Down),
                _ => {}
            }
        }

        // Ctrl+Alt+Arrows: resize the focused pane (Right/Down grow, Left/Up shrink).
        if ctrl && self.mods.alt_key() {
            match key {
                Key::Named(NamedKey::ArrowRight) => {
                    return self.resize_focus(Axis::LeftRight, true)
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    return self.resize_focus(Axis::LeftRight, false)
                }
                Key::Named(NamedKey::ArrowDown) => return self.resize_focus(Axis::TopBottom, true),
                Key::Named(NamedKey::ArrowUp) => return self.resize_focus(Axis::TopBottom, false),
                _ => {}
            }
        }

        if ctrl && !shift {
            if let Key::Character(s) = key {
                if let Some(d) = s.chars().next().and_then(|c| c.to_digit(10)) {
                    if d >= 1 {
                        let idx = (d as usize) - 1;
                        if idx < self.tabs.len() {
                            self.active = idx;
                            self.relayout();
                            self.request_redraw();
                        }
                        return;
                    }
                }
            }
            if matches!(key, Key::Named(NamedKey::Tab)) {
                if !self.tabs.is_empty() {
                    self.active = (self.active + 1) % self.tabs.len();
                    self.relayout();
                    self.request_redraw();
                }
                return;
            }
        }

        // Default: send to the focused pane (or every pane when broadcasting).
        if let Some(bytes) = crate::key::encode(key, self.mods) {
            if let Some(tab) = self.tabs.get_mut(self.active) {
                if self.broadcast {
                    for pane in tab.panes.values_mut() {
                        pane.term.scroll_to_bottom();
                        pane.pty.write(&bytes);
                    }
                } else {
                    let f = tab.focus;
                    if let Some(pane) = tab.panes.get_mut(&f) {
                        pane.term.scroll_to_bottom();
                        pane.pty.write(&bytes);
                    }
                }
            }
        }
    }

    pub(crate) fn handle_palette_key(&mut self, event_loop: &ActiveEventLoop, key: &Key) {
        let Some(p) = self.palette.as_mut() else {
            return;
        };
        match key {
            Key::Named(NamedKey::Escape) => self.palette = None,
            Key::Named(NamedKey::Enter) => {
                let cmd = p.selected();
                self.palette = None;
                if let Some(cmd) = cmd {
                    self.run_cmd(cmd, event_loop);
                }
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
        self.request_redraw();
    }

    pub(crate) fn run_cmd(&mut self, cmd: Cmd, event_loop: &ActiveEventLoop) {
        match cmd {
            Cmd::SplitRight => self.split_focus(Axis::LeftRight),
            Cmd::SplitDown => self.split_focus(Axis::TopBottom),
            Cmd::ClosePane => self.close_focus(event_loop),
            Cmd::NewTab => self.new_tab(),
            Cmd::NextTab => {
                if !self.tabs.is_empty() {
                    self.active = (self.active + 1) % self.tabs.len();
                    self.relayout();
                }
            }
            Cmd::PrevTab => {
                if !self.tabs.is_empty() {
                    self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
                    self.relayout();
                }
            }
            Cmd::RenamePane => {
                let cur = self
                    .tabs
                    .get(self.active)
                    .and_then(|t| t.panes.get(&t.focus))
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                self.rename = Some(cur);
            }
            Cmd::ToggleBroadcast => self.broadcast = !self.broadcast,
            Cmd::ToggleSeamless => {
                self.seamless = !self.seamless;
                self.relayout();
            }
            Cmd::SaveSession => {
                let _ = persist::save(&self.snapshot());
            }
            Cmd::LoadSession => self.restore_session(),
        }
        self.request_redraw();
    }
}
