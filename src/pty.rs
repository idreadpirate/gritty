// ConPTY-backed child process via portable-pty (WezTerm's crate).
// Output is drained on a background thread into a channel; input is written
// directly. This keeps the UI thread free of blocking reads.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

/// Bounded output queue per pane (~256 * 8 KB = 2 MB max buffered). A flooding
/// shell applies backpressure instead of growing memory without limit.
const QUEUE_DEPTH: usize = 256;

use anyhow::Result;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

/// The ANSI DSR cursor-position query (ESC [ 6 n). portable-pty ≥ 0.9 enables
/// PSUEDOCONSOLE_INHERIT_CURSOR, which causes the ConPTY layer to emit this
/// sequence at startup; the child process blocks until it receives a CPR reply
/// (ESC [ row ; col R).
const DSR_QUERY: &[u8] = b"\x1b[6n";

/// Scans the PTY output stream for the startup DSR cursor-position query,
/// carrying a small tail across read-chunk boundaries so a query split across
/// two `read` calls is still detected (RT-24). The query is only 4 bytes, so a
/// carry of at most `DSR_QUERY.len() - 1` bytes is enough: any real match either
/// lies wholly inside one chunk or straddles the join, and the join window is
/// fully covered by `carry || chunk`.
///
/// CA-44: the scanner also owns the one-shot reply decision. Only the *first*
/// probe — the ConPTY `PSEUDOCONSOLE_INHERIT_CURSOR` startup query that the child
/// blocks on — should be auto-answered with a synthetic `ESC[1;1R`. Any later CPR
/// query (readline/zsh/pwsh prompt reflow, progress UIs, TUIs) must NOT get a
/// hardcoded `1;1`, which would corrupt the program's wrap/redraw math. Folding
/// both behaviours into one type keeps the carry and the one-shot flag in lockstep
/// and makes the whole decision unit-testable.
#[derive(Default)]
struct DsrScanner {
    /// Trailing bytes of the previous chunk that could be the prefix of a
    /// boundary-straddling query. Never longer than `DSR_QUERY.len() - 1`.
    carry: Vec<u8>,
    /// Set once the startup probe has been answered; no further probe is replied
    /// to (CA-44). Scanning also stops once this is true.
    answered: bool,
}

impl DsrScanner {
    /// Feed the next read chunk and decide whether to send the one-shot startup
    /// CPR reply. Returns `true` exactly once — for the first chunk in which a
    /// complete `ESC [ 6 n` (possibly straddling a read boundary, RT-24) is
    /// detected — and `false` for every chunk thereafter (CA-44), even if it
    /// contains another probe. Once answered, the carry is dropped and no further
    /// scanning is performed.
    fn should_reply(&mut self, chunk: &[u8]) -> bool {
        if self.answered {
            return false;
        }
        if self.scan(chunk) {
            self.answered = true;
            self.carry.clear(); // no longer scanning; release the tail
            true
        } else {
            false
        }
    }

    /// Feed the next read chunk; returns true if a complete `ESC [ 6 n` is
    /// present in `carry ++ chunk`. Updates the carry for the next call.
    fn scan(&mut self, chunk: &[u8]) -> bool {
        let tail = DSR_QUERY.len() - 1;
        // Join only the carry with the chunk's leading bytes to test the seam;
        // the chunk's interior is tested directly to avoid a full copy per read.
        let found = if self.carry.is_empty() {
            chunk.windows(DSR_QUERY.len()).any(|w| w == DSR_QUERY)
        } else {
            let mut joined = Vec::with_capacity(self.carry.len() + chunk.len().min(tail));
            joined.extend_from_slice(&self.carry);
            joined.extend_from_slice(&chunk[..chunk.len().min(tail)]);
            let in_seam = joined.windows(DSR_QUERY.len()).any(|w| w == DSR_QUERY);
            in_seam || chunk.windows(DSR_QUERY.len()).any(|w| w == DSR_QUERY)
        };
        // Remember the last `tail` bytes (across the carry+chunk concatenation,
        // so a query that spans three+ reads of 1 byte each is still caught).
        if chunk.len() >= tail {
            self.carry.clear();
            self.carry.extend_from_slice(&chunk[chunk.len() - tail..]);
        } else {
            self.carry.extend_from_slice(chunk);
            let overflow = self.carry.len().saturating_sub(tail);
            if overflow > 0 {
                self.carry.drain(..overflow);
            }
        }
        found
    }
}

