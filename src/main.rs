// gritty — a lightweight, standalone native Windows terminal.
// Multiplexer: tabs + split panes with per-pane names, scrollback, copy/paste.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod background;
mod clipboard;
mod color;
mod font;
mod key;
mod layout;
mod pty;
mod render;
mod session;
mod term;

use std::num::NonZeroU32;
use std::rc::Rc;

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use background::Background;
use clipboard::Clip;
use color::{ACCENT, BG, FG, SELECTION_BG, UI_BAR_BG, UI_DIM, UI_TITLE_BG};
use font::FontAtlas;
use layout::{Axis, Node, Rect};
use render::{draw_cell, draw_text, fill_rect, stroke_rect, Cell};
use session::Tab;

#[derive(Debug, Clone, Copy)]
struct Wake;

/// Edgy neon accents — each new tab takes the next one.
const TAB_PALETTE: [u32; 6] = [
    0x00ff_3d9a, // pink
    0x003d_f0ff, // cyan
    0x004d_ff88, // green
    0x00ff_a23d, // orange
    0x00b4_5cff, // purple
    0x00ff_e04d, // yellow
];

#[derive(Clone, Copy)]
enum Dir4 {
    Left,
    Right,
    Up,
    Down,
}

struct Gritty {
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    _context: Option<softbuffer::Context<Rc<Window>>>,
    font: FontAtlas,
    background: Background,
    clip: Clip,
    tabs: Vec<Tab>,
    active: usize,
    mods: ModifiersState,
    mouse_pos: (f64, f64),
    selecting: bool,
    rename: Option<String>,
    proxy: EventLoopProxy<Wake>,
}

impl Gritty {
    fn new(proxy: EventLoopProxy<Wake>) -> Self {
        Self {
            window: None,
            surface: None,
            _context: None,
            font: FontAtlas::new(18.0),
            background: Background::new(),
            clip: Clip::new(),
            tabs: Vec::new(),
            active: 0,
            mods: ModifiersState::empty(),
            mouse_pos: (0.0, 0.0),
            selecting: false,
            rename: None,
            proxy,
        }
    }

    fn bar_h(&self) -> usize {
        self.font.cell_h
    }

    fn win_size(&self) -> (usize, usize) {
        self.window
            .as_ref()
            .map(|w| {
                let s = w.inner_size();
                (s.width.max(1) as usize, s.height.max(1) as usize)
            })
            .unwrap_or((1, 1))
    }

    fn content_rect(&self, w: usize, h: usize) -> Rect {
        let bar = self.bar_h();
        Rect { x: 0, y: bar, w, h: h.saturating_sub(bar) }
    }

    /// Full rectangle (title bar + grid) for each pane in the active tab.
    fn pane_rects(&self, w: usize, h: usize) -> Vec<(usize, Rect)> {
        let area = self.content_rect(w, h);
        let mut v = Vec::new();
        if let Some(tab) = self.tabs.get(self.active) {
            tab.tree.layout(area, &mut v);
        }
        v
    }

    /// Grid area of a pane = its rect minus the one-row title bar.
    fn grid_rect(&self, rect: Rect) -> Rect {
        let t = self.font.cell_h;
        Rect { x: rect.x, y: rect.y + t, w: rect.w, h: rect.h.saturating_sub(t) }
    }

    /// Resize every pane in the active tab to fit the current layout.
    fn relayout(&mut self) {
        let (w, h) = self.win_size();
        let rects = self.pane_rects(w, h);
        let (cw, ch) = (self.font.cell_w, self.font.cell_h);
        if let Some(tab) = self.tabs.get_mut(self.active) {
            for (id, rect) in rects {
                if let Some(pane) = tab.panes.get_mut(&id) {
                    let grid = Rect { x: rect.x, y: rect.y + ch, w: rect.w, h: rect.h.saturating_sub(ch) };
                    pane.resize(grid.w / cw, grid.h / ch);
                }
            }
        }
    }

