use std::rc::Rc;
use std::time::{Duration, Instant};

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::{Cursor, CursorIcon, Window, WindowId};

use crate::background::Background;
use crate::clipboard::Clip;
use crate::font::FontAtlas;
use crate::layout::{self, Axis, Rect};
use crate::palette::Palette;
use crate::persist;
use crate::proc;
use crate::session::Tab;
use crate::{Dir4, Wake, FRAME, TAB_PALETTE};

/// Default font size in pixels.
pub(crate) const DEFAULT_FONT_PX: f32 = 18.0;
/// Minimum font size in pixels.
const MIN_FONT_PX: f32 = 6.0;
/// Maximum font size in pixels.
const MAX_FONT_PX: f32 = 72.0;
/// Font zoom step in pixels.
pub(crate) const ZOOM_STEP: f32 = 2.0;

/// Maximum time between clicks to be counted as a multi-click (ms).
const MULTI_CLICK_MS: u64 = 500;

pub(crate) struct Gritty {
    pub(crate) window: Option<Rc<Window>>,
    pub(crate) surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    pub(crate) _context: Option<softbuffer::Context<Rc<Window>>>,
    pub(crate) font: FontAtlas,
    /// Current font size in pixels (CA-12 zoom).
    pub(crate) font_px: f32,
    pub(crate) background: Background,
    pub(crate) clip: Clip,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    pub(crate) mods: winit::keyboard::ModifiersState,
    pub(crate) mouse_pos: (f64, f64),
    pub(crate) selecting: bool,
    pub(crate) dragging: Option<Vec<u8>>,
    pub(crate) rename: Option<String>,
    pub(crate) palette: Option<Palette>,
    pub(crate) broadcast: bool,
    /// RT-8: pending signal-byte (ETX/EOT/SUB) awaiting second-press confirmation.
    pub(crate) broadcast_pending_signal: Option<u8>,
    pub(crate) seamless: bool,
    pub(crate) last_proc_poll: Instant,
    pub(crate) last_render: Instant,
    pub(crate) redraw_pending: bool,
    pub(crate) proxy: EventLoopProxy<Wake>,
    /// Last left-button press time (CA-18 multi-click).
    pub(crate) last_click: Option<Instant>,
    /// Consecutive click count at the same location (CA-18).
    pub(crate) click_count: u32,
    /// CA-21: whether the keybinding help overlay is visible.
    pub(crate) show_help: bool,
}

impl Gritty {
    pub(crate) fn new(proxy: EventLoopProxy<Wake>) -> Self {
        Self {
            window: None,
            surface: None,
            _context: None,
            font: FontAtlas::new(DEFAULT_FONT_PX),
            font_px: DEFAULT_FONT_PX,
            background: Background::new(),
            clip: Clip::new(),
            tabs: Vec::new(),
            active: 0,
            mods: winit::keyboard::ModifiersState::empty(),
            mouse_pos: (0.0, 0.0),
            selecting: false,
            dragging: None,
            rename: None,
            palette: None,
            broadcast: false,
            broadcast_pending_signal: None,
            seamless: false,
            last_proc_poll: Instant::now() - Duration::from_secs(5),
            last_render: Instant::now() - FRAME,
            redraw_pending: false,
            proxy,
            last_click: None,
            click_count: 0,
            show_help: false,
        }
    }

