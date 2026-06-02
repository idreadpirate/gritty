// ConPTY-backed child process via portable-pty (WezTerm's crate).
// Output is drained on a background thread into a channel; input is written
// directly. This keeps the UI thread free of blocking reads.

use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver};
use std::thread;

use anyhow::Result;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
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

        // The slave handle must drop or the child never sees EOF on exit.
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;

        let (tx, rx) = channel::<Vec<u8>>();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                        waker();
                    }
                }
            }
        });

        Ok(Self {
            master: pair.master,
            writer,
            killer,
            rx,
        })
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
            match pty.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(chunk) => {
                    out.extend_from_slice(&chunk);
                    if String::from_utf8_lossy(&out).contains("gritty_ok") {
                        return; // success
                    }
                }
                Err(_) => {}
            }
        }
        panic!(
            "did not see echoed text; got: {:?}",
            String::from_utf8_lossy(&out)
        );
    }
}
