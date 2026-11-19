//! Two libssh2 sessions per interactive tab, each owned by one thread.
//!
//! libssh2 sessions are not safe to use from two threads. A blocking exec or
//! SFTP transfer on the session that owns the shell also stops PTY reads and
//! writes until it returns, because that work used to be a `Call` on the same
//! actor. The shell therefore has its own session and a mailbox that can only
//! write, resize, or close. Metrics and SFTP use a second session — same
//! login, not a second tab — and a separate actor. A probe, a directory
//! listing, or a transfer can occupy only that second actor.

use serde_json::json;
use ssh2::{Channel, Session};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

enum PtyMsg {
    Write(Vec<u8>),
    Resize(u32, u32),
    Close,
}

enum ActorMsg<J> {
    Job(J),
    Close,
}

type SideJob = Box<dyn FnOnce(&Session) + Send>;

#[derive(Clone)]
pub struct PtySsh {
    tx: Sender<PtyMsg>,
}

#[derive(Clone)]
pub struct SideSsh {
    tx: Sender<ActorMsg<SideJob>>,
}

impl PtySsh {
    pub fn spawn(
        session: Session,
        channel: Channel,
        raw_tx: TermRawTx,
        events: broadcast::Sender<String>,
        id: String,
        stop: Arc<AtomicBool>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name(format!("ssh-{id}"))
            .spawn(move || {
                let shell = SshShell {
                    session,
                    channel,
                    raw_tx,
                    events,
                    id,
                    buf: [0u8; 8192],
                };
                pty_actor_loop(shell, rx, stop);
            })
            .expect("无法启动 SSH 会话线程");
        Self { tx }
    }

    pub fn write(&self, data: &[u8]) -> bool {
        self.tx.send(PtyMsg::Write(data.to_vec())).is_ok()
    }

    pub fn resize(&self, cols: u32, rows: u32) {
        let _ = self.tx.send(PtyMsg::Resize(cols, rows));
    }

    pub fn close(&self) {
        let _ = self.tx.send(PtyMsg::Close);
    }
}

impl SideSsh {
    pub fn spawn(session: Session, id: String) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name(format!("ssh-side-{id}"))
            .spawn(move || side_actor(session, rx))
            .expect("无法启动 SSH 监控线程");
        Self { tx }
    }

    pub fn call<T: Send + 'static>(
        &self,
        op: impl FnOnce(&Session) -> T + Send + 'static,
    ) -> Result<T, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let job: SideJob = Box::new(move |session| {
            let _ = reply_tx.send(op(session));
        });
        self.tx
            .send(ActorMsg::Job(job))
            .map_err(|_| "SSH 会话已经关闭".to_string())?;
        reply_rx.recv().map_err(|_| "SSH 会话已经关闭".to_string())
    }

    pub fn close(&self) {
        let _ = self.tx.send(ActorMsg::Close);
    }
}

pub(crate) type TermRawTx = std::sync::mpsc::Sender<(String, Vec<u8>)>;

/// Raw PTY bytes go to one screen thread. That thread is what the browser sees,
/// after the exit-code probe has been removed. Broadcasting the raw chunk here
/// would paint the probe before it could be filtered.
pub(crate) fn emit_data(raw_tx: &TermRawTx, session_id: &str, bytes: &[u8]) {
    let _ = raw_tx.send((session_id.to_string(), bytes.to_vec()));
}

trait ShellIo {
    fn set_blocking(&mut self, blocking: bool);
    fn drain(&mut self) -> bool;
    fn write_all(&mut self, data: &[u8]);
    fn resize(&mut self, cols: u32, rows: u32);
    fn shutdown(&mut self);
}

struct SshShell {
    session: Session,
    channel: Channel,
    raw_tx: TermRawTx,
    events: broadcast::Sender<String>,
    id: String,
    buf: [u8; 8192],
}

impl ShellIo for SshShell {
    fn set_blocking(&mut self, blocking: bool) {
        self.session.set_blocking(blocking);
    }

    fn drain(&mut self) -> bool {
        drain_shell(&mut self.channel, &mut self.buf, &self.raw_tx, &self.id)
    }

    fn write_all(&mut self, data: &[u8]) {
        let _ = self.channel.write_all(data);
        let _ = self.channel.flush();
    }

    fn resize(&mut self, cols: u32, rows: u32) {
        let _ = self.channel.request_pty_size(cols, rows, None, None);
    }

    fn shutdown(&mut self) {
        let _ = self.channel.close();
        let _ = self.channel.wait_close();
        let _ = self
            .events
            .send(json!({"type": "term:closed", "sessionId": self.id}).to_string());
    }
}