    fn new_tab(&mut self) {
        let (w, h) = self.win_size();
        let area = self.content_rect(w, h);
        let (cw, ch) = (self.font.cell_w, self.font.cell_h);
        let cols = area.w / cw;
        let rows = area.h.saturating_sub(ch) / ch;
        let n = self.tabs.len() + 1;
        let color = TAB_PALETTE[self.tabs.len() % TAB_PALETTE.len()];
        self.tabs
            .push(Tab::new(format!("tab {n}"), color, cols, rows, self.proxy.clone()));
        self.active = self.tabs.len() - 1;
        self.relayout();
    }

    fn split_focus(&mut self, axis: Axis) {
        let proxy = self.proxy.clone();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.split(axis, proxy);
        }
        self.relayout();
    }

    fn close_focus(&mut self, event_loop: &ActiveEventLoop) {
        let empty = self
            .tabs
            .get_mut(self.active)
            .map(|t| t.close_focus())
            .unwrap_or(false);
        if empty {
            self.tabs.remove(self.active);
            if self.tabs.is_empty() {
                event_loop.exit();
                return;
            }
            self.active = self.active.min(self.tabs.len() - 1);
        }
        self.relayout();
    }

    fn move_focus(&mut self, dir: Dir4) {
        let (w, h) = self.win_size();
        let rects = self.pane_rects(w, h);
        let focus = match self.tabs.get(self.active) {
            Some(t) => t.focus,
            None => return,
        };
        let Some(cur) = rects.iter().find(|(id, _)| *id == focus).map(|(_, r)| *r) else {
            return;
        };
        let (cx, cy) = cur.center();
        let mut best: Option<usize> = None;
        let mut best_d = u64::MAX;
        for (id, r) in &rects {
            if *id == focus {
                continue;
            }
            let (rx, ry) = r.center();
            let ok = match dir {
                Dir4::Left => rx < cx,
                Dir4::Right => rx > cx,
                Dir4::Up => ry < cy,
                Dir4::Down => ry > cy,
            };
            if !ok {
                continue;
            }
            let dx = rx as i64 - cx as i64;
            let dy = ry as i64 - cy as i64;
            let d = (dx * dx + dy * dy) as u64;
            if d < best_d {
                best_d = d;
                best = Some(*id);
            }
        }
        if let (Some(id), Some(tab)) = (best, self.tabs.get_mut(self.active)) {
            tab.focus = id;
        }
    }

    fn drain_pty(&mut self) {
        for tab in &mut self.tabs {
            for pane in tab.panes.values_mut() {
                while let Ok(chunk) = pane.pty.rx.try_recv() {
                    pane.term.feed(&chunk);
                }
            }
        }
        self.request_redraw();
    }

    fn request_redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    /// Remove panes whose shell exited (e.g. `exit`), and tabs left empty.
    fn reap_dead(&mut self, event_loop: &ActiveEventLoop) {
        let mut changed = false;
        let mut ti = 0;
        while ti < self.tabs.len() {
            let dead: Vec<usize> = self.tabs[ti]
                .panes
                .iter()
                .filter(|(_, p)| !p.pty.is_alive())
                .map(|(id, _)| *id)
                .collect();
            for id in dead {
                changed = true;
                let tab = &mut self.tabs[ti];
                let tree = std::mem::replace(&mut tab.tree, Node::Leaf(id));
                match tree.without(id) {
                    Some(t) => {
                        tab.tree = t;
                        if tab.focus == id {
                            let mut lv = Vec::new();
                            tab.tree.leaves(&mut lv);
                            tab.focus = *lv.first().unwrap_or(&id);
                        }
                    }
                    None => {}
                }
                tab.panes.remove(&id);
            }
            if self.tabs[ti].panes.is_empty() {
                self.tabs.remove(ti);
                changed = true;
            } else {
                ti += 1;
            }
        }
        if changed {
            if self.tabs.is_empty() {
                event_loop.exit();
                return;
            }
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len() - 1;
            }
            self.relayout();
            self.request_redraw();
        }
    }

    // --- clipboard, scoped to the focused pane of the active tab ------------

    fn copy_selection(&mut self) {
        let text = self
            .tabs
            .get(self.active)
            .and_then(|t| t.panes.get(&t.focus))
            .and_then(|p| p.term.term.selection_to_string());
        if let Some(text) = text {
            if !text.is_empty() {
                self.clip.copy(&text);
            }
        }
    }

    fn paste(&mut self) {
        let Some(text) = self.clip.paste() else { return };
        let bracketed = self
            .tabs
            .get(self.active)
            .and_then(|t| t.panes.get(&t.focus))
            .map_or(false, |p| p.term.bracketed_paste());
        let data = term::wrap_paste(&text, bracketed);
        if let Some(tab) = self.tabs.get_mut(self.active) {
            let f = tab.focus;
            if let Some(pane) = tab.panes.get_mut(&f) {
                pane.term.scroll_to_bottom();
                pane.pty.write(&data);
            }
        }
    }

    // --- input -------------------------------------------------------------

    fn handle_key(&mut self, event_loop: &ActiveEventLoop, key: &Key) {
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
                Key::Named(NamedKey::ArrowRight) => return self.resize_focus(Axis::LeftRight, true),
                Key::Named(NamedKey::ArrowLeft) => return self.resize_focus(Axis::LeftRight, false),
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

        // Default: send to the focused pane.
        if let Some(bytes) = key::encode(key, self.mods) {
            if let Some(tab) = self.tabs.get_mut(self.active) {
                let f = tab.focus;
                if let Some(pane) = tab.panes.get_mut(&f) {
                    pane.term.scroll_to_bottom();
                    pane.pty.write(&bytes);
                }
            }
        }
    }

    fn focus_and_redraw(&mut self, dir: Dir4) {
        self.move_focus(dir);
        self.request_redraw();
    }

    fn resize_focus(&mut self, axis: Axis, grow: bool) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.resize_focus(axis, grow);
        }
        self.relayout();
        self.request_redraw();
    }

    /// Tab index under an x pixel on the tab bar, mirroring the render layout.
    fn tab_at(&self, x: usize) -> Option<usize> {
        let cw = self.font.cell_w;
        let mut tx = 0usize;
        for (i, tab) in self.tabs.iter().enumerate() {
            let tw = (tab.name.chars().count() + 2) * cw;
            if x >= tx && x < tx + tw {
                return Some(i);
            }
            tx += tw + cw / 2;
        }
        None
    }

    /// Pane id under a pixel, plus its grid rect (for selection coordinates).
    fn pane_at(&self, x: f64, y: f64) -> Option<(usize, Rect)> {
        let (w, h) = self.win_size();
        for (id, rect) in self.pane_rects(w, h) {
            if rect.contains(x as usize, y as usize) {
                return Some((id, self.grid_rect(rect)));
            }
        }
        None
    }

    fn point_in_grid(&self, grid: Rect, x: f64, y: f64, cols: usize, off: usize) -> (Point, Side) {
        let cw = self.font.cell_w as f64;
        let ch = self.font.cell_h as f64;
        let rel_x = (x - grid.x as f64).max(0.0);
        let rel_y = (y - grid.y as f64).max(0.0);
        let col = ((rel_x / cw).floor() as usize).min(cols.saturating_sub(1));
        let row = (rel_y / ch).floor() as i32;
        let side = if (rel_x % cw) < cw / 2.0 { Side::Left } else { Side::Right };
        (Point::new(Line(row - off as i32), Column(col)), side)
    }

    // --- rendering ---------------------------------------------------------

    fn redraw(&mut self) {
        let (w, h) = self.win_size();
        let stride = w;
        let height = h;

        let Some(surface) = self.surface.as_mut() else { return };
        surface
            .resize(NonZeroU32::new(w as u32).unwrap(), NonZeroU32::new(h as u32).unwrap())
            .expect("resize");
        let mut buffer = surface.buffer_mut().expect("buffer");

        self.background.resize(stride, height);
        buffer.copy_from_slice(&self.background.px);

        let (cw, ch) = (self.font.cell_w, self.font.cell_h);
        let active = self.active;

        // Tab bar.
        fill_rect(&mut buffer, stride, Rect { x: 0, y: 0, w: stride, h: ch }, UI_BAR_BG);
        let mut tx = 0usize;
        for (i, tab) in self.tabs.iter().enumerate() {
            let label = format!(" {} ", tab.name);
            let tw = label.chars().count() * cw;
            let (fg, bg) = if i == active { (BG, tab.color) } else { (tab.color, UI_BAR_BG) };
            let r = Rect { x: tx, y: 0, w: tw, h: ch };
            fill_rect(&mut buffer, stride, r, bg);
            draw_text(&mut buffer, stride, &mut self.font, tx, 0, &label, fg, bg, true, r);
            tx += tw + cw / 2;
        }

        let accent = self.tabs.get(active).map(|t| t.color).unwrap_or(ACCENT);

        // Panes.
        let area = Rect { x: 0, y: ch, w: stride, h: height.saturating_sub(ch) };
        let mut rects = Vec::new();
        let focus = self.tabs.get(active).map(|t| t.focus).unwrap_or(0);
        if let Some(tab) = self.tabs.get(active) {
            tab.tree.layout(area, &mut rects);
        }

        for (id, rect) in &rects {
            let id = *id;
            let rect = *rect;
            let is_focus = id == focus;

            // Pane title bar.
            let title_rect = Rect { x: rect.x, y: rect.y, w: rect.w, h: ch };
            let (tfg, tbg) = if is_focus { (BG, accent) } else { (UI_DIM, UI_TITLE_BG) };
            fill_rect(&mut buffer, stride, title_rect, tbg);
            let name = self
                .tabs
                .get(active)
                .and_then(|t| t.panes.get(&id))
                .map(|p| p.name.clone())
                .unwrap_or_default();
            draw_text(&mut buffer, stride, &mut self.font, rect.x + cw / 2, rect.y, &name, tfg, tbg, true, title_rect);

            // Grid.
            let grid = Rect { x: rect.x, y: rect.y + ch, w: rect.w, h: rect.h.saturating_sub(ch) };
            if let Some(pane) = self.tabs.get(active).and_then(|t| t.panes.get(&id)) {
                draw_pane_grid(&mut buffer, stride, &mut self.font, pane, grid, is_focus, accent);
            }

            if is_focus {
                stroke_rect(&mut buffer, stride, rect, accent);
            }
        }

        // Rename overlay.
        if let Some(buf_str) = self.rename.clone() {
            let line = format!(" rename pane: {buf_str}_ ");
            let r = Rect { x: 0, y: height.saturating_sub(ch), w: stride, h: ch };
            fill_rect(&mut buffer, stride, r, ACCENT);
            draw_text(&mut buffer, stride, &mut self.font, 0, r.y, &line, BG, ACCENT, true, r);
        }

        buffer.present().expect("present");
    }

}