/// A Windows Job Object that owns a pane's shell process with
/// `KILL_ON_JOB_CLOSE`, so dropping it terminates the shell *and its whole
/// descendant tree*. `killer.kill()` alone calls `TerminateProcess` on the shell
/// only, orphaning any grandchild (a backgrounded `npm`/`ssh`/agent worker) that
/// re-parented away — a real process leak on every pane close. The job closes
/// that hole: the OS reaps the entire tree when the last handle drops.
///
/// The handle is stored as `isize` (not the raw `HANDLE` pointer) so `Pty` stays
/// `Send`/`Sync` exactly as before, and closed once on drop.
#[cfg(windows)]
struct ProcessJob(isize);

#[cfg(windows)]
impl ProcessJob {
    /// Create a kill-on-close job and assign process `pid` to it. Returns `None`
    /// if any Win32 step fails (no job object on this OS, insufficient rights, or
    /// the process already sits in a job that forbids nesting) — the caller then
    /// degrades to the prior kill-the-shell-only behaviour.
    fn assign(pid: u32) -> Option<Self> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };

        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(info) as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            // PROCESS_SET_QUOTA | PROCESS_TERMINATE are the rights
            // AssignProcessToJobObject requires.
            let proc = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if set == 0 || proc.is_null() {
                CloseHandle(job);
                return None;
            }
            let assigned = AssignProcessToJobObject(job, proc);
            CloseHandle(proc); // the job retains the process; this handle isn't needed
            if assigned == 0 {
                CloseHandle(job);
                return None;
            }
            Some(Self(job as isize))
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessJob {
    fn drop(&mut self) {
        // KILL_ON_JOB_CLOSE: closing the final handle terminates the whole tree.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0 as *mut core::ffi::c_void) };
    }
}

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    alive: Arc<AtomicBool>,
    notified: Arc<AtomicBool>,
    pid: Option<u32>,
    pub rx: Receiver<Vec<u8>>,
    /// CA-/RT-orphans: holds the pane's process tree in a kill-on-close job so a
    /// pane close reaps grandchildren too, not just the shell. Held only for its
    /// `Drop`; `None` when the job couldn't be created (degrades gracefully).
    #[cfg(windows)]
    _job: Option<ProcessJob>,
}

impl Pty {
    /// Spawn `program args...` on a fresh PTY of the given size.
    /// `waker` is called whenever new output arrives, so the UI loop can wake.
    /// When `cwd` is `Some(path)`, the child process starts in that directory.
    pub fn spawn<W>(
        program: &str,
        args: &[&str],
        rows: u16,
        cols: u16,
        waker: W,
        cwd: Option<&str>,
    ) -> Result<Self>
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
        if let Some(dir) = cwd {
            cmd.cwd(dir);
        }
        let child = pair.slave.spawn_command(cmd)?;
        let killer = child.clone_killer();
        let pid = child.process_id();

        // Put the shell in a kill-on-close job *immediately* after spawn so the
        // whole pane tree dies on pane close (see ProcessJob). Done before the
        // shell has run anything, so descendants it later spawns are covered.
        #[cfg(windows)]
        let _job = pid.and_then(ProcessJob::assign);

        // The slave handle must drop or the child never sees EOF on exit.
        drop(pair.slave);

        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        let mut reader = pair.master.try_clone_reader()?;

        let alive = Arc::new(AtomicBool::new(true));
        let alive_reader = alive.clone();
        // Coalesces wakes: the UI is only pinged on the transition from
        // "nothing pending" to "data waiting", collapsing a flood of reads into
        // one wake per drain cycle.
        let notified = Arc::new(AtomicBool::new(false));
        let notified_reader = notified.clone();

        // Share the writer with the reader thread so it can reply to DSR queries.
        // portable-pty 0.9 sets PSUEDOCONSOLE_INHERIT_CURSOR which causes the
        // ConPTY to emit ESC[6n (cursor position request) immediately; the child
        // blocks until it receives a reply.  We synthesise ESC[1;1R here so the
        // child starts without waiting for the real terminal emulator to answer.
        let writer_for_dsr = Arc::clone(&writer);

