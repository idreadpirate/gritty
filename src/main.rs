// gritty — M1: render text via fontdue glyph cache into a CPU framebuffer.

mod font;
mod pty;
mod render;

use std::num::NonZeroU32;
use std::rc::Rc;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use font::FontAtlas;
use render::{draw_cell, Cell};

const BG: u32 = 0x0018_1818;
const FG: u32 = 0x00D0_D0D0;

struct Gritty {
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    _context: Option<softbuffer::Context<Rc<Window>>>,
    font: FontAtlas,
}

impl Gritty {
    fn new() -> Self {
        Self {
            window: None,
            surface: None,
            _context: None,
            font: FontAtlas::new(18.0),
        }
    }
}

impl ApplicationHandler for Gritty {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("gritty")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 600.0));
        let window = Rc::new(event_loop.create_window(attrs).expect("create window"));

        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface = softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");

        self.window = Some(window);
        self.surface = Some(surface);
        self._context = Some(context);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                let Some(window) = self.window.as_ref() else { return };
                let size = window.inner_size();
                let (w, h) = (size.width.max(1), size.height.max(1));

                let Some(surface) = self.surface.as_mut() else { return };
                surface
                    .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
                    .expect("resize");
                let mut buffer = surface.buffer_mut().expect("buffer");
                buffer.fill(BG);

                let stride = w as usize;
                let height = h as usize;

                let lines = [
                    "gritty — native windows terminal",
                    "",
                    "M1: fontdue glyph rendering works.",
                    "next: ConPTY shell, then copy/paste.",
                ];
                for (row, line) in lines.iter().enumerate() {
                    for (col, ch) in line.chars().enumerate() {
                        draw_cell(
                            &mut buffer,
                            stride,
                            height,
                            &mut self.font,
                            col,
                            row,
                            Cell { ch, fg: FG, bg: BG },
                        );
                    }
                }

                buffer.present().expect("present");
            }
            _ => {}
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
    let mut app = Gritty::new();
    event_loop.run_app(&mut app).expect("run");
}