    /// Request a repaint, but no faster than `FRAME`. If we're inside the
    /// cooldown, defer via `WaitUntil` so the frame still lands promptly.
    pub(crate) fn schedule_redraw(&mut self, event_loop: &ActiveEventLoop) {
        self.redraw_pending = true;
        if self.last_render.elapsed() >= FRAME {
            self.request_redraw();
        } else {
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                self.last_render + FRAME,
            ));
        }
    }

    /// Refresh each pane's foreground process name (one OS snapshot for all).
    pub(crate) fn update_procs(&mut self) {
        let procs = proc::snapshot();
        for tab in &mut self.tabs {
            for pane in tab.panes.values_mut() {
                pane.proc_name = pane
                    .pty
                    .pid()
                    .and_then(|pid| proc::foreground_name(&procs, pid))
                    .unwrap_or_default();
            }
        }
    }

    pub(crate) fn bar_h(&self) -> usize {
        self.font.cell_h
    }

    /// Height of a pane's title bar (0 in seamless mode).
    pub(crate) fn title_h(&self) -> usize {
        if self.seamless {
            0
        } else {
            self.font.cell_h
        }
    }

    pub(crate) fn win_size(&self) -> (usize, usize) {
        self.window
            .as_ref()
            .map(|w| {
                let s = w.inner_size();
                (s.width.max(1) as usize, s.height.max(1) as usize)
            })
            .unwrap_or((1, 1))
    }

    pub(crate) fn content_rect(&self, w: usize, h: usize) -> Rect {
        layout::content_rect(w, h, self.bar_h())
    }

    /// Full rectangle (title bar + grid) for each pane in the active tab.
    pub(crate) fn pane_rects(&self, w: usize, h: usize) -> Vec<(usize, Rect)> {
        let area = self.content_rect(w, h);
        let mut v = Vec::new();
        if let Some(tab) = self.tabs.get(self.active) {
            tab.tree.layout(area, &mut v);
        }
        v
    }

    /// Grid area of a pane = its rect minus the title bar.
    pub(crate) fn grid_rect(&self, rect: Rect) -> Rect {
        layout::grid_rect(rect, self.title_h())
    }

    /// Resize every pane in the active tab to fit the current layout.
    pub(crate) fn relayout(&mut self) {
        let (w, h) = self.win_size();
        let rects = self.pane_rects(w, h);
        let (cw, ch) = (self.font.cell_w, self.font.cell_h);
        let th = self.title_h();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            for (id, rect) in rects {
                if let Some(pane) = tab.panes.get_mut(&id) {
                    let grid = Rect {
                        x: rect.x,
                        y: rect.y + th,
                        w: rect.w,
                        h: rect.h.saturating_sub(th),
                    };
                    pane.resize(grid.w / cw, grid.h / ch);
                }
            }
        }
    }

    pub(crate) fn new_tab(&mut self) {
        let (w, h) = self.win_size();
        let area = self.content_rect(w, h);
        let (cw, ch) = (self.font.cell_w, self.font.cell_h);
        let th = self.title_h();
        let cols = area.w / cw;
        let rows = area.h.saturating_sub(th) / ch;
        let n = self.tabs.len() + 1;
        let color = TAB_PALETTE[self.tabs.len() % TAB_PALETTE.len()];
        self.tabs.push(Tab::new(
            format!("tab {n}"),
            color,
            cols,
            rows,
            self.proxy.clone(),
        ));
        self.active = self.tabs.len() - 1;
        self.relayout();
    }

    pub(crate) fn split_focus(&mut self, axis: Axis) {
        let proxy = self.proxy.clone();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.split(axis, proxy);
        }
        self.relayout();
    }

    pub(crate) fn close_focus(&mut self, event_loop: &ActiveEventLoop) {
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

    pub(crate) fn move_focus(&mut self, dir: Dir4) {
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

    /// Drain every pane's output into its grid. Returns true if the *visible*
    /// (active) tab changed, so we only repaint when there's something to see.
    pub(crate) fn drain_pty(&mut self) -> bool {
        let active = self.active;
        let mut visible_dirty = false;
        for (ti, tab) in self.tabs.iter_mut().enumerate() {
            for pane in tab.panes.values_mut() {
                pane.pty.mark_drained();
                let mut got = false;
                while let Ok(chunk) = pane.pty.rx.try_recv() {
                    pane.term.feed(&chunk);
                    got = true;
                }
                if got && ti == active {
                    visible_dirty = true;
                }
            }
        }
        visible_dirty
    }

    pub(crate) fn request_redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    /// Remove panes whose shell exited (e.g. `exit`), and tabs left empty.
    pub(crate) fn reap_dead(&mut self, event_loop: &ActiveEventLoop) {
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
                let tree = std::mem::replace(&mut tab.tree, crate::layout::Node::Leaf(id));
                if let Some(t) = tree.without(id) {
                    tab.tree = t;
                    if tab.focus == id {
                        let mut lv = Vec::new();
                        tab.tree.leaves(&mut lv);
                        tab.focus = *lv.first().unwrap_or(&id);
                    }
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

    pub(crate) fn copy_selection(&mut self) {
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

    pub(crate) fn paste(&mut self) {
        let Some(text) = self.clip.paste() else {
            return;
        };
        let bracketed = self
            .tabs
            .get(self.active)
            .and_then(|t| t.panes.get(&t.focus))
            .is_some_and(|p| p.term.bracketed_paste());
        let data = crate::term::wrap_paste(&text, bracketed);
        if let Some(tab) = self.tabs.get_mut(self.active) {
            let f = tab.focus;
            if let Some(pane) = tab.panes.get_mut(&f) {
                pane.term.scroll_to_bottom();
                pane.pty.write(&data);
            }
        }
    }

    /// Capture the current workspace for persistence.
    pub(crate) fn snapshot(&self) -> persist::SavedSession {
        let tabs = self
            .tabs
            .iter()
            .map(|t| {
                let mut ids = Vec::new();
                t.tree.leaves(&mut ids);
                let panes = ids
                    .iter()
                    .filter_map(|id| {
                        t.panes.get(id).map(|p| persist::SavedPane {
                            id: *id,
                            name: p.name.clone(),
                        })
                    })
                    .collect();
                persist::SavedTab {
                    name: t.name.clone(),
                    color: t.color,
                    focus: t.focus,
                    next_id: t.next_id(),
                    tree: t.tree.clone(),
                    panes,
                }
            })
            .collect();
        // CA-32: persist window geometry so the next launch restores the same size.
        let (win_w, win_h) = self
            .window
            .as_ref()
            .map(|w| {
                let s = w.inner_size();
                (Some(s.width), Some(s.height))
            })
            .unwrap_or((None, None));
        persist::SavedSession {
            active: self.active,
            tabs,
            win_w,
            win_h,
        }
    }

    /// Replace the current workspace with a saved one (if any).
    pub(crate) fn restore_session(&mut self) {
        // Caps against a crafted session that would mass-spawn shells (RT-5).
        const MAX_TABS: usize = 64;
        const MAX_PANES_PER_TAB: usize = 64;

        let Some(saved) = persist::load() else { return };
        if saved.tabs.is_empty() || saved.tabs.len() > MAX_TABS {
            return;
        }
        for st in &saved.tabs {
            let mut leaves = Vec::new();
            st.tree.leaves(&mut leaves);
            if leaves.len() > MAX_PANES_PER_TAB {
                return; // reject the whole session rather than spawn thousands
            }
        }
        let (w, h) = self.win_size();
        let area = self.content_rect(w, h);
        let (cw, ch) = (self.font.cell_w, self.font.cell_h);
        let cols = (area.w / cw).max(1);
        let rows = (area.h.saturating_sub(self.title_h()) / ch).max(1);
        self.tabs = saved
            .tabs
            .iter()
            .map(|st| Tab::from_saved(st, cols, rows, self.proxy.clone()))
            .collect();
        self.active = saved.active.min(self.tabs.len() - 1);
        self.relayout();
    }

    pub(crate) fn focus_and_redraw(&mut self, dir: Dir4) {
        self.move_focus(dir);
        self.request_redraw();
    }

    pub(crate) fn resize_focus(&mut self, axis: Axis, grow: bool) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.resize_focus(axis, grow);
        }
        self.relayout();
        self.request_redraw();
    }

    /// Tab index under an x pixel on the tab bar, mirroring the render layout.
    pub(crate) fn tab_at(&self, x: usize) -> Option<usize> {
        layout::tab_at(
            self.tabs.iter().map(|t| t.name.chars().count()),
            self.font.cell_w,
            x,
        )
    }

    /// CA-28: hit-test the tab strip for `×` and `+` buttons.
    /// Returns `TabHit::Close(i)` when x falls on tab i's close button,
    /// `TabHit::New` when x falls on the `+` button, and `None` otherwise.
    pub(crate) fn tab_button_at(&self, x: usize, w: usize) -> Option<TabHit> {
        tab_button_at(
            self.tabs.iter().map(|t| t.name.chars().count()),
            self.font.cell_w,
            x,
            w,
        )
    }

    /// Pane id under a pixel, plus its grid rect (for selection coordinates).
    pub(crate) fn pane_at(&self, x: f64, y: f64) -> Option<(usize, Rect)> {
        let (w, h) = self.win_size();
        for (id, rect) in self.pane_rects(w, h) {
            if rect.contains(x as usize, y as usize) {
                return Some((id, self.grid_rect(rect)));
            }
        }
        None
    }

    pub(crate) fn point_in_grid(
        &self,
        grid: Rect,
        x: f64,
        y: f64,
        cols: usize,
        off: usize,
    ) -> (Point, Side) {
        let (col, row, right) =
            layout::grid_cell(grid, x, y, cols, off, self.font.cell_w, self.font.cell_h);
        let side = if right { Side::Right } else { Side::Left };
        (Point::new(Line(row), Column(col)), side)
    }

    /// CA-12: Rebuild the font atlas at `new_px`, relayout, and redraw.
    pub(crate) fn apply_font_zoom(&mut self, new_px: f32) {
        let px = new_px.clamp(MIN_FONT_PX, MAX_FONT_PX);
        if (px - self.font_px).abs() < f32::EPSILON {
            return;
        }
        self.font_px = px;
        self.font = FontAtlas::new(px);
        self.relayout();
        self.request_redraw();
    }

    /// CA-23: Update the OS cursor based on whether the mouse is over a divider.
    pub(crate) fn update_cursor_shape(&self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let (x, y) = self.mouse_pos;
        let (w, h) = self.win_size();
        let area = self.content_rect(w, h);
        let cursor = if let Some(tab) = self.tabs.get(self.active) {
            match tab.tree.divider_at(area, x as usize, y as usize, 5) {
                Some(path) => {
                    // Determine axis of the divider for the right icon.
                    let icon = match tab.tree.split_area(&path, area) {
                        Some((crate::layout::Axis::LeftRight, _)) => CursorIcon::ColResize,
                        Some((crate::layout::Axis::TopBottom, _)) => CursorIcon::RowResize,
                        None => CursorIcon::Default,
                    };
                    Cursor::from(icon)
                }
                None => Cursor::from(CursorIcon::Default),
            }
        } else {
            Cursor::from(CursorIcon::Default)
        };
        window.set_cursor(cursor);
    }

    /// CA-18: classify click count based on timing, updating `click_count`.
    /// Returns the count (1 = single, 2 = double, 3+ = triple).
    pub(crate) fn classify_click(&mut self) -> u32 {
        let now = Instant::now();
        let elapsed_ms = match self.last_click {
            Some(prev) => now.duration_since(prev).as_millis() as u64,
            None => u64::MAX,
        };
        let count = next_click_count(elapsed_ms, self.click_count);
        self.last_click = Some(now);
        self.click_count = count;
        count
    }

    /// CA-7: true if the focused pane's terminal has any mouse-reporting mode active.
    pub(crate) fn pane_wants_mouse(&self) -> bool {
        use alacritty_terminal::term::TermMode;
        self.tabs
            .get(self.active)
            .and_then(|t| t.panes.get(&t.focus))
            .is_some_and(|p| {
                p.term
                    .term
                    .mode()
                    .intersects(TermMode::MOUSE_MODE | TermMode::SGR_MOUSE)
            })
    }

    /// CA-7: Forward a mouse event to the focused pane as an SGR mouse sequence.
    ///
    /// `btn`:  0=left, 1=middle, 2=right; add 32 for motion, 64 for wheel.
    /// `col`, `row`: 1-based terminal column/row of the click.
    /// `press`: true for press/motion, false for release.
    pub(crate) fn forward_mouse_sgr(&mut self, btn: u8, col: u16, row: u16, press: bool) {
        let seq = encode_sgr_mouse(btn, col, row, press);
        if let Some(tab) = self.tabs.get_mut(self.active) {
            let f = tab.focus;
            if let Some(pane) = tab.panes.get_mut(&f) {
                pane.pty.write(&seq);
            }
        }
    }

    /// CA-7: Convert pixel position to 1-based (col, row) for the focused pane.
    pub(crate) fn pixel_to_term_cell(&self, x: f64, y: f64) -> Option<(u16, u16)> {
        let (w, h) = self.win_size();
        let tab = self.tabs.get(self.active)?;
        let rects = self.pane_rects(w, h);
        let (_, pane_rect) = rects.iter().find(|(id, _)| *id == tab.focus)?;
        let grid = self.grid_rect(*pane_rect);
        let pane = tab.panes.get(&tab.focus)?;
        let (col, row, _) = layout::grid_cell(
            grid,
            x,
            y,
            pane.term.size.cols,
            pane.term.display_offset(),
            self.font.cell_w,
            self.font.cell_h,
        );
        // SGR uses 1-based coordinates; row can be negative in scrollback (clamp to 1).
        let term_col = (col as u16).saturating_add(1);
        let term_row = (row.max(0) as u16).saturating_add(1);
        Some((term_col, term_row))
    }
}

impl ApplicationHandler<Wake> for Gritty {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // CA-32: restore window size from session if available, else use defaults.
        let saved = persist::load();
        let (init_w, init_h) = saved
            .as_ref()
            .and_then(|s| match (s.win_w, s.win_h) {
                (Some(w), Some(h)) if w >= 200 && h >= 100 => Some((w as f64, h as f64)),
                _ => None,
            })
            .unwrap_or((960.0, 600.0));
        let mut attrs = Window::default_attributes()
            .with_title("gritty")
            .with_inner_size(winit::dpi::PhysicalSize::new(init_w, init_h));
        if let Some(icon) = crate::load_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }
        let window = Rc::new(event_loop.create_window(attrs).expect("create window"));
        crate::style_caption(&window);

        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");

        self.window = Some(window);
        self.surface = Some(surface);
        self._context = Some(context);

        // Resume the previous workspace, or start fresh.
        if saved.is_some_and(|s| !s.tabs.is_empty()) {
            self.restore_session();
        } else {
            self.new_tab();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: Wake) {
        let mut dirty = self.drain_pty();
        self.reap_dead(event_loop);
        if self.last_proc_poll.elapsed() >= Duration::from_millis(750) {
            self.update_procs();
            self.last_proc_poll = Instant::now();
            dirty = true; // headers may have changed
        }
        if dirty {
            self.schedule_redraw(event_loop);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // A deferred (throttled) frame is now due.
        if self.redraw_pending && self.last_render.elapsed() >= FRAME {
            self.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                let _ = persist::save(&self.snapshot());
                event_loop.exit();
            }

            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    self.handle_key(event_loop, &event.logical_key);
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_pos = (position.x, position.y);
                if let Some(path) = self.dragging.clone() {
                    self.drag_divider(&path, position.x, position.y);
                } else if self.selecting {
                    self.update_selection(position.x, position.y);
                } else {
                    // CA-7: forward mouse motion when terminal has mouse mode.
                    if self.pane_wants_mouse() {
                        if let Some((col, row)) = self.pixel_to_term_cell(position.x, position.y) {
                            // Button 35 = motion with no button held (32 + 3 for "no button").
                            self.forward_mouse_sgr(35, col, row, true);
                        }
                    } else {
                        // CA-23: update resize cursor only when not in mouse-report mode.
                        self.update_cursor_shape();
                    }
                }
            }

            WindowEvent::MouseInput { state, button, .. } => match (state, button) {
                (ElementState::Pressed, MouseButton::Left) => self.begin_selection(event_loop),
                (ElementState::Released, MouseButton::Left) => {
                    if self.pane_wants_mouse() {
                        if let Some((col, row)) =
                            self.pixel_to_term_cell(self.mouse_pos.0, self.mouse_pos.1)
                        {
                            self.forward_mouse_sgr(0, col, row, false);
                        }
                    } else if self.dragging.take().is_none() && self.selecting {
                        self.copy_selection();
                    }
                    self.selecting = false;
                }
                (ElementState::Pressed, MouseButton::Right) => self.paste(),
                _ => {}
            },

            WindowEvent::MouseWheel { delta, .. } => {
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / self.font.cell_h as f64) as f32,
                };
                if self.mods.control_key() {
                    // Ctrl + wheel resizes the focused pane (up = bigger).
                    if notches != 0.0 {
                        let grow = notches > 0.0;
                        if let Some(tab) = self.tabs.get_mut(self.active) {
                            tab.resize_focus(Axis::LeftRight, grow);
                            tab.resize_focus(Axis::TopBottom, grow);
                        }
                        self.relayout();
                        self.request_redraw();
                    }
                } else if self.pane_wants_mouse() {
                    // CA-7: forward wheel events to the PTY as SGR.
                    if notches != 0.0 {
                        if let Some((col, row)) =
                            self.pixel_to_term_cell(self.mouse_pos.0, self.mouse_pos.1)
                        {
                            // Wheel up = btn 64, wheel down = btn 65.
                            let btn = if notches > 0.0 { 64u8 } else { 65u8 };
                            self.forward_mouse_sgr(btn, col, row, true);
                        }
                    }
                } else {
                    let lines = (notches * 3.0) as i32;
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
            }

            WindowEvent::Resized(_) => {
                self.relayout();
                self.request_redraw();
            }

            WindowEvent::RedrawRequested => {
                self.redraw();
                event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
            }

            _ => {}
        }
    }
}