#[allow(clippy::too_many_arguments)]
fn draw_pane_grid(
    buffer: &mut [u32],
    stride: usize,
    font: &mut FontAtlas,
    pane: &session::Pane,
    grid: Rect,
    is_focus: bool,
    accent: u32,
) {
    let (cw, ch) = (font.cell_w, font.cell_h);
    let content = pane.term.term.renderable_content();
    let selection = content.selection;
    let at_bottom = content.display_offset == 0;
    let cursor_visible = is_focus && at_bottom && content.cursor.shape != CursorShape::Hidden;
    let cur_row = content.cursor.point.line.0;
    let cur_col = content.cursor.point.column.0 as i32;

    for item in content.display_iter {
        let line = item.point.line.0;
        if line < 0 {
            continue;
        }
        let row = line as usize;
        let col = item.point.column.0;
        let cell = item.cell;

        let mut fg = color::to_rgb(cell.fg, FG);
        let mut bg = color::to_rgb(cell.bg, BG);
        let is_default_bg = matches!(cell.bg, Color::Named(NamedColor::Background));
        let mut fill_bg = !is_default_bg;
        if selection.map_or(false, |r| r.contains(item.point)) {
            bg = SELECTION_BG;
            fill_bg = true;
        } else if cursor_visible && line == cur_row && col as i32 == cur_col {
            bg = accent;
            fg = BG;
            fill_bg = true;
        }

        let px = grid.x + col * cw;
        let py = grid.y + row * ch;
        draw_cell(buffer, stride, font, px, py, Cell { ch: cell.c, fg, bg }, fill_bg, grid);
    }
}