        let (tx, rx) = sync_channel::<Vec<u8>>(QUEUE_DEPTH);
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            // Owns both the RT-24 boundary-straddle carry and the CA-44 one-shot
            // reply flag: it answers only the first startup probe, then stops.
            let mut dsr = DsrScanner::default();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = &buf[..n];
                        // Auto-reply ONCE to the startup cursor-position DSR
                        // (ESC [ 6 n) so the child shell does not block waiting
                        // for a position report before the UI is live. RT-24: the
                        // scanner carries a tail across reads so a query split
                        // across two chunks is still detected. CA-44: only the
                        // first probe is answered; later CPR queries flow through
                        // to the term engine untouched.
                        if dsr.should_reply(chunk) {
                            if let Ok(mut w) = writer_for_dsr.lock() {
                                let _ = w.write_all(b"\x1b[1;1R");
                                let _ = w.flush();
                            }
                        }
                        if tx.send(chunk.to_vec()).is_err() {
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
            #[cfg(windows)]
            _job,
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
        // write_all to a ConPTY blocks if the child isn't draining stdin; mark it
        // so a UI-thread hang here (e.g. a broadcast to a stuck pane) is named.
        crate::watchdog::mark(crate::watchdog::PTY_WRITE);
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(data);
            let _ = w.flush();
        }
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
    fn dsr_detected_within_a_single_chunk() {
        let mut s = DsrScanner::default();
        assert!(s.scan(b"hello\x1b[6nworld"));
    }

    #[test]
    fn no_dsr_in_plain_data() {
        let mut s = DsrScanner::default();
        assert!(!s.scan(b"just some output\r\n"));
        assert!(!s.scan(b"more output without a query"));
    }

    #[test]
    fn dsr_split_across_a_read_boundary_is_detected() {
        // RT-24: ESC[6 ends one read, n begins the next. A per-chunk scan would
        // miss this; the carry must bridge the seam.
        let mut s = DsrScanner::default();
        assert!(!s.scan(b"prompt\x1b[6"));
        assert!(s.scan(b"npwd"));
    }

    #[test]
    fn dsr_split_with_esc_alone_at_boundary() {
        // The seam can fall anywhere inside the 4-byte query.
        let mut s = DsrScanner::default();
        assert!(!s.scan(b"abc\x1b"));
        assert!(s.scan(b"[6ndef"));
    }

    #[test]
    fn dsr_split_one_byte_per_read() {
        // RT-24 worst case: the query dribbles in one byte at a time across
        // four separate reads. The carry must accumulate the prefix.
        let mut s = DsrScanner::default();
        assert!(!s.scan(b"\x1b"));
        assert!(!s.scan(b"["));
        assert!(!s.scan(b"6"));
        assert!(s.scan(b"n"));
    }

    #[test]
    fn partial_prefix_then_unrelated_data_does_not_false_match() {
        // A near-miss prefix that is never completed must not trigger a reply.
        let mut s = DsrScanner::default();
        assert!(!s.scan(b"\x1b[6"));
        assert!(!s.scan(b"X")); // not 'n' — no query
        assert!(!s.scan(b"more"));
    }

    #[test]
    fn only_the_first_probe_is_answered() {
        // CA-44: the synthetic ESC[1;1R reply is for the ConPTY startup probe
        // only. A second/third CPR query (a prompt or TUI re-querying the cursor
        // mid-session) must NOT be answered with a bogus 1;1 — it has to reach the
        // term engine so the real cursor is reported.
        let mut s = DsrScanner::default();
        assert!(s.should_reply(b"boot\x1b[6n"), "first probe is answered");
        assert!(
            !s.should_reply(b"\x1b[6n"),
            "a later mid-session CPR must NOT be answered"
        );
        assert!(
            !s.should_reply(b"more\x1b[6noutput"),
            "still not answered after the first"
        );
    }

    #[test]
    fn first_probe_answered_even_when_split_across_reads() {
        // CA-44 + RT-24 together: the one-shot reply still fires when the first
        // probe straddles a read boundary, and stays off afterwards.
        let mut s = DsrScanner::default();
        assert!(!s.should_reply(b"hello\x1b[6"), "incomplete: no reply yet");
        assert!(s.should_reply(b"n"), "completed at the seam: answered once");
        assert!(
            !s.should_reply(b"\x1b[6n"),
            "subsequent probe is not answered"
        );
    }

    #[test]
    fn no_reply_until_a_complete_probe_arrives() {
        // Plain output must never trigger the one-shot reply.
        let mut s = DsrScanner::default();
        assert!(!s.should_reply(b"just output\r\n"));
        assert!(!s.should_reply(b"still nothing useful"));
        // The reply is still available for a genuine probe later.
        assert!(s.should_reply(b"\x1b[6n"));
    }

