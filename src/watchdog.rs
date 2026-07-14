// Hang watchdog: turns a silent UI-thread freeze into a `crash.log` line that
// names the phase the UI thread was stuck in.
//
// The UI thread updates a heartbeat as it enters/leaves each event-loop callback
// (and marks finer sub-phases before known-blocking calls — clipboard, PTY
// writes). A background thread watches that heartbeat; if the UI thread stays in
// one phase past `HANG_MS`, it records where. The watchdog runs on its own
// thread, so it still fires while the UI thread is wedged — exactly when an
// in-process panic hook or stderr cannot help.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

// Phase ids — keep in sync with `phase_name`.
pub const WINDOW_EVENT: u8 = 1;
pub const USER_EVENT: u8 = 2;
pub const ABOUT_TO_WAIT: u8 = 3;
pub const UPDATE_PROCS: u8 = 4;
pub const REDRAW: u8 = 5;
pub const DRAIN_PTY: u8 = 6;
pub const CLIPBOARD_COPY: u8 = 7;
pub const CLIPBOARD_PASTE: u8 = 8;
pub const PTY_WRITE: u8 = 9;

/// Millis-since-epoch when the UI thread entered its current top-level callback,
/// or 0 when idle (waiting for OS events). The watchdog only alarms when this is
/// non-zero and stale.
static BUSY_SINCE: AtomicU64 = AtomicU64::new(0);
/// Finest-grained phase the UI thread has marked within the current callback.
static PHASE: AtomicU8 = AtomicU8::new(0);
/// Set once per hang episode so a freeze is logged a single time, not every tick.
static LOGGED: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// RAII marker for a top-level UI callback: starts the hang timer on entry,
/// clears it (back to idle) on drop — so an early return or unwind still resets.
pub struct Active;

pub fn active(phase: u8) -> Active {
    PHASE.store(phase, Ordering::Relaxed);
    BUSY_SINCE.store(now_ms(), Ordering::Relaxed);
    LOGGED.store(false, Ordering::Relaxed);
    Active
}

impl Drop for Active {
    fn drop(&mut self) {
        BUSY_SINCE.store(0, Ordering::Relaxed);
    }
}

/// Update the fine-grained phase within the current callback (no timer reset).
/// Call right before a potentially-blocking operation.
pub fn mark(phase: u8) {
    PHASE.store(phase, Ordering::Relaxed);
}

fn phase_name(p: u8) -> &'static str {
    match p {
        WINDOW_EVENT => "window_event",
        USER_EVENT => "user_event (drain/reap/poll)",
        ABOUT_TO_WAIT => "about_to_wait (resize/redraw-arm)",
        UPDATE_PROCS => "update_procs (proc snapshot + agent detect)",
        REDRAW => "redraw (paint)",
        DRAIN_PTY => "drain_pty (vt feed)",
        CLIPBOARD_COPY => "clipboard copy (arboard set_text)",
        CLIPBOARD_PASTE => "clipboard paste (arboard get_text)",
        PTY_WRITE => "pty write (write_all to ConPTY)",
        _ => "unknown",
    }
}

/// How long the UI thread may stay in one phase before it's deemed hung.
const HANG_MS: u64 = 5000;

/// Spawn the watchdog. Cheap: one thread that sleeps 1s between checks and only
/// touches the filesystem on an actual hang.
pub fn start(log_path: std::path::PathBuf) {
    std::thread::Builder::new()
        .name("gritty-watchdog".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            // RT-138: commit-growth curve logging lives on this thread so it
            // still fires while the UI thread is wedged mid-runaway.
            crate::memguard::poll(&log_path);
            let since = BUSY_SINCE.load(Ordering::Relaxed);
            if since == 0 {
                continue; // idle / waiting — not a hang
            }
            let elapsed = now_ms().saturating_sub(since);
            if elapsed >= HANG_MS && !LOGGED.swap(true, Ordering::Relaxed) {
                log_hang(
                    &log_path,
                    phase_name(PHASE.load(Ordering::Relaxed)),
                    elapsed,
                );
            }
        })
        .ok();
}

fn log_hang(path: &std::path::Path, phase: &str, elapsed_ms: u64) {
    append_line(
        path,
        &format!("HANG: UI thread stuck in {phase} for {elapsed_ms}ms"),
    );
}

/// Best-effort timestamped append to the crash log — shared by the hang
/// logger above and the memory guard (RT-138). Never panics.
pub(crate) fn append_line(path: &std::path::Path, msg: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let secs = now_ms() / 1000;
    let line = format!("[{secs}] {msg}\n");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}
