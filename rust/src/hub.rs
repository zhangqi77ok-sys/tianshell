use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};
use ssh2::{Channel, Session};
use tokio::sync::broadcast;
use uuid::Uuid;
use crate::pty::LocalShellSession;
use crate::ssh::{connect_chain, SshTarget};
use crate::state::lock;
use crate::store::HostStore;

#[derive(Debug, Clone, Serialize)]
pub struct ConnectInfo {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub alias: String,
    pub username: String,
    pub host: String,
    pub port: u16,
    pub kind: String,
}

enum ShellCmd {
    Write(Vec<u8>),
    Resize(u32, u32),
    Close,
}

#[derive(Clone)]
enum TabIo {
    Local(Arc<Mutex<LocalShellSession>>),
    Remote(mpsc::Sender<ShellCmd>),
}

struct Tab {
    info: ConnectInfo,
    host_id: Option<String>,
    io: TabIo,
    stop: Arc<AtomicBool>,
}

pub struct SessionHub {
    tabs: Mutex<HashMap<String, Tab>>,
    events: broadcast::Sender<String>,
    interval: Arc<AtomicU64>,
}

impl SessionHub {
    pub fn new(interval_ms: u64) -> Arc<Self> {
        let (events, _) = broadcast::channel(256);
        Arc::new(Self {
            tabs: Mutex::new(HashMap::new()),
            events,
            interval: Arc::new(AtomicU64::new(interval_ms.clamp(1000, 10_000))),
        })
    }

    pub fn set_interval(&self, ms: u64) {
        self.interval.store(ms.clamp(1000, 10_000), Ordering::Relaxed);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.events.subscribe()
    }

    pub fn broadcast(&self, event: Value) {
        let _ = self.events.send(event.to_string());
    }

    pub fn list_sessions(&self) -> Vec<ConnectInfo> {
        lock(&self.tabs).values().map(|tab| tab.info.clone()).collect()
    }

    pub fn host_id(&self, session_id: &str) -> Result<Option<String>, String> {
        let tabs = lock(&self.tabs);
        let tab = tabs.get(session_id).ok_or_else(|| "找不到这个会话".to_string())?;
        Ok(tab.host_id.clone())
    }

    pub fn open_local(&self) -> Result<ConnectInfo, String> {
        let (mut session, mut reader) = LocalShellSession::spawn(None).map_err(|err| err.to_string())?;
        let stderr = session.take_stderr();
        let session = Arc::new(Mutex::new(session));
        let id = Uuid::new_v4().to_string();
        let events = self.events.clone();
        let id_reader = id.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => emit_data(&events, &id_reader, &buf[..n]),
                    Err(_) => break,
                }
            }
            let _ = events.send(json!({"type": "term:closed", "sessionId": id_reader}).to_string());
        });
        if let Some(mut stderr) = stderr {
            let events = self.events.clone();
            let id_err = id.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                loop {
                    match stderr.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => emit_data(&events, &id_err, &buf[..n]),
                    }
                }
            });
        }
        let info = ConnectInfo {
            session_id: id.clone(),
            alias: "本机终端".into(),
            username: "local".into(),
            host: "localhost".into(),
            port: 0,
            kind: "local".into(),
        };
        lock(&self.tabs).insert(
            id,
            Tab {
                info: info.clone(),
                host_id: None,
                io: TabIo::Local(session),
                stop: Arc::new(AtomicBool::new(false)),
            },
        );
        self.broadcast(json!({"type": "session:opened", "session": info}));
        Ok(info)
    }

    pub fn open_ssh(&self, hosts: &Arc<Mutex<HostStore>>, host_id: &str, cols: u32, rows: u32) -> Result<ConnectInfo, String> {
        let chain = {
            let guard = lock(hosts);
            crate::ssh::resolve_targets(&guard, host_id)?
        };
        let leaf = chain.last().cloned().ok_or_else(|| "跳板机链为空".to_string())?;
        let (session, learned) = connect_chain(&chain).map_err(|err| err.to_string())?;
        {
            let mut guard = lock(hosts);
            for (id, fingerprint) in &learned {
                if !fingerprint.is_empty() {
                    let _ = guard.remember_fingerprint(id, fingerprint);
                }
            }
        }
        session.set_blocking(true);
        let mut channel = session.channel_session().map_err(|err| err.to_string())?;
        let cols = clamp_dim(cols, 80);
        let rows = clamp_dim(rows, 24);
        channel
            .request_pty("xterm-256color", None, Some((cols, rows, 0, 0)))
            .map_err(|err| format!("无法打开远程终端: {err}"))?;
        channel.shell().map_err(|err| format!("无法启动远程 Shell: {err}"))?;
        session.set_blocking(false);

        let id = Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let events = self.events.clone();
        let actor_id = id.clone();
        let actor_stop = stop.clone();
        std::thread::spawn(move || ssh_actor(session, channel, rx, events, actor_id, actor_stop));

        let metrics_id = id.clone();
        let metrics_stop = stop.clone();
        let metrics_events = self.events.clone();
        let interval = self.interval.clone();
        std::thread::spawn(move || metrics_loop(chain, metrics_events, metrics_stop, interval, metrics_id));

        let info = ConnectInfo {
            session_id: id.clone(),
            alias: leaf.alias.clone(),
            username: leaf.username,
            host: leaf.host,
            port: leaf.port,
            kind: "ssh".into(),
        };
        lock(&self.tabs).insert(
            id,
            Tab {
                info: info.clone(),
                host_id: Some(leaf.id),
                io: TabIo::Remote(tx),
                stop,
            },
        );
        self.broadcast(json!({"type": "session:opened", "session": info}));
        Ok(info)
    }

    pub fn close_session(&self, session_id: &str) {
        let tab = lock(&self.tabs).remove(session_id);
        let Some(tab) = tab else { return };
        self.broadcast(json!({"type": "session:closed", "sessionId": session_id}));
        tab.stop.store(true, Ordering::Relaxed);
        match tab.io {
            TabIo::Local(session) => {
                let _ = lock(&session).kill();
            }
            TabIo::Remote(tx) => {
                let _ = tx.send(ShellCmd::Close);
            }
        }
    }

    pub fn close_all(&self) {
        let ids: Vec<String> = lock(&self.tabs).keys().cloned().collect();
        for id in ids {
            self.close_session(&id);
        }
    }

    pub fn dispatch(&self, raw: &str) {
        let Ok(msg) = serde_json::from_str::<Value>(raw) else { return };
        let kind = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "term:input" => {
                if let (Some(id), Some(data)) = (
                    msg.get("sessionId").and_then(|v| v.as_str()),
                    msg.get("data").and_then(|v| v.as_str()),
                ) {
                    let _ = self.write(id, data);
                }
            }
            "term:broadcast" => {
                let data = msg.get("data").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(ids) = msg.get("sessionIds").and_then(|v| v.as_array()) {
                    for id in ids.iter().filter_map(|v| v.as_str()) {
                        let _ = self.write(id, data);
                    }
                }
            }
            "term:resize" => {
                if let Some(id) = msg.get("sessionId").and_then(|v| v.as_str()) {
                    let cols = msg.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u32;
                    let rows = msg.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u32;
                    self.resize(id, cols, rows);
                }
            }
            _ => {}
        }
    }

    fn write(&self, session_id: &str, data: &str) -> bool {
        let io = {
            let tabs = lock(&self.tabs);
            tabs.get(session_id).map(|tab| tab.io.clone())
        };
        match io {
            Some(TabIo::Local(session)) => lock(&session).write(data.as_bytes()).is_ok(),
            Some(TabIo::Remote(tx)) => tx.send(ShellCmd::Write(data.as_bytes().to_vec())).is_ok(),
            None => false,
        }
    }

    fn resize(&self, session_id: &str, cols: u32, rows: u32) {
        let io = {
            let tabs = lock(&self.tabs);
            tabs.get(session_id).map(|tab| tab.io.clone())
        };
        if let Some(TabIo::Remote(tx)) = io {
            let _ = tx.send(ShellCmd::Resize(clamp_dim(cols, 80), clamp_dim(rows, 24)));
        }
    }
}