    #[test]
    fn carry_never_exceeds_query_len_minus_one() {
        // Invariant guarding RT-24's bounded memory: the carry holds at most
        // DSR_QUERY.len()-1 bytes regardless of chunk size.
        let mut s = DsrScanner::default();
        for _ in 0..50 {
            let _ = s.scan(b"a very long chunk of plain terminal output bytes");
            assert!(s.carry.len() < DSR_QUERY.len());
        }
        // Also after tiny sub-tail chunks.
        let mut s2 = DsrScanner::default();
        for _ in 0..50 {
            let _ = s2.scan(b"x");
            assert!(s2.carry.len() < DSR_QUERY.len());
        }
    }

    #[test]
    #[ignore = "diagnostic: measures a real shell's idle output rate (DSR storm hunt)"]
    fn idle_shell_output_probe() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        // First shell that exists, mirroring session::shell_candidates() order.
        let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
        let candidates = [
            format!(r"{pf}\PowerShell\7\pwsh.exe"),
            format!(r"{sysroot}\System32\WindowsPowerShell\v1.0\powershell.exe"),
            format!(r"{sysroot}\System32\cmd.exe"),
        ];
        let shell = candidates
            .iter()
            .find(|p| std::path::Path::new(p).exists())
            .expect("a shell exists")
            .clone();

        let wakes = Arc::new(AtomicUsize::new(0));
        let wakes_w = Arc::clone(&wakes);
        let pty = Pty::spawn(
            &shell,
            &[],
            24,
            80,
            move || {
                wakes_w.fetch_add(1, Ordering::Relaxed);
            },
            None,
        )
        .expect("spawn shell");

        // Drain startup output for 1.5s so we measure the steady state, not boot.
        let settle = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < settle {
            pty.mark_drained();
            while pty.rx.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(10));
        }

        // Measure 3s of IDLE — we send the shell nothing.
        wakes.store(0, Ordering::Relaxed);
        let (mut bytes, mut chunks) = (0usize, 0usize);
        let probe = Instant::now() + Duration::from_secs(3);
        while Instant::now() < probe {
            pty.mark_drained();
            while let Ok(c) = pty.rx.try_recv() {
                bytes += c.len();
                chunks += 1;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        let shell_name = shell.rsplit('\\').next().unwrap_or(&shell);
        eprintln!(
            "\n[idle probe] shell={shell_name}  over 3s idle:  bytes={bytes}  chunks={chunks}  wakes={}",
            wakes.load(Ordering::Relaxed)
        );
        eprintln!(
            "[idle probe] => bytes/s={}  wakes/s={}\n",
            bytes / 3,
            wakes.load(Ordering::Relaxed) / 3
        );
    }

    #[test]
    fn conpty_echo_roundtrips() {
        let pty = Pty::spawn("cmd.exe", &["/c", "echo", "gritty_ok"], 24, 80, || {}, None)
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
    fn live_pty_exposes_pid_alive_write_and_resize() {
        // A long-lived interactive shell so the process stays alive while we poke it.
        let mut pty = Pty::spawn("cmd.exe", &[], 24, 80, || {}, None).expect("spawn cmd.exe");
        assert!(pty.is_alive(), "freshly spawned shell should be alive");
        assert!(pty.pid().is_some(), "a spawned child has a pid");

        // write + resize must not panic and must leave the child alive.
        pty.resize(40, 120);
        pty.write(b"echo hi\r\n");
        // give it a moment; it should still be running (we never sent `exit`).
        std::thread::sleep(Duration::from_millis(100));
        assert!(pty.is_alive(), "shell should still be alive after I/O");
    }

    /// The orphan fix: a freshly spawned child must be held in a kill-on-close
    /// job, so closing the pane reaps its whole descendant tree (not just the
    /// shell). This proves the CreateJobObject→SetInformation→OpenProcess→Assign
    /// FFI sequence succeeds end-to-end against a real ConPTY child; the
    /// KILL_ON_JOB_CLOSE semantics themselves are guaranteed by the OS flag.
    #[cfg(windows)]
    #[test]
    fn spawned_child_is_held_in_kill_on_close_job() {
        let pty = Pty::spawn("cmd.exe", &[], 24, 80, || {}, None).expect("spawn cmd.exe");
        assert!(
            pty._job.is_some(),
            "shell should be assigned to a kill-on-close job object"
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
            None,
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
