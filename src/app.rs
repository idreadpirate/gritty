use std::rc::Rc;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions;
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
use crate::persist::{self, SavedWindow};
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

/// Cap on simultaneously-restored windows (mirrors the per-window tab cap; guards
/// against a crafted session mass-spawning OS windows). Also the runtime ceiling
/// on interactively-created OS windows (RT-137).
const MAX_WINDOWS: usize = 16;
/// Cap on tabs per window — enforced both when restoring a saved session and on
/// the interactive new-tab path (RT-137).
const MAX_TABS: usize = 64;
/// Cap on panes (tree leaves) per tab — enforced both when restoring and on the
/// interactive split path (RT-137).
const MAX_PANES_PER_TAB: usize = 64;
/// Aggregate cap on the total number of panes restored across ALL windows in one
/// `restore_windows` call (RT-26). The per-window/per-tab caps bound each entry
/// independently, but a crafted `session.json` can still encode their *product*
/// (16 windows × 64 tabs × 64 leaves ≈ 64k shells); `Tab::from_saved` spawns one
/// real shell per leaf synchronously on the UI thread, so the product must also
/// be bounded or startup mass-spawns shells and freezes.
const MAX_RESTORED_PANES: usize = 256;

/// A tab being dragged on the bar. If released outside the window it tears off
/// into its own OS window (`armed` flips true once the pointer leaves the bounds).
pub(crate) struct TabDrag {
    pub(crate) index: usize,
    pub(crate) armed: bool,
}

/// One OS window: its render surface, decorative background, tab set, and all
/// per-window interaction state. `Gritty` owns a list of these so tabs can be
/// torn off onto other screens.
pub(crate) struct Win {
    pub(crate) window: Rc<Window>,
    pub(crate) surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    pub(crate) _context: softbuffer::Context<Rc<Window>>,
    pub(crate) background: Background,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    pub(crate) mouse_pos: (f64, f64),
    pub(crate) selecting: bool,
    pub(crate) dragging: Option<Vec<u8>>,
    /// Tab tear-off drag in progress (set on a tab-bar press).
    pub(crate) tab_drag: Option<TabDrag>,
    pub(crate) rename: Option<String>,
    /// While a rename prompt is open: true = renaming the active tab, false = pane.
    pub(crate) rename_is_tab: bool,
    pub(crate) palette: Option<Palette>,
    pub(crate) broadcast: bool,
    /// RT-8: pending signal-byte (ETX/EOT/SUB) awaiting second-press confirmation.
    pub(crate) broadcast_pending_signal: Option<u8>,
    pub(crate) seamless: bool,
    /// CA-21: whether the keybinding help overlay is visible.
    pub(crate) show_help: bool,
    pub(crate) last_render: Instant,
    pub(crate) redraw_pending: bool,
    /// Last left-button press time (CA-18 multi-click).
    pub(crate) last_click: Option<Instant>,
    /// Consecutive click count at the same location (CA-18).
    pub(crate) click_count: u32,
    /// CA-62/CA-82: pixel position of the last left press, so a multi-click is
    /// only counted when the pointer stayed at (about) the same cell.
    pub(crate) last_click_pos: Option<(f64, f64)>,
}

pub(crate) struct Gritty {
    /// One entry per OS window. `focused` indexes the window with keyboard focus.
    pub(crate) windows: Vec<Win>,
    pub(crate) focused: usize,
    pub(crate) font: FontAtlas,
    /// Current font size in pixels (CA-12 zoom). Shared across windows.
    pub(crate) font_px: f32,
    pub(crate) clip: Clip,
    pub(crate) mods: winit::keyboard::ModifiersState,
    pub(crate) last_proc_poll: Instant,
    pub(crate) proxy: EventLoopProxy<Wake>,
}

impl Gritty {
    pub(crate) fn new(proxy: EventLoopProxy<Wake>) -> Self {
        Self {
            windows: Vec::new(),
            focused: 0,
            font: FontAtlas::new(DEFAULT_FONT_PX),
            font_px: DEFAULT_FONT_PX,
            clip: Clip::new(),
            mods: winit::keyboard::ModifiersState::empty(),
            last_proc_poll: Instant::now() - Duration::from_secs(5),
            proxy,
        }
    }

    /// Index of the window owning `id`, if any.
    pub(crate) fn idx_of(&self, id: WindowId) -> Option<usize> {
        self.windows.iter().position(|w| w.window.id() == id)
    }