fn clamp_dim(value: u32, fallback: u32) -> u32 {
    if value == 0 { fallback } else { value.min(512) }
}

fn emit_data(events: &broadcast::Sender<String>, session_id: &str, bytes: &[u8]) {
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    let _ = events.send(json!({"type": "term:data", "sessionId": session_id, "data": data}).to_string());
}

fn is_retry(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted
    ) || err.to_string().to_ascii_lowercase().contains("eagain")
        || err.to_string().to_ascii_lowercase().contains("would block")
}

fn ssh_actor(
    session: Session,
    mut channel: Channel,
    rx: mpsc::Receiver<ShellCmd>,
    events: broadcast::Sender<String>,
    id: String,
    stop: Arc<AtomicBool>,
) {
    let mut buf = [0u8; 8192];
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let mut eof = false;
        loop {
            match channel.read(&mut buf) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => emit_data(&events, &id, &buf[..n]),
                Err(err) if is_retry(&err) => break,
                Err(_) => {
                    eof = true;
                    break;
                }
            }
        }
        if eof {
            break;
        }
        {
            let mut stderr = channel.stderr();
            match stderr.read(&mut buf) {
                Ok(n) if n > 0 => emit_data(&events, &id, &buf[..n]),
                Ok(_) => {}
                Err(err) if is_retry(&err) => {}
                Err(_) => {}
            }
        }
        match rx.recv_timeout(Duration::from_millis(12)) {
            Ok(ShellCmd::Write(data)) => {
                session.set_blocking(true);
                let _ = channel.write_all(&data);
                let _ = channel.flush();
                session.set_blocking(false);
            }
            Ok(ShellCmd::Resize(cols, rows)) => {
                session.set_blocking(true);
                let _ = channel.request_pty_size(cols, rows, None, None);
                session.set_blocking(false);
            }
            Ok(ShellCmd::Close) => break,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    session.set_blocking(true);
    let _ = channel.close();
    let _ = events.send(json!({"type": "term:closed", "sessionId": id}).to_string());
}

fn metrics_loop(
    chain: Vec<SshTarget>,
    events: broadcast::Sender<String>,
    stop: Arc<AtomicBool>,
    interval: Arc<AtomicU64>,
    session_id: String,
) {
    while !stop.load(Ordering::Relaxed) {
        if let Ok((session, _)) = connect_chain(&chain) {
            if let Ok(metrics) = crate::monitor::sample_remote_metrics(&session) {
                if let Ok(value) = serde_json::to_value(&metrics) {
                    let _ = events.send(
                        json!({"type": "metrics:update", "sessionId": session_id, "metrics": value}).to_string(),
                    );
                }
            }
        }
        let wait = interval.load(Ordering::Relaxed).max(1000);
        let steps = (wait / 200).max(1);
        for _ in 0..steps {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}
