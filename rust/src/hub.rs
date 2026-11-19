use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::Serialize;
use serde_json::{json, Value};
use ssh2::Session;
use tokio::sync::broadcast;
use uuid::Uuid;
use crate::monitor::{metrics_from_samples, PROBE_COMMAND};
use crate::pty::LocalShellSession;
use crate::shared_ssh::{emit_data, SharedSsh};
use crate::ssh::{connect_chain, exec_command};
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

#[derive(Clone)]
enum TabIo {
    Local(Arc<Mutex<LocalShellSession>>),
    Remote(SharedSsh),
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

    pub fn open_local(&self, cols: u32, rows: u32) -> Result<ConnectInfo, String> {
        let (session, mut reader) = LocalShellSession::open(None, cols, rows).map_err(|err| err.to_string())?;
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

        let id = Uuid::new_v4().to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let shared = SharedSsh::spawn(session, channel, self.events.clone(), id.clone(), stop.clone());

        let metrics = shared.clone();
        let metrics_id = id.clone();
        let metrics_stop = stop.clone();
        let metrics_events = self.events.clone();
        let interval = self.interval.clone();
        std::thread::spawn(move || metrics_loop(metrics, metrics_events, metrics_stop, interval, metrics_id));

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
                io: TabIo::Remote(shared),
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
            TabIo::Remote(ssh) => ssh.close(),
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
            Some(TabIo::Remote(ssh)) => ssh.write(data.as_bytes()),
            None => false,
        }
    }

    fn resize(&self, session_id: &str, cols: u32, rows: u32) {
        let io = {
            let tabs = lock(&self.tabs);
            tabs.get(session_id).map(|tab| tab.io.clone())
        };
        match io {
            Some(TabIo::Remote(ssh)) => ssh.resize(clamp_dim(cols, 80), clamp_dim(rows, 24)),
            Some(TabIo::Local(session)) => {
                let _ = lock(&session).resize(clamp_dim(cols, 80), clamp_dim(rows, 24));
            }
            None => {}
        }
    }

    pub fn with_ssh<T: Send + 'static>(
        &self,
        session_id: &str,
        op: impl FnOnce(&Session) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let ssh = self.remote(session_id)?;
        ssh.call(op)?
    }

    pub fn sample_metrics(&self, session_id: &str) -> Result<crate::monitor::ServerMetrics, String> {
        sample_shared(&self.remote(session_id)?)
    }

    fn remote(&self, session_id: &str) -> Result<SharedSsh, String> {
        let tabs = lock(&self.tabs);
        let tab = tabs.get(session_id).ok_or_else(|| "找不到这个会话".to_string())?;
        match &tab.io {
            TabIo::Remote(ssh) => Ok(ssh.clone()),
            TabIo::Local(_) => Err("会话未连接到 SSH 主机".into()),
        }
    }
}

fn clamp_dim(value: u32, fallback: u32) -> u32 {
    if value == 0 { fallback } else { value.min(512) }
}

fn sample_shared(ssh: &SharedSsh) -> Result<crate::monitor::ServerMetrics, String> {
    let started = Instant::now();
    let first = ssh.call(|session| exec_command(session, PROBE_COMMAND).map(|item| item.stdout).map_err(|err| err.to_string()))??;
    std::thread::sleep(Duration::from_millis(200));
    let second = ssh.call(|session| exec_command(session, PROBE_COMMAND).map(|item| item.stdout).map_err(|err| err.to_string()))??;
    Ok(metrics_from_samples(&first, &second, started.elapsed().as_secs_f64()))
}

fn metrics_loop(
    ssh: SharedSsh,
    events: broadcast::Sender<String>,
    stop: Arc<AtomicBool>,
    interval: Arc<AtomicU64>,
    session_id: String,
) {
    while !stop.load(Ordering::Relaxed) {
        if let Ok(metrics) = sample_shared(&ssh) {
            if let Ok(value) = serde_json::to_value(&metrics) {
                let _ = events.send(
                    json!({"type": "metrics:update", "sessionId": session_id, "metrics": value}).to_string(),
                );
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
