// ConPTY-backed child process via portable-pty (WezTerm's crate).
// Output is drained on a background thread into a channel; input is written
// directly. This keeps the UI thread free of blocking reads.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::Arc;
use std::thread;

/// Bounded output queue per pane (~256 * 8 KB = 2 MB max buffered). A flooding
/// shell applies backpressure instead of growing memory without limit.
const QUEUE_DEPTH: usize = 256;

use anyhow::Result;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    alive: Arc<AtomicBool>,
    notified: Arc<AtomicBool>,
    pid: Option<u32>,
    pub rx: Receiver<Vec<u8>>,
}

impl Pty {
    /// Spawn `program args...` on a fresh PTY of the given size.
    /// `waker` is called whenever new output arrives, so the UI loop can wake.
    pub fn spawn<W>(program: &str, args: &[&str], rows: u16, cols: u16, waker: W) -> Result<Self>
    where
        W: Fn() + Send + 'static,
    {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(program);
        for a in args {
            cmd.arg(a);
        }
        let child = pair.slave.spawn_command(cmd)?;
        let killer = child.clone_killer();
        let pid = child.process_id();

        // The slave handle must drop or the child never sees EOF on exit.
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;

        let alive = Arc::new(AtomicBool::new(true));
        let alive_reader = alive.clone();
        // Coalesces wakes: the UI is only pinged on the transition from
        // "nothing pending" to "data waiting", collapsing a flood of reads into
        // one wake per drain cycle.
        let notified = Arc::new(AtomicBool::new(false));
        let notified_reader = notified.clone();

        let (tx, rx) = sync_channel::<Vec<u8>>(QUEUE_DEPTH);
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                        if !notified_reader.swap(true, Ordering::AcqRel) {
                            waker();
                        }
                    }
                }
            }
            // Child exited / PTY closed: mark dead and wake so the UI reaps it.
            // Release/Acquire (see is_alive) so the store is visible before the wake.
            alive_reader.store(false, Ordering::Release);
            waker();
        });

        Ok(Self {
            master: pair.master,
            writer,
            killer,
            alive,
            notified,
            pid,
            rx,
        })
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Call before draining `rx`: re-arms the wake so output arriving during or
    /// after the drain triggers a fresh wake (no lost updates).
    pub fn mark_drained(&self) {
        self.notified.store(false, Ordering::Release);
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Send bytes to the child's stdin.
    pub fn write(&mut self, data: &[u8]) {
        let _ = self.writer.write_all(data);
        let _ = self.writer.flush();
    }

    /// Resize the PTY (call on window resize).
    pub fn resize(&self, rows: u16, cols: u16) {
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.killer.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn conpty_echo_roundtrips() {
        let pty = Pty::spawn("cmd.exe", &["/c", "echo", "gritty_ok"], 24, 80, || {})
            .expect("spawn cmd.exe over ConPTY");

        let mut out = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(chunk) = pty.rx.recv_timeout(Duration::from_millis(200)) {
                out.extend_from_slice(&chunk);
                if String::from_utf8_lossy(&out).contains("gritty_ok") {
                    return; // success
                }
            }
        }
        panic!(
            "did not see echoed text; got: {:?}",
            String::from_utf8_lossy(&out)
        );
    }

    #[test]
    fn high_volume_no_loss_or_deadlock() {
        // 2000 numbered lines: exercises sustained draining without loss/hang.
        let pty = Pty::spawn(
            "cmd.exe",
            &["/c", "for /L %i in (1,1,2000) do @echo line%i"],
            24,
            80,
            || {},
        )
        .expect("spawn");

        let mut out = String::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            pty.mark_drained();
            while let Ok(c) = pty.rx.try_recv() {
                out.push_str(&String::from_utf8_lossy(&c));
            }
            if out.contains("line2000") {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(out.contains("line1"), "missing first line");
        assert!(
            out.contains("line2000"),
            "missing last line (loss or deadlock)"
        );
    }
}