    /// Create a fresh OS window (no tabs yet) with our icon and dark caption.
    /// `size` is physical pixels; `pos` places the top-left if given.
    /// Returns `None` if the OS refuses to create the window (tear-off stays
    /// graceful instead of crashing).
    fn spawn_window(
        &self,
        event_loop: &ActiveEventLoop,
        size: (f64, f64),
        pos: Option<(i32, i32)>,
        seamless: bool,
    ) -> Option<Win> {
        let mut attrs = Window::default_attributes()
            .with_title("gritty")
            .with_inner_size(winit::dpi::PhysicalSize::new(size.0, size.1));
        if let Some((x, y)) = pos {
            attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(x, y));
        }
        if let Some(icon) = crate::load_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }
        let window = Rc::new(event_loop.create_window(attrs).ok()?);
        crate::style_caption(&window);
        let context = softbuffer::Context::new(window.clone()).ok()?;
        let surface = softbuffer::Surface::new(&context, window.clone()).ok()?;
        Some(Win {
            window,
            surface,
            _context: context,
            background: Background::new(),
            tabs: Vec::new(),
            active: 0,
            mouse_pos: (0.0, 0.0),
            selecting: false,
            dragging: None,
            tab_drag: None,
            rename: None,
            rename_is_tab: false,
            palette: None,
            broadcast: false,
            broadcast_pending_signal: None,
            seamless,
            show_help: false,
            last_render: Instant::now() - FRAME,
            redraw_pending: false,
            last_click: None,
            click_count: 0,
            last_click_pos: None,
        })
    }

    /// Request a repaint of window `wi`, but no faster than `FRAME`. If we're
    /// inside the cooldown, defer via `WaitUntil` so the frame still lands.
    pub(crate) fn schedule_redraw(&mut self, wi: usize, event_loop: &ActiveEventLoop) {
        let Some(win) = self.windows.get_mut(wi) else {
            return;
        };
        win.redraw_pending = true;
        if win.last_render.elapsed() >= FRAME {
            win.window.request_redraw();
        } else {
            let until = win.last_render + FRAME;
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(until));
        }
    }

    /// Refresh each pane's foreground process name across all windows (one OS
    /// snapshot for all). Returns `true` only if a name actually changed, so the
    /// caller can skip a repaint when nothing did — otherwise an idle window full
    /// of panes would repaint on every poll and burn CPU for no visible change.
    pub(crate) fn update_procs(&mut self) -> bool {
        let procs = proc::snapshot();
        let mut changed = false;
        for win in &mut self.windows {
            for tab in &mut win.tabs {
                for pane in tab.panes.values_mut() {
                    let name = pane
                        .pty
                        .pid()
                        .and_then(|pid| proc::foreground_name(&procs, pid))
                        .unwrap_or_default();
                    if name != pane.proc_name {
                        pane.proc_name = name;
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    pub(crate) fn bar_h(&self) -> usize {
        self.font.cell_h
    }

    /// Height of a pane's title bar in window `wi` (0 in seamless mode).
    pub(crate) fn title_h(&self, wi: usize) -> usize {
        if self.windows.get(wi).map(|w| w.seamless).unwrap_or(false) {
            0
        } else {
            self.font.cell_h
        }
    }

    pub(crate) fn win_size(&self, wi: usize) -> (usize, usize) {
        self.windows
            .get(wi)
            .map(|w| {
                let s = w.window.inner_size();
                (s.width.max(1) as usize, s.height.max(1) as usize)
            })
            .unwrap_or((1, 1))
    }

    pub(crate) fn content_rect(&self, w: usize, h: usize) -> Rect {
        layout::content_rect(w, h, self.bar_h())
    }

    /// Full rectangle (title bar + grid) for each pane in window `wi`'s active tab.
    pub(crate) fn pane_rects(&self, wi: usize, w: usize, h: usize) -> Vec<(usize, Rect)> {
        let area = self.content_rect(w, h);
        let mut v = Vec::new();
        if let Some(win) = self.windows.get(wi) {
            if let Some(tab) = win.tabs.get(win.active) {
                tab.tree.layout(area, &mut v);
            }
        }
        v
    }

    /// Grid area of a pane = its rect minus the title bar.
    pub(crate) fn grid_rect(&self, wi: usize, rect: Rect) -> Rect {
        layout::grid_rect(rect, self.title_h(wi))
    }

    /// Resize every pane in window `wi`'s active tab to fit the current layout.
    pub(crate) fn relayout(&mut self, wi: usize) {
        let (w, h) = self.win_size(wi);
        let rects = self.pane_rects(wi, w, h);
        let (cw, ch) = (self.font.cell_w.max(1), self.font.cell_h.max(1));
        let th = self.title_h(wi);
        if let Some(win) = self.windows.get_mut(wi) {
            let active = win.active;
            if let Some(tab) = win.tabs.get_mut(active) {
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
    }

    pub(crate) fn new_tab(&mut self, wi: usize) {
        // RT-137: refuse new tabs once the window is at the cap so holding
        // Ctrl+Shift+T (auto-repeat) can't fork-bomb shells/reader threads. The
        // restore path already enforces MAX_TABS; the interactive path must too.
        if self
            .windows
            .get(wi)
            .is_some_and(|win| tab_cap_reached(win.tabs.len()))
        {
            return;
        }
        let (w, h) = self.win_size(wi);
        let area = self.content_rect(w, h);
        let (cw, ch) = (self.font.cell_w.max(1), self.font.cell_h.max(1));
        let th = self.title_h(wi);
        let cols = (area.w / cw).max(1);
        let rows = (area.h.saturating_sub(th) / ch).max(1);
        let (n, color) = match self.windows.get(wi) {
            Some(win) => (
                win.tabs.len() + 1,
                TAB_PALETTE[win.tabs.len() % TAB_PALETTE.len()],
            ),
            None => return,
        };
        match Tab::new(format!("tab {n}"), color, cols, rows, self.proxy.clone()) {
            Ok(tab) => {
                if let Some(win) = self.windows.get_mut(wi) {
                    win.tabs.push(tab);
                    win.active = win.tabs.len() - 1;
                }
                self.relayout(wi);
            }
            Err(e) => {
                // RT-110: a failed shell spawn is only fatal at cold start, when
                // there is no live tab anywhere to fall back to. If the user
                // already has running tabs (interactive Ctrl+Shift+T / palette /
                // `+` button), a transient spawn miss must NOT tear down every
                // window — keep the existing work and just report the failure
                // (mirrors `split_focus`'s graceful skip, cf. CA-53).
                let any_tabs_alive = self.windows.iter().any(|w| !w.tabs.is_empty());
                if new_tab_failure_is_fatal(any_tabs_alive) {
                    show_error_dialog(&format!(
                        "Gritty could not start a shell.\n\n{e}\n\nThe application will now exit."
                    ));
                    std::process::exit(1);
                } else {
                    show_error_dialog(&format!(
                        "Gritty could not start a new shell.\n\n{e}\n\nYour existing tabs are unaffected."
                    ));
                }
            }
        }
    }

    pub(crate) fn split_focus(&mut self, wi: usize, axis: Axis) {
        let proxy = self.proxy.clone();
        if let Some(win) = self.windows.get_mut(wi) {
            let active = win.active;
            if let Some(tab) = win.tabs.get_mut(active) {
                // RT-137: refuse the split once the tab is at the pane cap so
                // holding Ctrl+Shift+D (auto-repeat) can't fork-bomb shells. The
                // restore path already enforces MAX_PANES_PER_TAB.
                if pane_cap_reached(tab.panes.len()) {
                    return;
                }
                // On split failure (shell could not spawn) we silently skip the
                // split rather than crash — the existing pane continues.
                let _ = tab.split(axis, proxy);
            }
        }
        self.relayout(wi);
    }

    pub(crate) fn close_focus(&mut self, wi: usize, event_loop: &ActiveEventLoop) {
        let win_empty = {
            let Some(win) = self.windows.get_mut(wi) else {
                return;
            };
            let active = win.active;
            let tab_empty = win
                .tabs
                .get_mut(active)
                .map(|t| t.close_focus())
                .unwrap_or(false);
            if tab_empty {
                win.tabs.remove(active);
                if win.tabs.is_empty() {
                    true
                } else {
                    win.active = win.active.min(win.tabs.len() - 1);
                    false
                }
            } else {
                false
            }
        };
        if win_empty {
            self.windows.remove(wi);
            if self.windows.is_empty() {
                self.persist_session();
                event_loop.exit();
                return;
            }
            if self.focused >= self.windows.len() {
                self.focused = self.windows.len() - 1;
            }
            let f = self.focused;
            self.relayout(f);
            self.request_redraw(f);
        } else {
            self.relayout(wi);
        }
    }

    pub(crate) fn move_focus(&mut self, wi: usize, dir: Dir4) {
        let (w, h) = self.win_size(wi);
        let rects = self.pane_rects(wi, w, h);
        let focus = match self
            .windows
            .get(wi)
            .and_then(|win| win.tabs.get(win.active))
        {
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
        if let (Some(id), Some(win)) = (best, self.windows.get_mut(wi)) {
            if let Some(tab) = win.tabs.get_mut(win.active) {
                tab.focus = id;
            }
        }
    }

    /// Drain every pane's output into its grid across all windows. Returns the
    /// indices of windows whose *visible* (active) tab changed, so callers only
    /// repaint windows with something new to show.
    pub(crate) fn drain_pty(&mut self) -> Vec<usize> {
        let mut dirty = Vec::new();
        for (wi, win) in self.windows.iter_mut().enumerate() {
            let active = win.active;
            let mut win_dirty = false;
            for (ti, tab) in win.tabs.iter_mut().enumerate() {
                for pane in tab.panes.values_mut() {
                    pane.pty.mark_drained();
                    let mut got = false;
                    while let Ok(chunk) = pane.pty.rx.try_recv() {
                        pane.term.feed(&chunk);
                        got = true;
                    }
                    if got && ti == active {
                        win_dirty = true;
                    }
                }
            }
            if win_dirty {
                dirty.push(wi);
            }
        }
        dirty
    }

    pub(crate) fn request_redraw(&self, wi: usize) {
        if let Some(win) = self.windows.get(wi) {
            win.window.request_redraw();
        }
    }

    /// Remove panes whose shell exited, tabs left empty, and windows left empty.
    pub(crate) fn reap_dead(&mut self, event_loop: &ActiveEventLoop) {
        let mut changed = false;
        let mut wi = 0;
        while wi < self.windows.len() {
            let win = &mut self.windows[wi];
            let mut ti = 0;
            while ti < win.tabs.len() {
                let dead: Vec<usize> = win.tabs[ti]
                    .panes
                    .iter()
                    .filter(|(_, p)| !p.pty.is_alive())
                    .map(|(id, _)| *id)
                    .collect();
                for id in dead {
                    changed = true;
                    let tab = &mut win.tabs[ti];
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
                if win.tabs[ti].panes.is_empty() {
                    win.tabs.remove(ti);
                    changed = true;
                } else {
                    ti += 1;
                }
            }
            if win.tabs.is_empty() {
                self.windows.remove(wi);
                changed = true;
            } else {
                wi += 1;
            }
        }
        if changed {
            if self.windows.is_empty() {
                self.persist_session();
                event_loop.exit();
                return;
            }
            if self.focused >= self.windows.len() {
                self.focused = self.windows.len() - 1;
            }
            for wi in 0..self.windows.len() {
                self.relayout(wi);
                self.request_redraw(wi);
            }
        }
    }

    // --- clipboard, scoped to the focused pane of window `wi`'s active tab ----

    pub(crate) fn copy_selection(&mut self, wi: usize) {
        let text = self
            .windows
            .get(wi)
            .and_then(|win| win.tabs.get(win.active))
            .and_then(|t| t.panes.get(&t.focus))
            .and_then(|p| p.term.term.selection_to_string());
        if let Some(text) = text {
            // CA-42: a whitespace-only (or empty) drag must not clobber the
            // clipboard the user may have copied from elsewhere.
            if selection_is_copyable(&text) {
                self.clip.copy(&text);
            }
        }
    }

    pub(crate) fn paste(&mut self, wi: usize) {
        let Some(text) = self.clip.paste() else {
            return;
        };
        let bracketed = self
            .windows
            .get(wi)
            .and_then(|win| win.tabs.get(win.active))
            .and_then(|t| t.panes.get(&t.focus))
            .is_some_and(|p| p.term.bracketed_paste());
        let data = crate::term::wrap_paste(&text, bracketed);
        if let Some(win) = self.windows.get_mut(wi) {
            let active = win.active;
            if let Some(tab) = win.tabs.get_mut(active) {
                let f = tab.focus;
                if let Some(pane) = tab.panes.get_mut(&f) {
                    pane.term.scroll_to_bottom();
                    pane.pty.write(&data);
                }
            }
        }
    }

    /// Paste the clipboard into EVERY pane across all tabs and all windows at
    /// once — dispatch one command to a whole fleet of agents without going
    /// pane-by-pane. Each pane gets the text wrapped for its own bracketed-paste
    /// mode. Returns the number of panes written to.
    pub(crate) fn broadcast_paste_all(&mut self) -> usize {
        let Some(text) = self.clip.paste() else {
            return 0;
        };
        if text.is_empty() {
            return 0;
        }
        let mut written = 0;
        for win in &mut self.windows {
            for tab in &mut win.tabs {
                for pane in tab.panes.values_mut() {
                    let data = crate::term::wrap_paste(&text, pane.term.bracketed_paste());
                    pane.term.scroll_to_bottom();
                    pane.pty.write(&data);
                    written += 1;
                }
            }
        }
        written
    }

    /// Capture every window's workspace for persistence.
    pub(crate) fn snapshot(&self) -> persist::SavedSession {
        let windows = self
            .windows
            .iter()
            .map(|win| {
                let tabs = win
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
                let inner = win.window.inner_size();
                let pos = win.window.outer_position().ok();
                SavedWindow {
                    active: win.active,
                    tabs,
                    win_w: Some(inner.width),
                    win_h: Some(inner.height),
                    win_x: pos.map(|p| p.x),
                    win_y: pos.map(|p| p.y),
                    seamless: win.seamless,
                }
            })
            .collect();
        persist::SavedSession::from_windows(windows)
    }

    /// Save the workspace to disk (best-effort). Called on every exit path and
    /// whenever names change, so the layout — including tab and pane names and
    /// each window's screen position — survives a restart no matter how gritty
    /// was closed.
    pub(crate) fn persist_session(&self) {
        let _ = persist::save(&self.snapshot());
    }

    /// Restore saved windows, each as its own OS window at its saved position.
    /// Skips any window/tab that fails to spawn so one bad entry can't block
    /// startup. Caps both windows and per-window tabs/panes (RT-5).
    pub(crate) fn restore_windows(
        &mut self,
        event_loop: &ActiveEventLoop,
        saved: Vec<SavedWindow>,
    ) {
        // RT-26: bound the *aggregate* restored-pane count across all windows, not
        // just each window/tab independently — a crafted session can encode the
        // product (16×64×64) under the file-size cap and `Tab::from_saved` spawns
        // one real shell per leaf synchronously. Once the global budget is spent we
        // stop restoring further windows.
        let mut restored_panes = 0usize;

        for sw in saved.into_iter().take(MAX_WINDOWS) {
            if sw.tabs.is_empty() || sw.tabs.len() > MAX_TABS {
                continue;
            }
            let window_panes: usize = sw
                .tabs
                .iter()
                .map(|st| {
                    let mut lv = Vec::new();
                    st.tree.leaves(&mut lv);
                    lv.len()
                })
                .sum();
            let too_many_panes = sw.tabs.iter().any(|st| {
                let mut lv = Vec::new();
                st.tree.leaves(&mut lv);
                lv.len() > MAX_PANES_PER_TAB
            });
            if too_many_panes {
                continue;
            }
            // RT-26: skip any window that would push the aggregate over budget. We
            // stop rather than partially restore a window so each restored window is
            // internally consistent.
            if restored_panes_over_budget(restored_panes, window_panes) {
                break;
            }
            let size = match (sw.win_w, sw.win_h) {
                (Some(w), Some(h)) if w >= 200 && h >= 100 => (w as f64, h as f64),
                _ => (960.0, 600.0),
            };
            let pos = match (sw.win_x, sw.win_y) {
                (Some(x), Some(y)) => Some((x, y)),
                _ => None,
            };
            let Some(mut win) = self.spawn_window(event_loop, size, pos, sw.seamless) else {
                continue;
            };
            let inner = win.window.inner_size();
            let (cw, ch) = (self.font.cell_w.max(1), self.font.cell_h.max(1));
            let area = layout::content_rect(
                inner.width.max(1) as usize,
                inner.height.max(1) as usize,
                self.bar_h(),
            );
            // A pane title bar (one cell row) is reserved only when NOT seamless
            // (CA-57: a restored seamless window has no per-pane title bars).
            let cols = (area.w / cw).max(1);
            let title_px = if sw.seamless { 0 } else { self.font.cell_h };
            let rows = (area.h.saturating_sub(title_px) / ch).max(1);
            let tabs: Vec<Tab> = sw
                .tabs
                .iter()
                .filter_map(|st| Tab::from_saved(st, cols, rows, self.proxy.clone()).ok())
                .collect();
            if tabs.is_empty() {
                continue;
            }
            win.active = sw.active.min(tabs.len() - 1);
            // RT-26: count the panes we actually spawned (some leaves may have been
            // dropped by `Tab::from_saved` on spawn failure) toward the budget.
            restored_panes += tabs.iter().map(|t| t.panes.len()).sum::<usize>();
            win.tabs = tabs;
            self.windows.push(win);
            let wi = self.windows.len() - 1;
            self.relayout(wi);
            self.request_redraw(wi);
        }
    }

    /// Replace the entire workspace with the saved session (if any). Used by the
    /// "load session" command — closes current windows and reopens saved ones.
    pub(crate) fn restore_session(&mut self, event_loop: &ActiveEventLoop) {
        let Some(saved) = persist::load() else {
            return;
        };
        let windows = saved.windows();
        if windows.is_empty() {
            return;
        }
        // RT-41: don't clear the live windows up front. If every saved window
        // fails to respawn (resource exhaustion, or shells that won't start),
        // clearing first would leave gritty with ZERO windows — no RedrawRequested,
        // no input target, and nothing to call `event_loop.exit()`: an invisible
        // zombie the user can only kill via Task Manager. Instead restore into a
        // fresh `self.windows` (the live ones held aside) and only discard the old
        // ones once at least one new window actually spawned; otherwise put them
        // back so the workspace is preserved.
        let previous = std::mem::take(&mut self.windows);
        self.restore_windows(event_loop, windows);
        if self.windows.is_empty() {
            // Nothing restored — keep the existing workspace intact.
            self.windows = previous;
        }
        if !self.windows.is_empty() {
            self.focused = self.focused.min(self.windows.len() - 1);
        }
    }

    /// Tear the tab at `tab_index` out of window `wi` into a new OS window placed
    /// at `pos` (top-left, physical px; `None` lets the OS choose). The torn tab
    /// keeps its live panes/PTYs. A window's only tab is never torn off.
    pub(crate) fn tear_off(
        &mut self,
        event_loop: &ActiveEventLoop,
        wi: usize,
        tab_index: usize,
        pos: Option<(i32, i32)>,
    ) {
        // RT-137: refuse a tear-off once at the window cap so repeated tear-offs /
        // Ctrl+Shift+N (auto-repeat) can't spawn unbounded OS windows. Checked
        // before removing the source tab so a refused tear-off loses nothing.
        if window_cap_reached(self.windows.len()) {
            return;
        }
        let (n, size, seamless) = match self.windows.get(wi) {
            Some(win) => {
                let s = win.window.inner_size();
                (
                    win.tabs.len(),
                    (s.width.max(200) as f64, s.height.max(100) as f64),
                    win.seamless,
                )
            }
            None => return,
        };
        if n <= 1 || tab_index >= n {
            return; // never tear a window's only tab
        }
        // Remove the live tab (panes + PTYs move intact).
        let tab = self.windows[wi].tabs.remove(tab_index);
        {
            let win = &mut self.windows[wi];
            if win.active > tab_index {
                win.active -= 1;
            }
            win.active = win.active.min(win.tabs.len().saturating_sub(1));
        }
        let Some(mut nw) = self.spawn_window(event_loop, size, pos, seamless) else {
            // Couldn't create the window — put the tab back so it isn't lost.
            let win = &mut self.windows[wi];
            let at = tab_index.min(win.tabs.len());
            win.tabs.insert(at, tab);
            return;
        };
        nw.tabs.push(tab);
        nw.active = 0;
        self.windows.push(nw);
        let new_wi = self.windows.len() - 1;
        self.focused = new_wi;
        self.relayout(new_wi);
        self.relayout(wi);
        self.request_redraw(wi);
        self.request_redraw(new_wi);
    }

    pub(crate) fn focus_and_redraw(&mut self, wi: usize, dir: Dir4) {
        self.move_focus(wi, dir);
        self.request_redraw(wi);
    }

    pub(crate) fn resize_focus(&mut self, wi: usize, axis: Axis, grow: bool) {
        if let Some(win) = self.windows.get_mut(wi) {
            let active = win.active;
            if let Some(tab) = win.tabs.get_mut(active) {
                tab.resize_focus(axis, grow);
            }
        }
        self.relayout(wi);
        self.request_redraw(wi);
    }

    /// Tab index under an x pixel on window `wi`'s tab bar.
    pub(crate) fn tab_at(&self, wi: usize, x: usize) -> Option<usize> {
        let win = self.windows.get(wi)?;
        layout::tab_at(
            win.tabs.iter().map(|t| t.name.chars().count()),
            self.font.cell_w,
            x,
        )
    }

    /// CA-28: hit-test window `wi`'s tab strip for `×` and `+` buttons.
    pub(crate) fn tab_button_at(&self, wi: usize, x: usize, w: usize) -> Option<TabHit> {
        let win = self.windows.get(wi)?;
        tab_button_at(
            win.tabs.iter().map(|t| t.name.chars().count()),
            self.font.cell_w,
            x,
            w,
        )
    }

    /// Pane id under a pixel in window `wi`, plus its grid rect.
    pub(crate) fn pane_at(&self, wi: usize, x: f64, y: f64) -> Option<(usize, Rect)> {
        let (w, h) = self.win_size(wi);
        for (id, rect) in self.pane_rects(wi, w, h) {
            if rect.contains(x as usize, y as usize) {
                return Some((id, self.grid_rect(wi, rect)));
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

    /// CA-12: Rebuild the font atlas at `new_px`, relayout all windows, redraw.
    pub(crate) fn apply_font_zoom(&mut self, new_px: f32) {
        let px = new_px.clamp(MIN_FONT_PX, MAX_FONT_PX);
        if (px - self.font_px).abs() < f32::EPSILON {
            return;
        }
        self.font_px = px;
        // CA-103: re-derive metrics at the new size in place — keep the parsed
        // font face, don't re-read + re-parse the TTF from disk on every zoom key.
        self.font.set_px(px);
        for wi in 0..self.windows.len() {
            self.relayout(wi);
            self.request_redraw(wi);
        }
    }

    /// CA-23: Update window `wi`'s OS cursor based on divider hover.
    pub(crate) fn update_cursor_shape(&self, wi: usize) {
        let Some(win) = self.windows.get(wi) else {
            return;
        };
        let (x, y) = win.mouse_pos;
        let (w, h) = self.win_size(wi);
        let area = self.content_rect(w, h);
        let cursor = if let Some(tab) = win.tabs.get(win.active) {
            match tab.tree.divider_at(area, x as usize, y as usize, 5) {
                Some(path) => {
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
        win.window.set_cursor(cursor);
    }

    /// CA-33: Return the hyperlink URI of the cell at pixel (x, y) in window `wi`.
    pub(crate) fn hyperlink_at_pixel(&self, wi: usize, x: f64, y: f64) -> Option<String> {
        let (w, h) = self.win_size(wi);
        let win = self.windows.get(wi)?;
        let tab = win.tabs.get(win.active)?;
        let rects = self.pane_rects(wi, w, h);
        let (pane_id, rect) = rects
            .iter()
            .find(|(_, r)| r.contains(x as usize, y as usize))?;
        let grid = self.grid_rect(wi, *rect);
        let pane = tab.panes.get(pane_id)?;
        let (cw, ch) = (self.font.cell_w, self.font.cell_h);
        let gx = (x as usize).saturating_sub(grid.x);
        let gy = (y as usize).saturating_sub(grid.y);
        // RT-16: clamp the column and bounds-check the line before indexing the VT
        // grid — `pane_at`/`r.contains` accept the full pane rect, so a click on a
        // trailing partial column (pane width not a multiple of the cell width) or
        // far into scrollback would otherwise index out of range and panic, which
        // under `panic = "abort"` is a silent crash.
        let vt = pane.term.term.grid();
        let (cols, screen_lines, total_lines) = (vt.columns(), vt.screen_lines(), vt.total_lines());
        let history = total_lines.saturating_sub(screen_lines);
        let display_offset = pane.term.display_offset();
        let (line, col) =
            hyperlink_cell(gx, gy, cw, ch, cols, screen_lines, history, display_offset)?;
        let point = Point::new(Line(line), Column(col));
        let cell = vt[point].clone();
        let hyperlink = cell.hyperlink()?;
        Some(hyperlink.uri().to_owned())
    }

    /// CA-18: classify click count for window `wi` based on timing.
    pub(crate) fn classify_click(&mut self, wi: usize) -> u32 {
        let now = Instant::now();
        let (cw, ch) = (
            self.font.cell_w.max(1) as f64,
            self.font.cell_h.max(1) as f64,
        );
        let Some(win) = self.windows.get_mut(wi) else {
            return 1;
        };
        let elapsed_ms = match win.last_click {
            Some(prev) => now.duration_since(prev).as_millis() as u64,
            None => u64::MAX,
        };
        // CA-62/CA-82: a double/triple click must be at (about) the same cell, not
        // merely within the time window — otherwise two quick clicks in different
        // cells/panes mis-fire a word/line selection. Reset when the pointer moved
        // more than ~1 cell from the previous press.
        let (mx, my) = win.mouse_pos;
        let moved_far = match win.last_click_pos {
            Some((px, py)) => (mx - px).abs() > cw || (my - py).abs() > ch,
            None => true,
        };
        let count = next_click_count(elapsed_ms, moved_far, win.click_count);
        win.last_click = Some(now);
        win.last_click_pos = Some((mx, my));
        win.click_count = count;
        count
    }

    /// CA-7: true if the focused pane of window `wi` has a mouse-reporting mode.
    pub(crate) fn pane_wants_mouse(&self, wi: usize) -> bool {
        use alacritty_terminal::term::TermMode;
        self.windows
            .get(wi)
            .and_then(|win| win.tabs.get(win.active))
            .and_then(|t| t.panes.get(&t.focus))
            .is_some_and(|p| {
                p.term
                    .term
                    .mode()
                    .intersects(TermMode::MOUSE_MODE | TermMode::SGR_MOUSE)
            })
    }

    /// CA-7: Forward a mouse event to window `wi`'s focused pane as SGR.
    pub(crate) fn forward_mouse_sgr(
        &mut self,
        wi: usize,
        btn: u8,
        col: u16,
        row: u16,
        press: bool,
    ) {
        let seq = encode_sgr_mouse(btn, col, row, press);
        if let Some(win) = self.windows.get_mut(wi) {
            let active = win.active;
            if let Some(tab) = win.tabs.get_mut(active) {
                let f = tab.focus;
                if let Some(pane) = tab.panes.get_mut(&f) {
                    pane.pty.write(&seq);
                }
            }
        }
    }

    /// CA-7: Convert pixel position to 1-based (col, row) for window `wi`'s focused pane.
    pub(crate) fn pixel_to_term_cell(&self, wi: usize, x: f64, y: f64) -> Option<(u16, u16)> {
        let (w, h) = self.win_size(wi);
        let win = self.windows.get(wi)?;
        let tab = win.tabs.get(win.active)?;
        let rects = self.pane_rects(wi, w, h);
        let (_, pane_rect) = rects.iter().find(|(id, _)| *id == tab.focus)?;
        let grid = self.grid_rect(wi, *pane_rect);
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
        let term_col = (col as u16).saturating_add(1);
        let term_row = (row.max(0) as u16).saturating_add(1);
        Some((term_col, term_row))
    }
}

impl ApplicationHandler<Wake> for Gritty {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.windows.is_empty() {
            return;
        }
        let saved = persist::load();
        let saved_windows = saved.as_ref().map(|s| s.windows()).unwrap_or_default();

        if !saved_windows.is_empty() {
            self.restore_windows(event_loop, saved_windows);
        }
        // Fresh start, or every saved window failed to restore: open one window
        // with a single tab.
        if self.windows.is_empty() {
            // RT-17: degrade gracefully if the OS refuses the first window
            // (headless/Session-0, exhausted desktop heap, broken GPU/driver)
            // instead of `.expect()`-aborting silently under `panic = "abort"`.
            match self.spawn_window(event_loop, (960.0, 600.0), None, false) {
                Some(win) => {
                    self.windows.push(win);
                    self.new_tab(0);
                }
                None => {
                    show_error_dialog(
                        "Gritty could not create its window.\n\nThe application will now exit.",
                    );
                    event_loop.exit();
                    return;
                }
            }
        }
        self.focused = 0;
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: Wake) {
        let dirty = self.drain_pty();
        self.reap_dead(event_loop);
        let mut proc_dirty = false;
        if self.last_proc_poll.elapsed() >= Duration::from_millis(750) {
            proc_dirty = self.update_procs(); // repaint only if a header changed
            self.last_proc_poll = Instant::now();

            // Leak probe (debug only): RSS + OS thread count vs. live pane count.
            // RSS climbing → heap leak; os_threads growing while panes flat →
            // leaked PTY reader threads; neither growing but CPU pegged → redraw
            // spin. Prints to stderr, visible under `cargo run`.
            #[cfg(debug_assertions)]
            if let Some((rss, threads)) = proc::self_stats() {
                let panes: usize = self
                    .windows
                    .iter()
                    .flat_map(|w| w.tabs.iter())
                    .map(|t| t.panes.len())
                    .sum();
                eprintln!(
                    "[gritty probe] rss={} MB  os_threads={}  panes={}  windows={}",
                    rss / (1024 * 1024),
                    threads,
                    panes,
                    self.windows.len()
                );
            }
        }
        // reap_dead may have removed windows (shifting indices); when anything
        // changed, just schedule all live windows — they're few and frame-capped.
        if proc_dirty || !dirty.is_empty() {
            for wi in 0..self.windows.len() {
                self.schedule_redraw(wi, event_loop);
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        for win in &self.windows {
            if win.redraw_pending && win.last_render.elapsed() >= FRAME {
                win.window.request_redraw();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(wi) = self.idx_of(id) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                self.persist_session();
                self.windows.remove(wi);
                if self.windows.is_empty() {
                    event_loop.exit();
                    return;
                }
                if self.focused >= self.windows.len() {
                    self.focused = self.windows.len() - 1;
                }
            }

            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    // Keyboard always targets the focused window.
                    self.handle_key(event_loop, &event.logical_key);
                }
            }

            // Repaint a fresh frame when re-shown or refocused, so alt-tabbing
            // back can't present a stale softbuffer frame.
            WindowEvent::Focused(true) => {
                self.focused = wi;
                self.request_redraw(wi);
            }
            WindowEvent::Occluded(false) => self.request_redraw(wi),

            // Pointer left the window: if a tab is being dragged, arm tear-off
            // (belt-and-suspenders with the bounds check in CursorMoved, so the
            // gesture works whether or not winit captures the pointer).
            WindowEvent::CursorLeft { .. } => {
                if let Some(win) = self.windows.get_mut(wi) {
                    if let Some(td) = win.tab_drag.as_mut() {
                        td.armed = true;
                    }
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                let (px, py) = (position.x, position.y);
                // Tab tear-off drag: arm once the pointer leaves the bounds.
                let inner = self.windows[wi].window.inner_size();
                let (ww, wh) = (inner.width as f64, inner.height as f64);
                {
                    let win = &mut self.windows[wi];
                    win.mouse_pos = (px, py);
                    if let Some(td) = win.tab_drag.as_mut() {
                        if px < 0.0 || py < 0.0 || px >= ww || py >= wh {
                            td.armed = true;
                        }
                        return; // suppress selection/divider/hover while tearing
                    }
                }
                let dragging = self.windows[wi].dragging.clone();
                if let Some(path) = dragging {
                    self.drag_divider(wi, &path, px, py);
                } else if self.windows[wi].selecting {
                    self.update_selection(wi, px, py);
                } else if self.pane_wants_mouse(wi) {
                    if let Some((col, row)) = self.pixel_to_term_cell(wi, px, py) {
                        // Button 35 = motion with no button held (32 + 3).
                        self.forward_mouse_sgr(wi, 35, col, row, true);
                    }
                } else {
                    self.update_cursor_shape(wi);
                }
            }

            WindowEvent::MouseInput { state, button, .. } => match (state, button) {
                (ElementState::Pressed, MouseButton::Left) => {
                    self.focused = wi;
                    self.begin_selection(wi, event_loop);
                }
                (ElementState::Released, MouseButton::Left) => {
                    // Tab tear-off: dropped outside the window → new window.
                    if let Some(td) = self.windows[wi].tab_drag.take() {
                        let (w, h) = self.win_size(wi);
                        let (mx, my) = self.windows[wi].mouse_pos;
                        let outside = mx < 0.0 || my < 0.0 || mx >= w as f64 || my >= h as f64;
                        let n = self.windows[wi].tabs.len();
                        if should_tear_off(td.armed, outside, n) {
                            let pos = cursor_pos().map(|(cx, cy)| (cx - 40, cy - 8));
                            self.tear_off(event_loop, wi, td.index, pos);
                        }
                        if let Some(win) = self.windows.get_mut(wi) {
                            win.selecting = false;
                        }
                        return;
                    }
                    if self.pane_wants_mouse(wi) {
                        let (mx, my) = self.windows[wi].mouse_pos;
                        if let Some((col, row)) = self.pixel_to_term_cell(wi, mx, my) {
                            self.forward_mouse_sgr(wi, 0, col, row, false);
                        }
                    } else {
                        let was_dragging = self.windows[wi].dragging.take().is_some();
                        if !was_dragging && self.windows[wi].selecting {
                            self.copy_selection(wi);
                        }
                    }
                    if let Some(win) = self.windows.get_mut(wi) {
                        win.selecting = false;
                    }
                }
                (ElementState::Pressed, MouseButton::Right) => self.paste(wi),
                _ => {}
            },

            WindowEvent::MouseWheel { delta, .. } => {
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / self.font.cell_h as f64) as f32,
                };
                if self.mods.control_key() {
                    if notches != 0.0 {
                        let grow = notches > 0.0;
                        if let Some(win) = self.windows.get_mut(wi) {
                            let active = win.active;
                            if let Some(tab) = win.tabs.get_mut(active) {
                                tab.resize_focus(Axis::LeftRight, grow);
                                tab.resize_focus(Axis::TopBottom, grow);
                            }
                        }
                        self.relayout(wi);
                        self.request_redraw(wi);
                    }
                } else if self.pane_wants_mouse(wi) {
                    if notches != 0.0 {
                        let (mx, my) = self.windows[wi].mouse_pos;
                        if let Some((col, row)) = self.pixel_to_term_cell(wi, mx, my) {
                            let btn = if notches > 0.0 { 64u8 } else { 65u8 };
                            self.forward_mouse_sgr(wi, btn, col, row, true);
                        }
                    }
                } else {
                    let lines = (notches * 3.0) as i32;
                    if lines != 0 {
                        if let Some(win) = self.windows.get_mut(wi) {
                            let active = win.active;
                            if let Some(tab) = win.tabs.get_mut(active) {
                                let f = tab.focus;
                                if let Some(pane) = tab.panes.get_mut(&f) {
                                    pane.term.scroll(lines);
                                }
                            }
                        }
                        self.request_redraw(wi);
                    }
                }
            }

            WindowEvent::Resized(_) => {
                self.relayout(wi);
                self.request_redraw(wi);
            }

            WindowEvent::RedrawRequested => {
                self.redraw(wi);
                event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
            }

            _ => {}
        }
    }
}

impl Gritty {
    pub(crate) fn begin_selection(&mut self, wi: usize, event_loop: &ActiveEventLoop) {
        // Clear any stale tab-drag from a previous press whose release we never
        // saw (e.g. released over another window with no pointer capture).
        if let Some(win) = self.windows.get_mut(wi) {
            win.tab_drag = None;
        }
        let (x, y) = self
            .windows
            .get(wi)
            .map(|w| w.mouse_pos)
            .unwrap_or((0.0, 0.0));

        // CA-33: Ctrl+Click on a hyperlink cell — open it in the OS handler.
        if self.mods.control_key() {
            if let Some(uri) = self.hyperlink_at_pixel(wi, x, y) {
                open_hyperlink(&uri);
                return;
            }
        }

        // Click on the tab bar switches/closes/creates tabs instead of selecting.
        if (y as usize) < self.bar_h() {
            let (w, _) = self.win_size(wi);
            // CA-28: check for × (close) and + (new) button hits first.
            if let Some(hit) = self.tab_button_at(wi, x as usize, w) {
                match hit {
                    TabHit::Close(i) => {
                        let len = self.windows.get(wi).map(|win| win.tabs.len()).unwrap_or(0);
                        if i < len {
                            if let Some(win) = self.windows.get_mut(wi) {
                                win.active = i;
                                win.broadcast = false;
                                win.broadcast_pending_signal = None;
                            }
                            self.close_focus(wi, event_loop);
                        }
                    }
                    TabHit::New => {
                        self.new_tab(wi);
                        if let Some(win) = self.windows.get_mut(wi) {
                            win.broadcast = false;
                            win.broadcast_pending_signal = None;
                        }
                    }
                }
                self.request_redraw(wi);
                return;
            }
            if let Some(i) = self.tab_at(wi, x as usize) {
                if let Some(win) = self.windows.get_mut(wi) {
                    if i != win.active {
                        win.broadcast = false;
                        win.broadcast_pending_signal = None;
                    }
                    win.active = i;
                    // Arm a possible tear-off: dragging this tab out tears it off.
                    win.tab_drag = Some(TabDrag {
                        index: i,
                        armed: false,
                    });
                }
                self.drain_pty(); // RT-10: flush newly focused tab's PTY output.
                self.relayout(wi);
                self.request_redraw(wi);
            }
            return;
        }

        // Grab a divider to drag-resize.
        let (w, h) = self.win_size(wi);
        let area = self.content_rect(w, h);
        let divider = self
            .windows
            .get(wi)
            .and_then(|win| win.tabs.get(win.active))
            .and_then(|t| t.tree.divider_at(area, x as usize, y as usize, 5));
        if let Some(path) = divider {
            if let Some(win) = self.windows.get_mut(wi) {
                win.dragging = Some(path);
            }
            return;
        }

        // CA-7: if the pane has mouse mode, forward the click and return early.
        if self.pane_wants_mouse(wi) {
            if let Some((id, _)) = self.pane_at(wi, x, y) {
                if let Some(win) = self.windows.get_mut(wi) {
                    if let Some(tab) = win.tabs.get_mut(win.active) {
                        if tab.panes.contains_key(&id) {
                            tab.focus = id;
                        }
                    }
                }
            }
            if let Some((col, row)) = self.pixel_to_term_cell(wi, x, y) {
                self.forward_mouse_sgr(wi, 0, col, row, true);
            }
            return;
        }

        let Some((id, grid)) = self.pane_at(wi, x, y) else {
            return;
        };
        // Focus the clicked pane.
        if let Some(win) = self.windows.get_mut(wi) {
            if let Some(tab) = win.tabs.get_mut(win.active) {
                if tab.panes.contains_key(&id) {
                    tab.focus = id;
                }
            }
        }

        // CA-18: classify click count for word/line selection.
        let count = self.classify_click(wi);

        let (cols, off) = self
            .windows
            .get(wi)
            .and_then(|win| win.tabs.get(win.active))
            .and_then(|t| t.panes.get(&id))
            .map(|p| (p.term.size.cols, p.term.display_offset()))
            .unwrap_or((1, 0));
        let (point, side) = self.point_in_grid(grid, x, y, cols, off);

        let sel_type = match count {
            1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };

        if let Some(win) = self.windows.get_mut(wi) {
            if let Some(tab) = win.tabs.get_mut(win.active) {
                if let Some(pane) = tab.panes.get_mut(&id) {
                    pane.term.term.selection = Some(Selection::new(sel_type, point, side));
                }
            }
            win.selecting = true;
        }
        self.request_redraw(wi);
    }

    pub(crate) fn drag_divider(&mut self, wi: usize, path: &[u8], x: f64, y: f64) {
        let (w, h) = self.win_size(wi);
        let area = self.content_rect(w, h);
        let Some((axis, srect)) = self
            .windows
            .get(wi)
            .and_then(|win| win.tabs.get(win.active))
            .and_then(|t| t.tree.split_area(path, area))
        else {
            return;
        };
        let ratio = match axis {
            Axis::LeftRight => (x - srect.x as f64) / (srect.w.max(1) as f64),
            Axis::TopBottom => (y - srect.y as f64) / (srect.h.max(1) as f64),
        } as f32;
        if let Some(win) = self.windows.get_mut(wi) {
            if let Some(tab) = win.tabs.get_mut(win.active) {
                tab.tree.set_ratio(path, ratio);
            }
        }
        self.relayout(wi);
        self.request_redraw(wi);
    }

    pub(crate) fn update_selection(&mut self, wi: usize, x: f64, y: f64) {
        let focus = match self
            .windows
            .get(wi)
            .and_then(|win| win.tabs.get(win.active))
        {
            Some(t) => t.focus,
            None => return,
        };
        let (w, h) = self.win_size(wi);
        let grid = self
            .pane_rects(wi, w, h)
            .into_iter()
            .find(|(id, _)| *id == focus)
            .map(|(_, r)| self.grid_rect(wi, r));
        let Some(grid) = grid else { return };
        let (cols, off) = self
            .windows
            .get(wi)
            .and_then(|win| win.tabs.get(win.active))
            .and_then(|t| t.panes.get(&focus))
            .map(|p| (p.term.size.cols, p.term.display_offset()))
            .unwrap_or((1, 0));
        let (point, side) = self.point_in_grid(grid, x, y, cols, off);
        if let Some(win) = self.windows.get_mut(wi) {
            if let Some(tab) = win.tabs.get_mut(win.active) {
                if let Some(pane) = tab.panes.get_mut(&focus) {
                    if let Some(sel) = pane.term.term.selection.as_mut() {
                        sel.update(point, side);
                    }
                }
            }
        }
        self.request_redraw(wi);
    }
}

// --- Cursor position (for placing torn-off windows) --------------------------

/// Global cursor position in physical screen pixels, used to drop a torn-off
/// window where the user released the tab. `None` off Windows or on failure.
#[cfg(windows)]
fn cursor_pos() -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: p is a valid, writable POINT.
    if unsafe { GetCursorPos(&mut p) } != 0 {
        Some((p.x, p.y))
    } else {
        None
    }
}

#[cfg(not(windows))]
fn cursor_pos() -> Option<(i32, i32)> {
    None
}

// --- Error dialog ------------------------------------------------------------

/// Show a native Windows MessageBox with an error icon, then return.
#[cfg(windows)]
pub(crate) fn show_error_dialog(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let title: Vec<u16> = "Gritty — Fatal Error"
        .encode_utf16()
        .chain(std::iter::once(0u16))
        .collect();
    let body: Vec<u16> = message
        .encode_utf16()
        .chain(std::iter::once(0u16))
        .collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
pub(crate) fn show_error_dialog(message: &str) {
    eprintln!("Fatal error: {message}");
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
pub(crate) fn tab_button_at(
    name_lens: impl IntoIterator<Item = usize>,
    cw: usize,
    x: usize,
    w: usize,
) -> Option<TabHit> {
    let mut tx = 0usize;
    for (i, len) in name_lens.into_iter().enumerate() {
        let text_w = (len + 2) * cw;
        let slot_w = text_w + cw;
        let gap = cw / 2;
        if tx + slot_w > w {
            break; // overflow: don't draw (or hit-test) past window edge
        }
        if x >= tx + text_w && x < tx + slot_w {
            return Some(TabHit::Close(i));
        }
        tx += slot_w + gap;
    }
    if tx + cw <= w && x >= tx && x < tx + cw {
        return Some(TabHit::New);
    }
    None
}

/// Decide whether a tab-bar drag that just ended should tear the tab into its
/// own window: only when the drag left the window (`armed`), was released
/// outside the bounds, and the source window has more than one tab.
pub(crate) fn should_tear_off(armed: bool, released_outside: bool, tab_count: usize) -> bool {
    armed && released_outside && tab_count > 1
}

/// RT-8: Returns true if `b` is a signal-bearing control byte requiring a
/// second-press confirmation before broadcasting (ETX/EOT/SUB).
pub(crate) fn is_broadcast_signal_byte(b: u8) -> bool {
    matches!(b, 0x03 | 0x04 | 0x1a)
}

/// CA-33: Return `true` iff `uri` has a scheme safe to hand to the OS opener.
///
/// RT-25/RT-40: `http`/`https` only — `file://` is deliberately rejected.
/// `open_hyperlink` passes the URI to `ShellExecuteW`'s `open` verb, which
/// *executes* a `file://` target (a local or UNC `.exe`/`.bat`/`.ps1`/`.lnk`/…).
/// OSC-8 lets the visible link text differ from the target, so a hostile byte
/// stream (a `cat`'d file, an SSH session, a build log) could render benign text
/// over a `file://` path and a single Ctrl+click would launch attacker-chosen
/// code. Browser schemes route through the default browser and are safe; the
/// local-file program launcher is not.
pub(crate) fn is_safe_hyperlink_scheme(uri: &str) -> bool {
    let lower = uri.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// CA-33: Open `uri` with the OS default handler via `ShellExecuteW`, detached.
fn open_hyperlink(uri: &str) {
    if !is_safe_hyperlink_scheme(uri) {
        return;
    }
    #[cfg(windows)]
    {
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn ShellExecuteW(
                hwnd: *mut core::ffi::c_void,
                lpoperation: *const u16,
                lpfile: *const u16,
                lpparameters: *const u16,
                lpdirectory: *const u16,
                nshowcmd: i32,
            ) -> isize;
        }
        const SW_SHOWNORMAL: i32 = 1;
        let verb: Vec<u16> = "open\0".encode_utf16().collect();
        let uri_w: Vec<u16> = uri.encode_utf16().chain(std::iter::once(0u16)).collect();
        // SAFETY: all pointers are valid null-terminated UTF-16; hwnd=null is
        // valid for a shell open that needs no parent window.
        unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                uri_w.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            );
        }
    }
    #[cfg(not(windows))]
    {
        let _ = uri;
    }
}

/// RT-16: Map a pane-local pixel `(gx, gy)` to an in-bounds VT grid
/// `(line, column)`, or `None` if it falls outside the grid. The column is
/// clamped to the last cell (a trailing partial column of a non-cell-multiple
/// pane width must never index `cols`), and the line is rejected when it falls
/// outside the scrollback (`-history`) / screen (`screen_lines - 1`) range, so
/// indexing `grid()[point]` can never panic.
#[allow(clippy::too_many_arguments)]
pub(crate) fn hyperlink_cell(
    gx: usize,
    gy: usize,
    cw: usize,
    ch: usize,
    cols: usize,
    screen_lines: usize,
    history: usize,
    display_offset: usize,
) -> Option<(i32, usize)> {
    if cols == 0 || screen_lines == 0 {
        return None;
    }
    let col = (gx / cw.max(1)).min(cols - 1);
    let row = (gy / ch.max(1)) as i32;
    let line = row - display_offset as i32;
    let min_line = -(history as i32);
    let max_line = screen_lines as i32 - 1;
    if line < min_line || line > max_line {
        return None;
    }
    Some((line, col))
}

/// RT-110: A failed `new_tab` shell spawn is fatal only when no tab is alive
/// anywhere (the genuine cold-start case). With other tabs running, the failure
/// is transient and must not exit the process.
pub(crate) fn new_tab_failure_is_fatal(any_tabs_alive: bool) -> bool {
    !any_tabs_alive
}

/// RT-26: true if restoring a window with `window_panes` panes would push the
/// running aggregate (`restored_so_far`) past `MAX_RESTORED_PANES`. Restoring
/// stops at the first window that would exceed the budget.
pub(crate) fn restored_panes_over_budget(restored_so_far: usize, window_panes: usize) -> bool {
    restored_so_far.saturating_add(window_panes) > MAX_RESTORED_PANES
}

/// RT-137: true if a window already at `current_tabs` tabs is at the runtime cap
/// and a new tab must be refused. Mirrors the restore-time `MAX_TABS` bound.
pub(crate) fn tab_cap_reached(current_tabs: usize) -> bool {
    current_tabs >= MAX_TABS
}

/// RT-137: true if a tab already at `current_panes` panes is at the runtime cap
/// and a split must be refused. Mirrors the restore-time `MAX_PANES_PER_TAB` bound.
pub(crate) fn pane_cap_reached(current_panes: usize) -> bool {
    current_panes >= MAX_PANES_PER_TAB
}

/// RT-137: true if there are already `current_windows` OS windows and a new
/// window (tear-off / Ctrl+Shift+N) must be refused. Mirrors `MAX_WINDOWS`.
pub(crate) fn window_cap_reached(current_windows: usize) -> bool {
    current_windows >= MAX_WINDOWS
}

/// CA-7: Encode an SGR mouse sequence.
pub(crate) fn encode_sgr_mouse(btn: u8, col: u16, row: u16, press: bool) -> Vec<u8> {
    let suffix = if press { 'M' } else { 'm' };
    format!("\x1b[<{};{};{}{}", btn, col, row, suffix).into_bytes()
}

/// CA-42: a selection is worth copying only when it has a non-whitespace
/// character; a whitespace-only drag must not clobber the clipboard.
pub(crate) fn selection_is_copyable(text: &str) -> bool {
    !text.trim().is_empty()
}

/// CA-18 / CA-62 / CA-82: classify a click into single/double/triple. A multi-
/// click requires both a short interval (`elapsed_ms <= MULTI_CLICK_MS`) AND that
/// the pointer did not move far from the previous press (`!moved_far`); otherwise
/// the run resets to a fresh single click.
pub(crate) fn next_click_count(elapsed_ms: u64, moved_far: bool, prev_count: u32) -> u32 {
    if elapsed_ms <= MULTI_CLICK_MS && !moved_far {
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
        assert_eq!(next_click_count(u64::MAX, false, 0), 1);
    }

    #[test]
    fn rapid_second_click_is_double() {
        assert_eq!(next_click_count(100, false, 1), 2);
    }

    #[test]
    fn rapid_third_click_is_triple() {
        assert_eq!(next_click_count(200, false, 2), 3);
    }

    #[test]
    fn fourth_rapid_click_stays_at_three() {
        assert_eq!(next_click_count(50, false, 3), 3);
    }

    #[test]
    fn slow_click_resets_to_single() {
        assert_eq!(next_click_count(600, false, 2), 1);
    }

    #[test]
    fn click_at_exactly_threshold_counts() {
        assert_eq!(next_click_count(MULTI_CLICK_MS, false, 1), 2);
    }

    #[test]
    fn click_just_over_threshold_resets() {
        assert_eq!(next_click_count(MULTI_CLICK_MS + 1, false, 1), 1);
    }

    #[test]
    fn click_far_from_previous_resets_even_when_fast() {
        // CA-62/CA-82: a fast second click in a different cell is a fresh single
        // click, not a double/triple.
        assert_eq!(next_click_count(50, true, 1), 1);
        assert_eq!(next_click_count(50, true, 2), 1);
    }

    #[test]
    fn click_same_spot_and_fast_advances() {
        assert_eq!(next_click_count(50, false, 1), 2);
    }

    #[test]
    fn selection_copyable_ignores_whitespace_only() {
        // CA-42: only a selection with a non-whitespace char may hit the clipboard.
        assert!(selection_is_copyable("x"));
        assert!(selection_is_copyable("  a  "));
        assert!(!selection_is_copyable(""));
        assert!(!selection_is_copyable("   "));
        assert!(!selection_is_copyable("\t\r\n  "));
    }

    // --- CA-28 tab button hit-test -------------------------------------------

    #[test]
    fn tab_button_close_hit() {
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
        assert_eq!(tab_button_at(lens, cw, 5, w), None);
    }

    #[test]
    fn tab_button_new_tab_hit() {
        let cw = 10usize;
        let lens = [2usize];
        let w = 1000;
        assert_eq!(tab_button_at(lens, cw, 55, w), Some(TabHit::New));
        assert_eq!(tab_button_at(lens, cw, 64, w), Some(TabHit::New));
    }

    #[test]
    fn tab_button_close_second_tab() {
        let cw = 10usize;
        let lens = [2usize, 3usize];
        let w = 1000;
        assert_eq!(tab_button_at(lens, cw, 105, w), Some(TabHit::Close(1)));
        assert_eq!(tab_button_at(lens, cw, 114, w), Some(TabHit::Close(1)));
    }

    #[test]
    fn tab_button_overflow_stops_at_window_edge() {
        let cw = 10usize;
        let lens = [2usize];
        let w = 5;
        assert_eq!(tab_button_at(lens, cw, 0, w), None);
        assert_eq!(tab_button_at(lens, cw, 4, w), None);
    }

    // --- Tab tear-off decision ------------------------------------------------

    #[test]
    fn tear_off_requires_armed_outside_and_multiple_tabs() {
        assert!(should_tear_off(true, true, 2));
        assert!(should_tear_off(true, true, 5));
    }

    #[test]
    fn tear_off_rejected_when_not_armed_or_inside_or_single_tab() {
        assert!(!should_tear_off(false, true, 2)); // never left the window
        assert!(!should_tear_off(true, false, 2)); // released inside
        assert!(!should_tear_off(true, true, 1)); // only tab — nothing to split off
        assert!(!should_tear_off(false, false, 1));
    }

    // --- RT-26 aggregate restored-pane budget --------------------------------

    #[test]
    fn restore_budget_allows_up_to_cap_and_blocks_overflow() {
        // Empty start: a window at exactly the cap fits; one over does not.
        assert!(!restored_panes_over_budget(0, MAX_RESTORED_PANES));
        assert!(restored_panes_over_budget(0, MAX_RESTORED_PANES + 1));
    }

    #[test]
    fn restore_budget_is_aggregate_across_windows() {
        // A crafted session of many full 64-leaf tabs across windows is bounded by
        // the *running total*, not each window independently: once the budget is
        // (nearly) spent, the next window is refused even though it alone is small.
        let mut restored = 0usize;
        let mut windows_kept = 0usize;
        // 16 windows × 64 panes each = 1024 panes; only the first few fit under 256.
        for _ in 0..MAX_WINDOWS {
            let window_panes = 64;
            if restored_panes_over_budget(restored, window_panes) {
                break;
            }
            restored += window_panes;
            windows_kept += 1;
        }
        assert_eq!(windows_kept, MAX_RESTORED_PANES / 64);
        assert!(restored <= MAX_RESTORED_PANES);
    }

    // --- RT-137 runtime creation caps ----------------------------------------

    #[test]
    fn tab_cap_refuses_at_max_tabs() {
        assert!(!tab_cap_reached(0));
        assert!(!tab_cap_reached(MAX_TABS - 1));
        assert!(tab_cap_reached(MAX_TABS));
        assert!(tab_cap_reached(MAX_TABS + 1));
    }

    #[test]
    fn pane_cap_refuses_at_max_panes_per_tab() {
        assert!(!pane_cap_reached(1));
        assert!(!pane_cap_reached(MAX_PANES_PER_TAB - 1));
        assert!(pane_cap_reached(MAX_PANES_PER_TAB));
        assert!(pane_cap_reached(MAX_PANES_PER_TAB + 5));
    }

    #[test]
    fn window_cap_refuses_at_max_windows() {
        assert!(!window_cap_reached(1));
        assert!(!window_cap_reached(MAX_WINDOWS - 1));
        assert!(window_cap_reached(MAX_WINDOWS));
        assert!(window_cap_reached(MAX_WINDOWS + 1));
    }

    // --- CA-33 hyperlink scheme sanitizer ------------------------------------

    #[test]
    fn hyperlink_scheme_allows_only_http_https() {
        assert!(is_safe_hyperlink_scheme("http://example.com"));
        assert!(is_safe_hyperlink_scheme("https://example.com/path?q=1"));
    }

    #[test]
    fn hyperlink_scheme_rejects_other_schemes() {
        assert!(!is_safe_hyperlink_scheme("javascript:alert(1)"));
        assert!(!is_safe_hyperlink_scheme("data:text/html,<h1>hi</h1>"));
        assert!(!is_safe_hyperlink_scheme("ftp://example.com/file"));
        assert!(!is_safe_hyperlink_scheme("mailto:user@example.com"));
        assert!(!is_safe_hyperlink_scheme("ssh://host/path"));
        assert!(!is_safe_hyperlink_scheme(""));
        assert!(!is_safe_hyperlink_scheme("noscheme"));
    }

    #[test]
    fn hyperlink_scheme_rejects_file_urls() {
        // RT-25/RT-40: `file://` reaches ShellExecuteW's `open` verb, which
        // *executes* the target. A local exe/script or a UNC share (no
        // pre-existing local file needed) must never be Ctrl-click launchable.
        assert!(!is_safe_hyperlink_scheme("file:///C:/Users/foo/bar.txt"));
        assert!(!is_safe_hyperlink_scheme(
            "file:///C:/Windows/System32/calc.exe"
        ));
        assert!(!is_safe_hyperlink_scheme(
            "file://attacker-host/share/payload.exe"
        ));
        assert!(!is_safe_hyperlink_scheme("file:///tmp/report.html"));
    }

    #[test]
    fn hyperlink_scheme_case_insensitive() {
        assert!(is_safe_hyperlink_scheme("HTTP://example.com"));
        assert!(is_safe_hyperlink_scheme("HTTPS://example.com"));
        assert!(!is_safe_hyperlink_scheme("FILE:///tmp/foo"));
        assert!(!is_safe_hyperlink_scheme("FTP://example.com"));
    }

    // --- RT-16 hyperlink cell clamp/bounds -----------------------------------

    #[test]
    fn hyperlink_cell_clamps_trailing_partial_column() {
        // 83px wide pane, 10px cells, 8 cols (80px) → the trailing 3px partial
        // column would compute col 8 (== cols), which must clamp to 7, not panic.
        let (line, col) = hyperlink_cell(82, 0, 10, 20, 8, 24, 0, 0).expect("in bounds");
        assert_eq!(col, 7);
        assert_eq!(line, 0);
    }

    #[test]
    fn hyperlink_cell_maps_interior_pixel() {
        let (line, col) = hyperlink_cell(25, 40, 10, 20, 80, 24, 0, 0).expect("in bounds");
        assert_eq!((line, col), (2, 2));
    }

    #[test]
    fn hyperlink_cell_rejects_line_past_history() {
        // Scrolled up by 10 with only 3 lines of history: row 0 → line -10, which
        // is past the -3 history bound, so the cell must be rejected (no panic).
        assert!(hyperlink_cell(0, 0, 10, 20, 80, 24, 3, 10).is_none());
        // Within history (offset 3 ≤ history 3) it maps fine.
        assert_eq!(hyperlink_cell(0, 0, 10, 20, 80, 24, 3, 3), Some((-3, 0)));
    }

    #[test]
    fn hyperlink_cell_empty_grid_is_none() {
        assert!(hyperlink_cell(0, 0, 10, 20, 0, 24, 0, 0).is_none());
        assert!(hyperlink_cell(0, 0, 10, 20, 80, 0, 0, 0).is_none());
    }

    // --- RT-110 new-tab failure fatality -------------------------------------

    #[test]
    fn new_tab_failure_fatal_only_at_cold_start() {
        assert!(new_tab_failure_is_fatal(false)); // no tab alive → cold start → exit
        assert!(!new_tab_failure_is_fatal(true)); // tabs alive → keep them, non-fatal
    }

    // --- RT-8 control-byte predicate -----------------------------------------

    #[test]
    fn is_signal_byte_identifies_etx_eot_sub() {
        assert!(is_broadcast_signal_byte(0x03));
        assert!(is_broadcast_signal_byte(0x04));
        assert!(is_broadcast_signal_byte(0x1a));
    }

    #[test]
    fn is_signal_byte_rejects_normal_bytes() {
        assert!(!is_broadcast_signal_byte(b'a'));
        assert!(!is_broadcast_signal_byte(0x0d));
        assert!(!is_broadcast_signal_byte(0x09));
        assert!(!is_broadcast_signal_byte(0x1b));
    }
}
