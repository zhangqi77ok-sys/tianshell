//! One libssh2 session, one thread.
//!
//! libssh2 sessions are not safe to use from two threads. The interactive shell,
//! SFTP, and metric probes for a tab all send work to the thread that owns the
//! session, so login happens once and closing the thread drops every channel.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;
use base64::Engine;
use serde_json::json;
use ssh2::{Channel, Session};
use tokio::sync::broadcast;

enum Msg {
    Write(Vec<u8>),
    Resize(u32, u32),
    Close,
    Call(Box<dyn FnOnce(&Session) + Send>),
}

#[derive(Clone)]
pub struct SharedSsh {
    tx: Sender<Msg>,
}

impl SharedSsh {
    pub fn spawn(
        session: Session,
        channel: Channel,
        events: broadcast::Sender<String>,
        id: String,
        stop: Arc<AtomicBool>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name(format!("ssh-{id}"))
            .spawn(move || ssh_actor(session, channel, rx, events, id, stop))
            .expect("无法启动 SSH 会话线程");
        Self { tx }
    }

    pub fn write(&self, data: &[u8]) -> bool {
        self.tx.send(Msg::Write(data.to_vec())).is_ok()
    }

    pub fn resize(&self, cols: u32, rows: u32) {
        let _ = self.tx.send(Msg::Resize(cols, rows));
    }

    pub fn close(&self) {
        let _ = self.tx.send(Msg::Close);
    }

    pub fn call<T: Send + 'static>(&self, op: impl FnOnce(&Session) -> T + Send + 'static) -> Result<T, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let job: Box<dyn FnOnce(&Session) + Send> = Box::new(move |session| {
            let _ = reply_tx.send(op(session));
        });
        self.tx.send(Msg::Call(job)).map_err(|_| "SSH 会话已经关闭".to_string())?;
        reply_rx.recv().map_err(|_| "SSH 会话已经关闭".to_string())
    }
}

pub(crate) fn emit_data(events: &broadcast::Sender<String>, session_id: &str, bytes: &[u8]) {
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    let _ = events.send(json!({"type": "term:data", "sessionId": session_id, "data": data}).to_string());
}

fn ssh_actor(
    session: Session,
    mut channel: Channel,
    rx: Receiver<Msg>,
    events: broadcast::Sender<String>,
    id: String,
    stop: Arc<AtomicBool>,
) {
    session.set_blocking(false);
    let mut buf = [0u8; 8192];
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if !drain_shell(&mut channel, &mut buf, &events, &id) {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(12)) {
            Ok(Msg::Write(data)) => {
                session.set_blocking(true);
                let _ = channel.write_all(&data);
                let _ = channel.flush();
                session.set_blocking(false);
            }
            Ok(Msg::Resize(cols, rows)) => {
                session.set_blocking(true);
                let _ = channel.request_pty_size(cols, rows, None, None);
                session.set_blocking(false);
            }
            Ok(Msg::Close) => break,
            Ok(Msg::Call(job)) => {
                session.set_blocking(true);
                job(&session);
                session.set_blocking(false);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    session.set_blocking(true);
    let _ = channel.close();
    let _ = channel.wait_close();
    let _ = events.send(json!({"type": "term:closed", "sessionId": id}).to_string());
}

fn drain_shell(channel: &mut Channel, buf: &mut [u8], events: &broadcast::Sender<String>, id: &str) -> bool {
    loop {
        match channel.read(buf) {
            Ok(0) => return false,
            Ok(n) => emit_data(events, id, &buf[..n]),
            Err(err) if is_retry(&err) => break,
            Err(_) => return false,
        }
    }
    let mut stderr = channel.stderr();
    match stderr.read(buf) {
        Ok(n) if n > 0 => emit_data(events, id, &buf[..n]),
        Ok(_) => {}
        Err(err) if is_retry(&err) => {}
        Err(_) => {}
    }
    true
}

fn is_retry(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted
    ) || err.to_string().to_ascii_lowercase().contains("eagain")
        || err.to_string().to_ascii_lowercase().contains("would block")
}