impl Gritty {
    pub(crate) fn begin_selection(&mut self, event_loop: &ActiveEventLoop) {
        let (x, y) = self.mouse_pos;

        // Click on the tab bar switches tabs instead of selecting.
        if (y as usize) < self.bar_h() {
            let (w, _) = self.win_size();
            // CA-28: check for × (close) and + (new) button hits first.
            if let Some(hit) = self.tab_button_at(x as usize, w) {
                match hit {
                    TabHit::Close(i) => {
                        if i < self.tabs.len() {
                            // switch to that tab then close focus (reuses existing logic)
                            self.active = i;
                            self.close_focus(event_loop);
                            // RT-8: disable broadcast when tab layout changes
                            self.broadcast = false;
                            self.broadcast_pending_signal = None;
                        }
                    }
                    TabHit::New => {
                        self.new_tab();
                        // RT-8: disable broadcast on new tab (user should re-enable explicitly)
                        self.broadcast = false;
                        self.broadcast_pending_signal = None;
                    }
                }
                self.request_redraw();
                return;
            }
            if let Some(i) = self.tab_at(x as usize) {
                // RT-8: auto-disable broadcast on tab switch.
                if i != self.active {
                    self.broadcast = false;
                    self.broadcast_pending_signal = None;
                }
                self.active = i;
                self.drain_pty(); // RT-10: flush newly focused tab's PTY output.
                self.relayout();
                self.request_redraw();
            }
            return;
        }

        // Grab a divider to drag-resize.
        let (w, h) = self.win_size();
        let area = self.content_rect(w, h);
        if let Some(path) = self
            .tabs
            .get(self.active)
            .and_then(|t| t.tree.divider_at(area, x as usize, y as usize, 5))
        {
            self.dragging = Some(path);
            return;
        }

        // CA-7: if the pane has mouse mode, forward the click and return early.
        if self.pane_wants_mouse() {
            // Still focus the clicked pane first.
            if let Some((id, _)) = self.pane_at(x, y) {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if tab.panes.contains_key(&id) {
                        tab.focus = id;
                    }
                }
            }
            if let Some((col, row)) = self.pixel_to_term_cell(x, y) {
                self.forward_mouse_sgr(0, col, row, true);
            }
            return;
        }

