// gritty — a lightweight, standalone native Windows terminal.
// Multiplexer: tabs + split panes with per-pane names, scrollback, copy/paste.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod background;
mod clipboard;
mod color;
mod font;
mod fuzzy;
mod input;
mod key;
mod layout;
mod paint;
mod palette;
mod persist;
mod proc;
mod pty;
mod render;
mod session;
mod term;

use winit::event_loop::EventLoop;
use winit::window::Window;

use app::Gritty;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Wake;

/// Industrial gunmetal/amber accents — each new tab takes the next one.
pub(crate) const TAB_PALETTE: [u32; 6] = [
    0x00ff_7b00, // molten orange
    0x00e6_9522, // industrial amber
    0x00c0_8a4a, // bronze
    0x00b8_7333, // copper
    0x007f_a6c9, // steel blue
    0x00d4_a017, // gold
];

#[derive(Clone, Copy)]
pub(crate) enum Dir4 {
    Left,
    Right,
    Up,
    Down,
}

/// Minimum spacing between repaints (~120 fps). Bursty PTY output coalesces into
/// at most one frame per interval, so 15 noisy panes can't peg a core.
pub(crate) const FRAME: std::time::Duration = std::time::Duration::from_millis(8);

/// Paint the OS title bar (the caption that shows "gritty") the same
/// indigo-charcoal as the app body with steel-grey text, so the icon sits in a
/// seamless dark header. Windows 11 only; a no-op elsewhere.
#[cfg(windows)]
pub(crate) fn style_caption(window: &Window) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(w) = handle.as_raw() else {
        return;
    };
    let hwnd = w.hwnd.get() as *mut core::ffi::c_void;
    let caption: u32 = 0x001F_1516; // #16151F indigo-charcoal as BBGGRR
    let text: u32 = 0x00D9_D1C9; // #C9D1D9 steel grey as BBGGRR
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR as u32,
            &caption as *const u32 as *const core::ffi::c_void,
            4,
        );
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TEXT_COLOR as u32,
            &text as *const u32 as *const core::ffi::c_void,
            4,
        );
    }
}

#[cfg(not(windows))]
pub(crate) fn style_caption(_window: &Window) {}

/// Window/taskbar icon, baked from grittyicon.png at build time (64x64 RGBA).
pub(crate) fn load_icon() -> Option<winit::window::Icon> {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/icon_rgba.bin"));
    winit::window::Icon::from_rgba(bytes.to_vec(), 64, 64).ok()
}

fn main() {
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .expect("event loop");
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = Gritty::new(proxy);
    event_loop.run_app(&mut app).expect("run");
}