fn pty_actor_loop<I: ShellIo>(mut io: I, rx: Receiver<PtyMsg>, stop: Arc<AtomicBool>) {
    io.set_blocking(false);
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if !io.drain() {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(12)) {
            Ok(PtyMsg::Write(data)) => {
                io.set_blocking(true);
                io.write_all(&data);
                io.set_blocking(false);
            }
            Ok(PtyMsg::Resize(cols, rows)) => {
                io.set_blocking(true);
                io.resize(cols, rows);
                io.set_blocking(false);
            }
            Ok(PtyMsg::Close) => break,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    io.set_blocking(true);
    io.shutdown();
}

fn side_actor(session: Session, rx: Receiver<ActorMsg<SideJob>>) {
    session.set_blocking(true);
    side_actor_loop(rx, |job| job(&session));
}

fn side_actor_loop<J>(rx: Receiver<ActorMsg<J>>, mut run: impl FnMut(J)) {
    while let Ok(msg) = rx.recv() {
        match msg {
            ActorMsg::Job(job) => run(job),
            ActorMsg::Close => break,
        }
    }
}

fn drain_shell(
    channel: &mut Channel,
    buf: &mut [u8],
    raw_tx: &TermRawTx,
    id: &str,
) -> bool {
    loop {
        match channel.read(buf) {
            Ok(0) => return false,
            Ok(n) => emit_data(raw_tx, id, &buf[..n]),
            Err(err) if is_retry(&err) => break,
            Err(_) => return false,
        }
    }
    let mut stderr = channel.stderr();
    match stderr.read(buf) {
        Ok(n) if n > 0 => emit_data(raw_tx, id, &buf[..n]),
        Ok(_) => {}
        Err(err) if is_retry(&err) => {}
        Err(_) => {}
    }
    true
}

fn is_retry(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    ) || err.to_string().to_ascii_lowercase().contains("eagain")
        || err.to_string().to_ascii_lowercase().contains("would block")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::Mutex;
    use std::time::Instant;

    struct FakeShell {
        drains: Arc<AtomicU64>,
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl ShellIo for FakeShell {
        fn set_blocking(&mut self, _blocking: bool) {}

        fn drain(&mut self) -> bool {
            self.drains.fetch_add(1, Ordering::SeqCst);
            true
        }

        fn write_all(&mut self, data: &[u8]) {
            self.writes.lock().expect("writes").push(data.to_vec());
        }

        fn resize(&mut self, _cols: u32, _rows: u32) {}

        fn shutdown(&mut self) {}
    }

    struct Release {
        release: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        side_tx: Sender<ActorMsg<Box<dyn FnOnce() + Send>>>,
    }

    impl Drop for Release {
        fn drop(&mut self) {
            self.release.store(true, Ordering::SeqCst);
            self.stop.store(true, Ordering::SeqCst);
            let _ = self.side_tx.send(ActorMsg::Close);
        }
    }

    /// Metrics probes and SFTP listings are side jobs (`SideSsh::call` →
    /// `side_actor_loop`). The shell actor only drains and writes. The side
    /// job below does not return until the test has observed the PTY write, so
    /// a single shared actor would deadlock instead of recording that write.
    #[test]
    fn metrics_or_sftp_job_does_not_block_pty_write_or_drain() {
        let stop = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let drains = Arc::new(AtomicU64::new(0));
        let writes = Arc::new(Mutex::new(Vec::new()));
        let (pty_tx, pty_rx) = mpsc::channel();
        let (side_tx, side_rx) = mpsc::channel::<ActorMsg<Box<dyn FnOnce() + Send>>>();

        let started_job = Arc::clone(&started);
        let release_job = Arc::clone(&release);
        let finished_job = Arc::clone(&finished);
        let drains_loop = Arc::clone(&drains);
        let writes_loop = Arc::clone(&writes);
        let stop_loop = Arc::clone(&stop);

        std::thread::scope(|scope| {
            scope.spawn(move || {
                pty_actor_loop(
                    FakeShell {
                        drains: drains_loop,
                        writes: writes_loop,
                    },
                    pty_rx,
                    stop_loop,
                );
            });
            scope.spawn(move || side_actor_loop(side_rx, |job| job()));
            let _release = Release {
                release: Arc::clone(&release),
                stop: Arc::clone(&stop),
                side_tx: side_tx.clone(),
            };

            side_tx
                .send(ActorMsg::Job(Box::new(move || {
                    started_job.store(true, Ordering::SeqCst);
                    while !release_job.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    finished_job.store(true, Ordering::SeqCst);
                })))
                .expect("side mailbox open");

            let wait_start = Instant::now();
            while !started.load(Ordering::SeqCst) {
                assert!(
                    wait_start.elapsed() < Duration::from_secs(2),
                    "side job did not start"
                );
                std::thread::sleep(Duration::from_millis(5));
            }

            assert!(pty_tx.send(PtyMsg::Write(b"uname\n".to_vec())).is_ok());
            let wait_write = Instant::now();
            loop {
                let seen = writes
                    .lock()
                    .expect("writes")
                    .iter()
                    .any(|chunk| chunk == b"uname\n");
                if seen {
                    break;
                }
                assert!(
                    wait_write.elapsed() < Duration::from_millis(800),
                    "pty write sat behind the side job"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(
                !finished.load(Ordering::SeqCst),
                "side job finished before the pty write"
            );

            let drains_at_write = drains.load(Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(120));
            assert!(
                drains.load(Ordering::SeqCst) > drains_at_write,
                "pty drain stopped while the side job was still running"
            );
            assert!(
                !finished.load(Ordering::SeqCst),
                "side job ended while checking pty drain"
            );
        });
    }
}
