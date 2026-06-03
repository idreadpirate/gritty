// gritty — a lightweight, standalone native Windows terminal.
// Multiplexer: tabs + split panes with per-pane names, scrollback, copy/paste.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod background;
mod clipboard;
mod color;
mod config;
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

/// Minimum spacing between repaints (~60 fps). Rendering is single-threaded
/// software rasterization, so each frame of a many-pane window is not free;
/// capping the repaint *rate* keeps bursty PTY output (or many noisy panes) from
/// pegging the event-loop core. 60 fps is smooth for terminal text; raise this
/// (e.g. 33ms / 30 fps) for more headroom on very dense layouts.
pub(crate) const FRAME: std::time::Duration = std::time::Duration::from_millis(16);

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

/// Relaunch gritty fully detached from the terminal/job that started it, then
/// exit the original process. Without this, closing the PowerShell/cmd window
/// used to launch gritty takes every pane down with it: the shell's parent job
/// is reaped on close, and a console-attached child dies with it. We re-exec
/// ourselves with `DETACHED_PROCESS` (no inherited console) and break away from
/// the job, so the surviving instance is owned by no terminal.
///
/// Guarded by an env marker so the detached child runs normally instead of
/// relaunching forever, and compiled out of debug builds so `cargo run` keeps
/// its console (and panic/stderr output) attached during development.
#[cfg(all(windows, not(debug_assertions)))]
fn ensure_detached() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
        PROCESS_INFORMATION, STARTUPINFOW,
    };

    /// Set on the relaunched child so it doesn't detach a second time.
    const MARKER: &str = "GRITTY_DETACHED";

    if std::env::var_os(MARKER).is_some() {
        return; // we are the detached child — run normally
    }
    let Ok(exe) = std::env::current_exe() else {
        return; // can't find ourselves — fall back to running in place
    };

    // Quoted, null-terminated UTF-16 command line (the exe path may contain
    // spaces).
    let cmdline: Vec<u16> = std::iter::once(u16::from(b'"'))
        .chain(exe.as_os_str().encode_wide())
        .chain([u16::from(b'"'), 0])
        .collect();

    // The child inherits our environment block (null lpEnvironment); set the
    // marker first so it sees it and skips its own relaunch. We're exiting
    // momentarily, so mutating our own env is harmless.
    std::env::set_var(MARKER, "1");

    // Spawn detached with no inherited console. Prefer breaking away from the
    // launching terminal's job so a kill-on-close job can't reap us; if the job
    // forbids breakaway, retry without it (still survives a closed console).
    let spawn = |flags: u32| -> bool {
        // CreateProcessW may write into lpCommandLine, so hand it a fresh,
        // owned, mutable copy each attempt.
        let mut cl = cmdline.clone();
        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: cl is a valid, owned, null-terminated UTF-16 buffer; all
        // optional pointers are null; si/pi are correctly sized and zeroed.
        let ok = unsafe {
            CreateProcessW(
                std::ptr::null(),
                cl.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0, // bInheritHandles = FALSE
                flags,
                std::ptr::null(),
                std::ptr::null(),
                &si,
                &mut pi,
            )
        };
        if ok != 0 {
            // We don't wait on the child; release the handles immediately.
            unsafe {
                CloseHandle(pi.hProcess);
                CloseHandle(pi.hThread);
            }
            true
        } else {
            false
        }
    };

    let detached = spawn(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB)
        || spawn(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);

    if detached {
        std::process::exit(0);
    }
    // Relaunch failed entirely — keep running in place rather than refusing to
    // start; the user can still close gritty's own window to exit.
}

#[cfg(not(all(windows, not(debug_assertions))))]
fn ensure_detached() {}

/// Window/taskbar icon, baked from grittyicon.png at build time (64x64 RGBA).
pub(crate) fn load_icon() -> Option<winit::window::Icon> {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/icon_rgba.bin"));
    winit::window::Icon::from_rgba(bytes.to_vec(), 64, 64).ok()
}

fn main() {
    // Detach from the launching terminal before doing anything else, so closing
    // that window can't reap us (and so we never create two windows/sessions).
    ensure_detached();

    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .expect("event loop");
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = Gritty::new(proxy);
    event_loop.run_app(&mut app).expect("run");
}
