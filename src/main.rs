// gritty — a lightweight, standalone native Windows terminal.
// M5: copy/paste — mouse selection w/ auto-copy, Ctrl+Shift+C/V, right-click paste.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod color;
mod font;
mod key;
mod pty;
mod render;
mod term;

use std::num::NonZeroU32;
use std::rc::Rc;

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::vte::ansi::CursorShape;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState};
use winit::window::{Window, WindowId};

use clipboard::Clip;
use color::{BG, CURSOR, FG, SELECTION_BG};
use font::FontAtlas;
use pty::Pty;
use render::{draw_cell, Cell};
use term::Terminal;

#[derive(Debug, Clone, Copy)]
struct Wake;

struct Gritty {
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    _context: Option<softbuffer::Context<Rc<Window>>>,
    font: FontAtlas,
    terminal: Option<Terminal>,
    pty: Option<Pty>,
    mods: ModifiersState,
    clip: Clip,
    mouse_pos: (f64, f64),
    selecting: bool,
    proxy: EventLoopProxy<Wake>,
}

impl Gritty {
    fn new(proxy: EventLoopProxy<Wake>) -> Self {
        Self {
            window: None,
            surface: None,
            _context: None,
            font: FontAtlas::new(18.0),
            terminal: None,
            pty: None,
            mods: ModifiersState::empty(),
            clip: Clip::new(),
            mouse_pos: (0.0, 0.0),
            selecting: false,
            proxy,
        }
    }

    fn grid_dims(&self, w: u32, h: u32) -> (usize, usize) {
        let cols = (w as usize / self.font.cell_w).max(1);
        let rows = (h as usize / self.font.cell_h).max(1);
        (cols, rows)
    }

    fn pixel_to_point(&self, x: f64, y: f64) -> (Point, Side) {
        let cw = self.font.cell_w as f64;
        let chh = self.font.cell_h as f64;
        let max_col = self
            .terminal
            .as_ref()
            .map(|t| t.size.cols.saturating_sub(1))
            .unwrap_or(0);
        let col = ((x / cw).floor().max(0.0) as usize).min(max_col);
        let off = self.terminal.as_ref().map(|t| t.display_offset()).unwrap_or(0) as i32;
        let row = (y / chh).floor().max(0.0) as i32;
        let side = if (x % cw) < cw / 2.0 { Side::Left } else { Side::Right };
        (Point::new(Line(row - off), Column(col)), side)
    }

    fn drain_pty(&mut self) {
        let (Some(pty), Some(terminal)) = (self.pty.as_mut(), self.terminal.as_mut()) else {
            return;
        };
        while let Ok(chunk) = pty.rx.try_recv() {
            terminal.feed(&chunk);
        }
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    fn copy_selection(&mut self) {
        if let Some(t) = self.terminal.as_ref() {
            if let Some(text) = t.term.selection_to_string() {
                if !text.is_empty() {
                    self.clip.copy(&text);
                }
            }
        }
    }

    fn paste(&mut self) {
        let Some(text) = self.clip.paste() else { return };
        let bracketed = self.terminal.as_ref().map_or(false, |t| t.bracketed_paste());
        let data = term::wrap_paste(&text, bracketed);
        if let Some(pty) = self.pty.as_mut() {
            pty.write(&data);
        }
    }

    fn redraw(&mut self) {
        let Some(window) = self.window.as_ref() else { return };
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        let stride = w as usize;
        let height = h as usize;

        let Some(surface) = self.surface.as_mut() else { return };
        surface
            .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
            .expect("resize");
        let mut buffer = surface.buffer_mut().expect("buffer");
        buffer.fill(BG);

        if let Some(terminal) = self.terminal.as_ref() {
            let content = terminal.term.renderable_content();
            let selection = content.selection;
            let at_bottom = content.display_offset == 0;
            let cursor_visible = at_bottom && content.cursor.shape != CursorShape::Hidden;
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

                if selection.map_or(false, |r| r.contains(item.point)) {
                    bg = SELECTION_BG;
                } else if cursor_visible && line == cur_row && col as i32 == cur_col {
                    bg = CURSOR;
                    fg = BG;
                }

                draw_cell(
                    &mut buffer,
                    stride,
                    height,
                    &mut self.font,
                    col,
                    row,
                    Cell { ch: cell.c, fg, bg },
                );
            }
        }

        buffer.present().expect("present");
    }
}

impl ApplicationHandler<Wake> for Gritty {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("gritty")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 600.0));
        let window = Rc::new(event_loop.create_window(attrs).expect("create window"));

        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface = softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");

        let size = window.inner_size();
        let (cols, rows) = self.grid_dims(size.width.max(1), size.height.max(1));
        let terminal = Terminal::new(cols, rows);

        let proxy = self.proxy.clone();
        let waker = move || {
            let _ = proxy.send_event(Wake);
        };
        let pty = Pty::spawn("pwsh.exe", &["-NoLogo"], rows as u16, cols as u16, waker.clone())
            .or_else(|_| Pty::spawn("cmd.exe", &[], rows as u16, cols as u16, waker))
            .expect("spawn a native shell");

        self.window = Some(window);
        self.surface = Some(surface);
        self._context = Some(context);
        self.terminal = Some(terminal);
        self.pty = Some(pty);
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: Wake) {
        self.drain_pty();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                // Clipboard shortcuts take priority over byte encoding.
                if self.mods.control_key() && self.mods.shift_key() {
                    if let Key::Character(s) = &event.logical_key {
                        match s.to_lowercase().as_str() {
                            "c" => {
                                self.copy_selection();
                                return;
                            }
                            "v" => {
                                self.paste();
                                return;
                            }
                            _ => {}
                        }
                    }
                }
                if let Some(bytes) = key::encode(&event.logical_key, self.mods) {
                    if let Some(pty) = self.pty.as_mut() {
                        pty.write(&bytes);
                    }
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_pos = (position.x, position.y);
                if self.selecting {
                    let (point, side) = self.pixel_to_point(position.x, position.y);
                    if let Some(t) = self.terminal.as_mut() {
                        if let Some(sel) = t.term.selection.as_mut() {
                            sel.update(point, side);
                        }
                    }
                    if let Some(window) = self.window.as_ref() {
                        window.request_redraw();
                    }
                }
            }

            WindowEvent::MouseInput { state, button, .. } => match (state, button) {
                (ElementState::Pressed, MouseButton::Left) => {
                    let (point, side) = self.pixel_to_point(self.mouse_pos.0, self.mouse_pos.1);
                    if let Some(t) = self.terminal.as_mut() {
                        t.term.selection = Some(Selection::new(SelectionType::Simple, point, side));
                    }
                    self.selecting = true;
                    if let Some(window) = self.window.as_ref() {
                        window.request_redraw();
                    }
                }
                (ElementState::Released, MouseButton::Left) => {
                    self.selecting = false;
                    self.copy_selection();
                }
                (ElementState::Pressed, MouseButton::Right) => self.paste(),
                _ => {}
            },

            WindowEvent::Resized(new) => {
                let (cols, rows) = self.grid_dims(new.width.max(1), new.height.max(1));
                if let Some(terminal) = self.terminal.as_mut() {
                    terminal.resize(cols, rows);
                }
                if let Some(pty) = self.pty.as_ref() {
                    pty.resize(rows as u16, cols as u16);
                }
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }

            WindowEvent::RedrawRequested => self.redraw(),

            _ => {}
        }
    }
}

fn main() {
    let event_loop = EventLoop::<Wake>::with_user_event().build().expect("event loop");
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = Gritty::new(proxy);
    event_loop.run_app(&mut app).expect("run");
}