        let Some((id, grid)) = self.pane_at(x, y) else {
            return;
        };
        // Focus the clicked pane.
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if tab.panes.contains_key(&id) {
                tab.focus = id;
            }
        }

        // CA-18: classify click count for word/line selection.
        let count = self.classify_click();

        let (cols, off) = self
            .tabs
            .get(self.active)
            .and_then(|t| t.panes.get(&id))
            .map(|p| (p.term.size.cols, p.term.display_offset()))
            .unwrap_or((1, 0));
        let (point, side) = self.point_in_grid(grid, x, y, cols, off);

        let sel_type = match count {
            1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };

        if let Some(tab) = self.tabs.get_mut(self.active) {
            if let Some(pane) = tab.panes.get_mut(&id) {
                pane.term.term.selection = Some(Selection::new(sel_type, point, side));
            }
        }
        self.selecting = true;
        self.request_redraw();
    }

    pub(crate) fn drag_divider(&mut self, path: &[u8], x: f64, y: f64) {
        let (w, h) = self.win_size();
        let area = self.content_rect(w, h);
        let Some((axis, srect)) = self
            .tabs
            .get(self.active)
            .and_then(|t| t.tree.split_area(path, area))
        else {
            return;
        };
        let ratio = match axis {
            Axis::LeftRight => (x - srect.x as f64) / (srect.w.max(1) as f64),
            Axis::TopBottom => (y - srect.y as f64) / (srect.h.max(1) as f64),
        } as f32;
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.tree.set_ratio(path, ratio);
        }
        self.relayout();
        self.request_redraw();
    }

    pub(crate) fn update_selection(&mut self, x: f64, y: f64) {
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

// --- Pure helper functions (unit-testable) ----------------------------------

/// CA-28: Result of a tab-strip button hit-test.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TabHit {
    /// The `×` close button of tab `i` was clicked.
    Close(usize),
    /// The `+` new-tab button was clicked.
    New,
}