impl ApplicationHandler<Wake> for Gritty {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title("gritty")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 600.0));
        if let Some(icon) = load_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }
        let window = Rc::new(event_loop.create_window(attrs).expect("create window"));

        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface = softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");

        self.window = Some(window);
        self.surface = Some(surface);
        self._context = Some(context);

        self.new_tab();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: Wake) {
        self.drain_pty();
        self.reap_dead(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    self.handle_key(event_loop, &event.logical_key);
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_pos = (position.x, position.y);
                if self.selecting {
                    self.update_selection(position.x, position.y);
                }
            }

            WindowEvent::MouseInput { state, button, .. } => match (state, button) {
                (ElementState::Pressed, MouseButton::Left) => self.begin_selection(),
                (ElementState::Released, MouseButton::Left) => {
                    self.selecting = false;
                    self.copy_selection();
                }
                (ElementState::Pressed, MouseButton::Right) => self.paste(),
                _ => {}
            },

            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (y * 3.0) as i32,
                    MouseScrollDelta::PixelDelta(p) => (p.y / self.font.cell_h as f64) as i32,
                };
                if lines != 0 {
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        let f = tab.focus;
                        if let Some(pane) = tab.panes.get_mut(&f) {
                            pane.term.scroll(lines);
                        }
                    }
                    self.request_redraw();
                }
            }

            WindowEvent::Resized(_) => {
                self.relayout();
                self.request_redraw();
            }

            WindowEvent::RedrawRequested => self.redraw(),

            _ => {}
        }
    }
}

