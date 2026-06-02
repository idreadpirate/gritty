use std::rc::Rc;
use std::time::{Duration, Instant};

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::background::Background;
use crate::clipboard::Clip;
use crate::font::FontAtlas;
use crate::layout::{self, Axis, Rect};
use crate::palette::Palette;
use crate::persist;
use crate::proc;
use crate::session::Tab;
use crate::{Dir4, Wake, FRAME, TAB_PALETTE};

pub(crate) struct Gritty {
    pub(crate) window: Option<Rc<Window>>,
    pub(crate) surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    pub(crate) _context: Option<softbuffer::Context<Rc<Window>>>,
    pub(crate) font: FontAtlas,
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
    pub(crate) seamless: bool,
    pub(crate) last_proc_poll: Instant,
    pub(crate) last_render: Instant,
    pub(crate) redraw_pending: bool,
    pub(crate) proxy: EventLoopProxy<Wake>,
}

impl Gritty {
    pub(crate) fn new(proxy: EventLoopProxy<Wake>) -> Self {
        Self {
            window: None,
            surface: None,
            _context: None,
            font: FontAtlas::new(18.0),
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
            seamless: false,
            last_proc_poll: Instant::now() - Duration::from_secs(5),
            last_render: Instant::now() - FRAME,
            redraw_pending: false,
            proxy,
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
        persist::SavedSession {
            active: self.active,
            tabs,
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
}

impl ApplicationHandler<Wake> for Gritty {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title("gritty")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 600.0));
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
        if persist::load().is_some_and(|s| !s.tabs.is_empty()) {
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
                }
            }

            WindowEvent::MouseInput { state, button, .. } => match (state, button) {
                (ElementState::Pressed, MouseButton::Left) => self.begin_selection(),
                (ElementState::Released, MouseButton::Left) => {
                    if self.dragging.take().is_none() && self.selecting {
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
    pub(crate) fn begin_selection(&mut self) {
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

        let Some((id, grid)) = self.pane_at(x, y) else {
            return;
        };
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