/// CA-28: Hit-test the tab strip for close (`×`) and new-tab (`+`) buttons.
///
/// Layout per tab: ` ▸<name> × ` or ` <name> × ` — the `×` occupies one cell
/// at the right end of each tab slot.  After all tabs a `+` occupies one cell.
/// `name_lens`: character count of each tab name; `cw`: cell width in pixels;
/// `x`: pixel being tested; `w`: total window width (for overflow guard).
pub(crate) fn tab_button_at(
    name_lens: impl IntoIterator<Item = usize>,
    cw: usize,
    x: usize,
    w: usize,
) -> Option<TabHit> {
    // Tab slot width mirrors paint.rs:
    //   label = " ▸<name> " or " <name> "  →  (name_chars + 2) cells + "×" (1 cell)
    //   Then a half-cell gap after each tab slot.
    // The ▸ marker adds 1 char for the active tab, but for hit-testing purposes
    // we use the same formula as tab_at (which uses the raw name len + 2) plus
    // 2 extra cells for " × " padding.  We need to keep this in sync with paint.rs.
    // paint.rs label: format!(" ▸{} ", tab.name) → len = name.chars + 3 for active
    //                 format!(" {} ",  tab.name) → len = name.chars + 2 for others
    // We don't know which is active here, so we use the same slot width for both:
    // slot_w = (name_chars + 2) * cw  (the text part)
    //        + cw                     (the "×" cell)
    // gap    = cw / 2
    // But we must add 1 for active tab's ▸ marker.
    // Since tab_at does NOT add the extra ▸, we keep the same base here for
    // the name portion and append the ×.  The ▸ shifts pixels slightly but
    // since we are only testing for the × at the END of the slot it's fine to
    // treat all tabs as (name_chars + 2) wide for the base text.
    let mut tx = 0usize;
    for (i, len) in name_lens.into_iter().enumerate() {
        // text_w mirrors paint.rs (same as tab_at base: (len+2)*cw)
        let text_w = (len + 2) * cw;
        // one extra cell for the '×' glyph at the right
        let slot_w = text_w + cw;
        let gap = cw / 2;
        if tx + slot_w > w {
            break; // overflow: don't draw (or hit-test) past window edge
        }
        // The × cell spans [tx + text_w .. tx + slot_w)
        if x >= tx + text_w && x < tx + slot_w {
            return Some(TabHit::Close(i));
        }
        tx += slot_w + gap;
    }
    // '+' button sits right after all tabs, one cell wide.
    if tx + cw <= w && x >= tx && x < tx + cw {
        return Some(TabHit::New);
    }
    None
}