impl Gritty {
    fn begin_selection(&mut self) {
        let (x, y) = self.mouse_pos;

        // Click on the tab bar switches tabs instead of selecting.
        if (y as usize) < self.bar_h() {
            if let Some(i) = self.tab_at(x as usize) {
                self.active = i;
                self.relayout();
                self.request_redraw();
            }
            return;
        }

        let Some((id, grid)) = self.pane_at(x, y) else { return };
        // Focus the clicked pane.
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if tab.panes.contains_key(&id) {
                tab.focus = id;
            }
        }
        let (cols, off) = self
            .tabs
            .get(self.active)
            .and_then(|t| t.panes.get(&id))
            .map(|p| (p.term.size.cols, p.term.display_offset()))
            .unwrap_or((1, 0));
        let (point, side) = self.point_in_grid(grid, x, y, cols, off);
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if let Some(pane) = tab.panes.get_mut(&id) {
                pane.term.term.selection = Some(Selection::new(SelectionType::Simple, point, side));
            }
        }
        self.selecting = true;
        self.request_redraw();
    }

    fn update_selection(&mut self, x: f64, y: f64) {
        let focus = match self.tabs.get(self.active) {
            Some(t) => t.focus,
            None => return,
        };
        let (w, h) = self.win_size();
        let grid = self
            .pane_rects(w, h)
            .into_iter()
            .find(|(id, _)| *id == focus)
            .map(|(_, r)| self.grid_rect(r));
        let Some(grid) = grid else { return };
        let (cols, off) = self
            .tabs
            .get(self.active)
            .and_then(|t| t.panes.get(&focus))
            .map(|p| (p.term.size.cols, p.term.display_offset()))
            .unwrap_or((1, 0));
        let (point, side) = self.point_in_grid(grid, x, y, cols, off);
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if let Some(pane) = tab.panes.get_mut(&focus) {
                if let Some(sel) = pane.term.term.selection.as_mut() {
                    sel.update(point, side);
                }
            }
        }
        self.request_redraw();
    }
}

/// Window/taskbar icon, baked from grittyicon.png at build time (64x64 RGBA).
fn load_icon() -> Option<winit::window::Icon> {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/icon_rgba.bin"));
    winit::window::Icon::from_rgba(bytes.to_vec(), 64, 64).ok()
}

fn main() {
    let event_loop = EventLoop::<Wake>::with_user_event().build().expect("event loop");
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = Gritty::new(proxy);
    event_loop.run_app(&mut app).expect("run");
}