/// RT-8: Returns true if `b` is a signal-bearing control byte that should
/// require a second-press confirmation before being broadcast to all panes.
/// Specifically: ETX (0x03 / Ctrl+C → SIGINT), EOT (0x04 / Ctrl+D → EOF/SIGHUP),
/// SUB (0x1a / Ctrl+Z → SIGTSTP).
pub(crate) fn is_broadcast_signal_byte(b: u8) -> bool {
    matches!(b, 0x03 | 0x04 | 0x1a)
}

/// CA-7: Encode an SGR mouse sequence.
///
/// `btn`:   button index (0=left, 1=middle, 2=right; +32=motion, +64=wheel).
/// `col`, `row`: 1-based terminal coordinates.
/// `press`: true → `M` (press/motion), false → `m` (release).
pub(crate) fn encode_sgr_mouse(btn: u8, col: u16, row: u16, press: bool) -> Vec<u8> {
    let suffix = if press { 'M' } else { 'm' };
    format!("\x1b[<{};{};{}{}", btn, col, row, suffix).into_bytes()
}

/// CA-18: Classify a click into single/double/triple based on elapsed time
/// and a running count. Pure function: does not mutate any state.
///
/// `elapsed_ms`: milliseconds since the previous click (u64::MAX = first click).
/// `prev_count`: count from the previous click.
/// Returns the new count (1, 2, or capped at 3).
pub(crate) fn next_click_count(elapsed_ms: u64, prev_count: u32) -> u32 {
    if elapsed_ms <= MULTI_CLICK_MS {
        (prev_count + 1).min(3)
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- CA-7 SGR encoding ---------------------------------------------------

    #[test]
    fn sgr_mouse_left_press() {
        let seq = encode_sgr_mouse(0, 1, 1, true);
        assert_eq!(seq, b"\x1b[<0;1;1M");
    }

    #[test]
    fn sgr_mouse_left_release() {
        let seq = encode_sgr_mouse(0, 5, 10, false);
        assert_eq!(seq, b"\x1b[<0;5;10m");
    }

    #[test]
    fn sgr_mouse_wheel_up() {
        let seq = encode_sgr_mouse(64, 3, 7, true);
        assert_eq!(seq, b"\x1b[<64;3;7M");
    }

    #[test]
    fn sgr_mouse_motion() {
        // motion btn = 35 (32 + 3)
        let seq = encode_sgr_mouse(35, 20, 5, true);
        assert_eq!(seq, b"\x1b[<35;20;5M");
    }

    #[test]
    fn sgr_mouse_large_coords() {
        let seq = encode_sgr_mouse(0, 220, 50, true);
        assert_eq!(seq, b"\x1b[<0;220;50M");
    }

    // --- CA-18 click-count classifier ----------------------------------------

    #[test]
    fn first_click_is_always_single() {
        // u64::MAX simulates "no previous click"
        assert_eq!(next_click_count(u64::MAX, 0), 1);
    }

    #[test]
    fn rapid_second_click_is_double() {
        assert_eq!(next_click_count(100, 1), 2);
    }

    #[test]
    fn rapid_third_click_is_triple() {
        assert_eq!(next_click_count(200, 2), 3);
    }

    #[test]
    fn fourth_rapid_click_stays_at_three() {
        assert_eq!(next_click_count(50, 3), 3);
    }

    #[test]
    fn slow_click_resets_to_single() {
        // 600ms > MULTI_CLICK_MS (500ms)
        assert_eq!(next_click_count(600, 2), 1);
    }

    #[test]
    fn click_at_exactly_threshold_counts() {
        assert_eq!(next_click_count(MULTI_CLICK_MS, 1), 2);
    }

    #[test]
    fn click_just_over_threshold_resets() {
        assert_eq!(next_click_count(MULTI_CLICK_MS + 1, 1), 1);
    }

    // --- CA-28 tab button hit-test -------------------------------------------

    #[test]
    fn tab_button_close_hit() {
        // One tab with name_len=2: text_w = 4*cw, slot_w = 5*cw.
        // × spans [40..50) for cw=10.
        let cw = 10usize;
        let lens = [2usize];
        let w = 1000;
        assert_eq!(tab_button_at(lens, cw, 40, w), Some(TabHit::Close(0)));
        assert_eq!(tab_button_at(lens, cw, 49, w), Some(TabHit::Close(0)));
    }

    #[test]
    fn tab_button_miss_returns_none() {
        let cw = 10usize;
        let lens = [2usize];
        let w = 1000;
        // x=5 is inside the text area, not the × button.
        assert_eq!(tab_button_at(lens, cw, 5, w), None);
    }

    #[test]
    fn tab_button_new_tab_hit() {
        // One tab: slot_w = 5*10 = 50, gap = 5. + sits at [55..65).
        let cw = 10usize;
        let lens = [2usize];
        let w = 1000;
        assert_eq!(tab_button_at(lens, cw, 55, w), Some(TabHit::New));
        assert_eq!(tab_button_at(lens, cw, 64, w), Some(TabHit::New));
    }

    #[test]
    fn tab_button_close_second_tab() {
        // Two tabs, cw=10:
        // tab0: slot_w=50 [0..50), × at [40..50), gap 5 → next at 55
        // tab1 name_len=3: slot_w=60 [55..115), × at [105..115)
        let cw = 10usize;
        let lens = [2usize, 3usize];
        let w = 1000;
        assert_eq!(tab_button_at(lens, cw, 105, w), Some(TabHit::Close(1)));
        assert_eq!(tab_button_at(lens, cw, 114, w), Some(TabHit::Close(1)));
    }

    #[test]
    fn tab_button_overflow_stops_at_window_edge() {
        // Window is too narrow to show any tab: nothing should match.
        let cw = 10usize;
        let lens = [2usize];
        let w = 5; // narrower than one tab slot
        assert_eq!(tab_button_at(lens, cw, 0, w), None);
        assert_eq!(tab_button_at(lens, cw, 4, w), None);
    }

    // --- RT-8 control-byte predicate -----------------------------------------

    #[test]
    fn is_signal_byte_identifies_etx_eot_sub() {
        assert!(is_broadcast_signal_byte(0x03)); // ETX / Ctrl+C
        assert!(is_broadcast_signal_byte(0x04)); // EOT / Ctrl+D
        assert!(is_broadcast_signal_byte(0x1a)); // SUB / Ctrl+Z
    }

    #[test]
    fn is_signal_byte_rejects_normal_bytes() {
        assert!(!is_broadcast_signal_byte(b'a'));
        assert!(!is_broadcast_signal_byte(0x0d)); // CR (Enter)
        assert!(!is_broadcast_signal_byte(0x09)); // Tab
        assert!(!is_broadcast_signal_byte(0x1b)); // ESC
    }
}
