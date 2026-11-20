use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};
use ssh2::Session;
use tokio::sync::broadcast;
use uuid::Uuid;
use crate::history::{CommandHistory, HistoryDraft};
use crate::monitor::{poll_probe, ProbeHistory, PROBE_COMMAND};
use crate::pty::LocalShellSession;
use crate::screen::{note_cd, parse_end_marker, transcript_before_marker, LineAssembler, Screen, ShellKind};
use crate::shared_ssh::{emit_data, PtySsh, SideSsh};
use crate::ssh::{connect_chain, exec_command, truncate_output};
use crate::state::lock;
use crate::store::HostStore;
use crate::terminal::TerminalView;

#[derive(Debug, Clone, Serialize)]
pub struct ConnectInfo {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub alias: String,
    pub username: String,
    pub host: String,
    pub port: u16,
    pub kind: String,
    #[serde(rename = "hostId", skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
}

#[derive(Clone)]
struct RemoteTab {
    pty: PtySsh,
    side: SideSsh,
    probes: Arc<Mutex<Option<ProbeHistory>>>,
}

#[derive(Clone)]
enum TabIo {
    Local(Arc<Mutex<LocalShellSession>>),
    Remote(RemoteTab),
}

struct Tab {
    info: ConnectInfo,
    host_id: Option<String>,
    io: TabIo,
    stop: Arc<AtomicBool>,
}

struct TabMind {
    shell: ShellKind,
    cwd: Option<String>,
    last_exit: Option<i32>,
    screen: Screen,
    assembler: LineAssembler,
    job: Option<LiveJob>,
    alias: String,
    kind: String,
    host_id: Option<String>,
}

struct LiveJob {
    id: String,
    nonce: String,
    history_id: Option<String>,
    log_mark: u64,
    command: String,
    done: bool,
    cancelled: bool,
    exit_code: Option<i32>,
    cwd: Option<String>,
}

struct HistoryFinish {
    id: String,
    exit_code: Option<i32>,
    cwd: Option<String>,
    ok: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobView {
    #[serde(rename = "jobId")]
    pub job_id: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub command: String,
    pub done: bool,
    pub cancelled: bool,
    #[serde(rename = "timedOut")]
    pub timed_out: bool,
    #[serde(rename = "exitCode")]
    pub exit_code: Option<i32>,
    pub cwd: Option<String>,
    pub stdout: String,
    pub truncated: bool,
    pub streams: String,
}

pub struct SessionHub {
    tabs: Mutex<HashMap<String, Tab>>,
    screens: Arc<Mutex<HashMap<String, Arc<Mutex<TabMind>>>>>,
    job_index: Arc<Mutex<HashMap<String, String>>>,
    finished: Arc<Mutex<HashMap<String, JobView>>>,
    history: Arc<Mutex<Option<Arc<CommandHistory>>>>,
    events: broadcast::Sender<String>,
    interval: Arc<AtomicU64>,
}

impl SessionHub {
    pub fn new(interval_ms: u64) -> Arc<Self> {
        let (events, _) = broadcast::channel::<String>(256);
        let screens = Arc::new(Mutex::new(HashMap::new()));
        let history = Arc::new(Mutex::new(None));
        let finished = Arc::new(Mutex::new(HashMap::new()));
        let pump_screens = screens.clone();
        let pump_history = history.clone();
        let pump_finished = finished.clone();
        let pump_events = events.clone();
        let mut pump_rx = events.subscribe();
        std::thread::Builder::new()
            .name("term-screen".into())
            .spawn(move || {
                loop {
                    match pump_rx.blocking_recv() {
                        Ok(raw) => apply_terminal_output(&raw, &pump_screens, &pump_history, &pump_finished, &pump_events),
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            })
            .expect("无法启动终端回放线程");
        Arc::new(Self {
            tabs: Mutex::new(HashMap::new()),
            screens,
            job_index: Arc::new(Mutex::new(HashMap::new())),
            finished,
            history,
            events,
            interval: Arc::new(AtomicU64::new(interval_ms.clamp(1000, 10_000))),
        })
    }

    pub fn set_history(&self, history: Arc<CommandHistory>) {
        *lock(&self.history) = Some(history);
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
        let cwd = dirs::home_dir().map(|path| path.to_string_lossy().to_string());
        self.register_mind(&id, ShellKind::local_default(), cwd, "local", "本机终端", None);
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
            host_id: None,
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
        // Shell and SFTP/metrics log in with the same chain on two libssh2
        // sessions. A probe or transfer cannot sit on the PTY actor. The
        // password is dropped when `connect_chain` returns.
        let chain_side = chain.clone();
        let side_handshake = std::thread::Builder::new()
            .name("ssh-side-handshake".into())
            .spawn(move || connect_chain(&chain_side))
            .map_err(|err| format!("无法启动监控连接: {err}"))?;
        let pty_result = connect_chain(&chain);
        let side_joined = side_handshake.join();
        let (pty_session, learned) = pty_result.map_err(|err| err.to_string())?;
        let side_result = side_joined.map_err(|_| "监控和 SFTP 连接在握手时中断".to_string())?;
        let (side_session, learned_side) = side_result.map_err(|err| {
            format!("监控和 SFTP 连接失败: {err}。这一步和终端分开登录；失败时不会打开只有一半功能的标签。")
        })?;
        remember_fingerprints(hosts, &learned);
        remember_fingerprints(hosts, &learned_side);

        let cols = clamp_dim(cols, 80);
        let rows = clamp_dim(rows, 24);
        let channel = open_shell(&pty_session, cols, rows)?;

        let id = Uuid::new_v4().to_string();
        self.register_mind(&id, ShellKind::Posix, None, "ssh", &leaf.alias, Some(leaf.id.clone()));
        let stop = Arc::new(AtomicBool::new(false));
        let pty = PtySsh::spawn(pty_session, channel, self.events.clone(), id.clone(), stop.clone());
        let side = SideSsh::spawn(side_session, id.clone());
        let probes = Arc::new(Mutex::new(None));
        let remote = RemoteTab { pty, side: side.clone(), probes: probes.clone() };

        let metrics_id = id.clone();
        let metrics_stop = stop.clone();
        let metrics_events = self.events.clone();
        let interval = self.interval.clone();
        std::thread::spawn(move || metrics_loop(side, probes, metrics_events, metrics_stop, interval, metrics_id));

        let info = ConnectInfo {
            session_id: id.clone(),
            alias: leaf.alias.clone(),
            username: leaf.username,
            host: leaf.host,
            port: leaf.port,
            kind: "ssh".into(),
            host_id: Some(leaf.id.clone()),
        };
        lock(&self.tabs).insert(
            id,
            Tab {
                info: info.clone(),
                host_id: Some(leaf.id),
                io: TabIo::Remote(remote),
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
        if let Some(mind) = lock(&self.screens).remove(session_id) {
            let mut mind = lock(&mind);
            if let Some(job) = mind.job.as_mut() {
                if !job.done {
                    job.done = true;
                    job.cancelled = true;
                }
            }
            if let Some(view) = snapshot(session_id, &mind) {
                lock(&self.finished).insert(view.job_id.clone(), view);
            }
        }
        tab.stop.store(true, Ordering::Relaxed);
        match tab.io {
            TabIo::Local(session) => {
                let _ = lock(&session).kill();
            }
            TabIo::Remote(remote) => {
                remote.pty.close();
                remote.side.close();
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
                    let _ = self.write_input(id, data, "interactive");
                }
            }
            "term:broadcast" => {
                let data = msg.get("data").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(ids) = msg.get("sessionIds").and_then(|v| v.as_array()) {
                    for id in ids.iter().filter_map(|v| v.as_str()) {
                        let _ = self.write_input(id, data, "interactive");
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

    fn resize(&self, session_id: &str, cols: u32, rows: u32) {
        let io = {
            let tabs = lock(&self.tabs);
            tabs.get(session_id).map(|tab| tab.io.clone())
        };
        match io {
            Some(TabIo::Remote(remote)) => remote.pty.resize(clamp_dim(cols, 80), clamp_dim(rows, 24)),
            Some(TabIo::Local(session)) => {
                let _ = lock(&session).resize(clamp_dim(cols, 80), clamp_dim(rows, 24));
            }
            None => {}
        }
    }

    pub fn list_views(&self) -> Vec<TerminalView> {
        let tabs = lock(&self.tabs);
        let screens = lock(&self.screens);
        tabs.values()
            .map(|tab| {
                let mind = screens.get(&tab.info.session_id).map(|mind| lock(mind));
                TerminalView {
                    session_id: tab.info.session_id.clone(),
                    kind: tab.info.kind.clone(),
                    alias: tab.info.alias.clone(),
                    username: tab.info.username.clone(),
                    host: tab.info.host.clone(),
                    port: tab.info.port,
                    host_id: tab.host_id.clone(),
                    cwd: mind.as_ref().and_then(|item| item.cwd.clone()),
                    last_exit: mind.as_ref().and_then(|item| item.last_exit),
                    busy: mind.as_ref().and_then(|item| item.job.as_ref()).map(|job| !job.done).unwrap_or(false),
                }
            })
            .collect()
    }

    pub fn read_screen(&self, session_id: &str, max_chars: usize) -> Result<Value, String> {
        let mind = self.mind(session_id)?;
        let view = self.list_views().into_iter().find(|item| item.session_id == session_id);
        let mind = lock(&mind);
        let (text, truncated) = mind.screen.text(max_chars.clamp(1, 100_000));
        Ok(json!({
            "sessionId": session_id,
            "kind": mind.kind,
            "alias": mind.alias,
            "hostId": mind.host_id,
            "cwd": mind.cwd,
            "lastExit": mind.last_exit,
            "busy": mind.job.as_ref().map(|job| !job.done).unwrap_or(false),
            "text": text,
            "truncated": truncated,
            "attached": view.is_some()
        }))
    }

    pub fn write_input(&self, session_id: &str, data: &str, source: &str) -> Result<(), String> {
        let mind = self.mind(session_id)?;
        let submitted = {
            let mut mind = lock(&mind);
            let lines = mind.assembler.feed(data);
            for line in &lines {
                note_cd(&mut mind.cwd, line);
            }
            lines
        };
        if !submitted.is_empty() {
            let meta = self.tab_meta(session_id);
            if let Some(history) = self.history_handle() {
                for line in submitted {
                    history.record(HistoryDraft {
                        host_id: meta.host_id.clone(),
                        alias: meta.alias.clone(),
                        session_kind: meta.kind.clone(),
                        session_id: Some(session_id.to_string()),
                        command: line,
                        cwd: meta.cwd.clone(),
                        exit_code: None,
                        ok: None,
                        source: source.to_string(),
                    });
                }
                self.broadcast(serde_json::json!({"type": "catalog:changed", "kind": "history"}));
            }
        }
        self.write_raw(session_id, data)
    }

    pub fn begin_command(&self, session_id: &str, command: &str, source: &str) -> Result<JobView, String> {
        let command = command.trim();
        if command.is_empty() {
            return Err("命令是空的".into());
        }
        if command.chars().count() > 8000 {
            return Err("命令太长了（上限 8000 个字符）".into());
        }
        if command.contains('\0') || command.contains("__TS_END_") {
            return Err("命令里不能有空字符或结果标记".into());
        }
        let mind = self.mind(session_id)?;
        let shell = lock(&mind).shell;
        if lock(&mind).job.as_ref().map(|job| !job.done).unwrap_or(false) {
            return Err("这个终端还有一条命令在跑。先 poll_command，或 interrupt_terminal。".into());
        }
        let nonce = command_nonce();
        let job_id = Uuid::new_v4().to_string();
        let meta = self.tab_meta(session_id);
        let redacted = self
            .history_handle()
            .map(|history| history.redact_text(command))
            .unwrap_or_else(|| command.to_string());
        let history_id = self.history_handle().and_then(|history| {
            let id = history.record(HistoryDraft {
                host_id: meta.host_id.clone(),
                alias: meta.alias.clone(),
                session_kind: meta.kind.clone(),
                session_id: Some(session_id.to_string()),
                command: command.to_string(),
                cwd: meta.cwd.clone(),
                exit_code: None,
                ok: None,
                source: source.to_string(),
            });
            self.broadcast(serde_json::json!({"type": "catalog:changed", "kind": "history"}));
            id
        });
        {
            let mut mind = lock(&mind);
            if mind.job.as_ref().map(|job| !job.done).unwrap_or(false) {
                return Err("这个终端还有一条命令在跑。先 poll_command，或 interrupt_terminal。".into());
            }
            if let Some(view) = snapshot(session_id, &mind) {
                lock(&self.finished).insert(view.job_id.clone(), view);
            }
            let mark = mind.screen.mark();
            mind.job = Some(LiveJob {
                id: job_id.clone(),
                nonce: nonce.clone(),
                history_id,
                log_mark: mark,
                command: redacted,
                done: false,
                cancelled: false,
                exit_code: None,
                cwd: None,
            });
        }
        lock(&self.job_index).insert(job_id.clone(), session_id.to_string());
        let payload = format!("{command}\r{}\r", shell.trailer(&nonce));
        if let Err(err) = self.write_raw(session_id, &payload) {
            let mut mind = lock(&mind);
            if let Some(job) = mind.job.as_mut() {
                job.done = true;
                job.cancelled = true;
            }
            return Err(err);
        }
        self.poll_job(&job_id)
    }

    pub fn wait_job(&self, job_id: &str, timeout: Duration) -> Result<JobView, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let view = self.poll_job(job_id)?;
            if view.done {
                return Ok(view);
            }
            if Instant::now() >= deadline {
                let mut view = view;
                view.timed_out = true;
                return Ok(view);
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    pub fn poll_job(&self, job_id: &str) -> Result<JobView, String> {
        let session_id = lock(&self.job_index)
            .get(job_id)
            .cloned()
            .ok_or_else(|| "没有这条命令。它可能属于另一个进程。".to_string())?;
        let Some(mind) = lock(&self.screens).get(&session_id).cloned() else {
            return lock(&self.finished)
                .get(job_id)
                .cloned()
                .ok_or_else(|| "这条命令所在的终端已经关闭".to_string());
        };
        let finish = {
            let mut mind = lock(&mind);
            let finish = observe_job(&mut mind);
            if let Some(view) = snapshot(&session_id, &mind).filter(|view| view.job_id == job_id) {
                lock(&self.finished).insert(view.job_id.clone(), view);
            }
            finish
        };
        self.apply_finish(finish);
        lock(&self.finished)
            .get(job_id)
            .cloned()
            .ok_or_else(|| "没有这条命令".to_string())
    }

    pub fn cancel_job(&self, job_id: &str) -> Result<JobView, String> {
        let session_id = lock(&self.job_index)
            .get(job_id)
            .cloned()
            .ok_or_else(|| "没有这条命令".to_string())?;
        let _ = self.write_raw(&session_id, "\u{3}");
        if let Some(mind) = lock(&self.screens).get(&session_id).cloned() {
            let finish = {
                let mut mind = lock(&mind);
                let finish = if let Some(job) = mind.job.as_mut().filter(|job| job.id == job_id && !job.done) {
                    job.done = true;
                    job.cancelled = true;
                    job.history_id.clone().map(|id| HistoryFinish {
                        id,
                        exit_code: None,
                        cwd: mind.cwd.clone(),
                        ok: Some(false),
                    })
                } else {
                    None
                };
                if let Some(view) = snapshot(&session_id, &mind).filter(|view| view.job_id == job_id) {
                    lock(&self.finished).insert(view.job_id.clone(), view);
                }
                finish
            };
            self.apply_finish(finish);
        }
        self.poll_job(job_id)
    }

    pub fn interrupt(&self, session_id: &str) -> Result<(), String> {
        self.write_raw(session_id, "\u{3}")?;
        if let Some(mind) = lock(&self.screens).get(session_id).cloned() {
            let finish = {
                let mut mind = lock(&mind);
                let finish = if let Some(job) = mind.job.as_mut().filter(|job| !job.done) {
                    job.done = true;
                    job.cancelled = true;
                    job.history_id.clone().map(|id| HistoryFinish {
                        id,
                        exit_code: None,
                        cwd: mind.cwd.clone(),
                        ok: Some(false),
                    })
                } else {
                    None
                };
                if let Some(view) = snapshot(session_id, &mind) {
                    lock(&self.finished).insert(view.job_id.clone(), view);
                }
                finish
            };
            self.apply_finish(finish);
        }
        Ok(())
    }

    fn register_mind(
        &self,
        id: &str,
        shell: ShellKind,
        cwd: Option<String>,
        kind: &str,
        alias: &str,
        host_id: Option<String>,
    ) {
        lock(&self.screens).insert(
            id.to_string(),
            Arc::new(Mutex::new(TabMind {
                shell,
                cwd,
                last_exit: None,
                screen: Screen::new(),
                assembler: LineAssembler::new(),
                job: None,
                alias: alias.to_string(),
                kind: kind.to_string(),
                host_id,
            })),
        );
    }

    fn mind(&self, session_id: &str) -> Result<Arc<Mutex<TabMind>>, String> {
        lock(&self.screens)
            .get(session_id)
            .cloned()
            .ok_or_else(|| "找不到这个会话".to_string())
    }

    fn write_raw(&self, session_id: &str, data: &str) -> Result<(), String> {
        let io = {
            let tabs = lock(&self.tabs);
            tabs.get(session_id)
                .map(|tab| tab.io.clone())
                .ok_or_else(|| "找不到这个会话".to_string())?
        };
        let ok = match io {
            TabIo::Local(session) => lock(&session).write(data.as_bytes()).is_ok(),
            TabIo::Remote(remote) => remote.pty.write(data.as_bytes()),
        };
        if ok {
            Ok(())
        } else {
            Err("终端已经关闭".into())
        }
    }

    fn tab_meta(&self, session_id: &str) -> TabMeta {
        let tabs = lock(&self.tabs);
        let screens = lock(&self.screens);
        let cwd = screens.get(session_id).and_then(|mind| lock(mind).cwd.clone());
        if let Some(tab) = tabs.get(session_id) {
            TabMeta {
                alias: Some(tab.info.alias.clone()),
                host_id: tab.host_id.clone(),
                kind: tab.info.kind.clone(),
                cwd,
            }
        } else {
            TabMeta {
                alias: None,
                host_id: None,
                kind: "local".into(),
                cwd,
            }
        }
    }

    fn history_handle(&self) -> Option<Arc<CommandHistory>> {
        lock(&self.history).clone()
    }

    fn apply_finish(&self, finish: Option<HistoryFinish>) {
        let Some(finish) = finish else { return };
        if let Some(history) = self.history_handle() {
            history.finish(&finish.id, finish.exit_code, finish.cwd, finish.ok);
        }
    }

    /// SFTP listings and transfers for this tab. `PtySsh` has no `call`, so this cannot enter the shell actor.
    pub fn with_ssh<T: Send + 'static>(
        &self,
        session_id: &str,
        op: impl FnOnce(&Session) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        self.remote(session_id)?.side.call(op)?
    }

    pub fn sample_metrics(&self, session_id: &str) -> Result<crate::monitor::ServerMetrics, String> {
        let remote = self.remote(session_id)?;
        let side = remote.side.clone();
        let mut previous = lock(&remote.probes);
        poll_probe(&mut previous, Instant::now(), || exec_probe(&side))
    }

    fn remote(&self, session_id: &str) -> Result<RemoteTab, String> {
        let tabs = lock(&self.tabs);
        let tab = tabs.get(session_id).ok_or_else(|| "找不到这个会话".to_string())?;
        match &tab.io {
            TabIo::Remote(remote) => Ok(remote.clone()),
            TabIo::Local(_) => Err("会话未连接到 SSH 主机".into()),
        }
    }
}

struct TabMeta {
    alias: Option<String>,
    host_id: Option<String>,
    kind: String,
    cwd: Option<String>,
}

fn clamp_dim(value: u32, fallback: u32) -> u32 {
    if value == 0 { fallback } else { value.min(512) }
}

fn command_nonce() -> String {
    Uuid::new_v4().simple().to_string().chars().take(8).collect()
}

fn observe_job(mind: &mut TabMind) -> Option<HistoryFinish> {
    let (log_mark, nonce, already_done) = {
        let job = mind.job.as_ref()?;
        (job.log_mark, job.nonce.clone(), job.done)
    };
    if already_done {
        return None;
    }
    let (raw, _) = mind.screen.since(log_mark);
    let (code, cwd) = parse_end_marker(&raw, &nonce)?;
    let history_id = {
        let job = mind.job.as_mut()?;
        job.done = true;
        job.exit_code = Some(code);
        job.cwd = Some(cwd.clone());
        job.history_id.clone()
    };
    mind.cwd = Some(cwd.clone());
    mind.last_exit = Some(code);
    history_id.map(|id| HistoryFinish {
        id,
        exit_code: Some(code),
        cwd: Some(cwd),
        ok: Some(code == 0),
    })
}

fn snapshot(session_id: &str, mind: &TabMind) -> Option<JobView> {
    let job = mind.job.as_ref()?;
    let (raw, log_cut) = mind.screen.since(job.log_mark);
    let marker = parse_end_marker(&raw, &job.nonce);
    let (stdout, cut) = truncate_output(transcript_before_marker(&raw, &job.nonce), 200_000);
    Some(JobView {
        job_id: job.id.clone(),
        session_id: session_id.to_string(),
        command: job.command.clone(),
        done: job.done || marker.is_some(),
        cancelled: job.cancelled,
        timed_out: false,
        exit_code: job.exit_code.or_else(|| marker.as_ref().map(|(code, _)| *code)),
        cwd: job.cwd.clone().or_else(|| marker.map(|(_, cwd)| cwd)).or(mind.cwd.clone()),
        stdout,
        truncated: log_cut || cut,
        streams: "merged".into(),
    })
}

fn apply_terminal_output(
    raw: &str,
    screens: &Mutex<HashMap<String, Arc<Mutex<TabMind>>>>,
    history: &Mutex<Option<Arc<CommandHistory>>>,
    finished: &Mutex<HashMap<String, JobView>>,
    events: &broadcast::Sender<String>,
) {
    let Ok(msg) = serde_json::from_str::<Value>(raw) else { return };
    if msg.get("type").and_then(|value| value.as_str()) != Some("term:data") {
        return;
    }
    let Some(session_id) = msg.get("sessionId").and_then(|value| value.as_str()) else { return };
    let Some(data) = msg.get("data").and_then(|value| value.as_str()) else { return };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else { return };
    let Some(mind) = lock(screens).get(session_id).cloned() else { return };
    let (finish, view) = {
        let mut mind = lock(&mind);
        mind.screen.ingest(&bytes);
        let finish = observe_job(&mut mind);
        let view = snapshot(session_id, &mind).filter(|view| view.done);
        (finish, view)
    };
    if let Some(view) = view {
        lock(finished).insert(view.job_id.clone(), view);
    }
    if let Some(finish) = finish {
        if let Some(history) = lock(history).clone() {
            history.finish(&finish.id, finish.exit_code, finish.cwd, finish.ok);
            let _ = events.send(serde_json::json!({"type": "catalog:changed", "kind": "history"}).to_string());
        }
    }
}

fn remember_fingerprints(hosts: &Arc<Mutex<HostStore>>, learned: &[(String, String)]) {
    let mut guard = lock(hosts);
    for (id, fingerprint) in learned {
        if !fingerprint.is_empty() {
            let _ = guard.remember_fingerprint(id, fingerprint);
        }
    }
}

fn open_shell(session: &Session, cols: u32, rows: u32) -> Result<ssh2::Channel, String> {
    session.set_blocking(true);
    let mut channel = session.channel_session().map_err(|err| err.to_string())?;
    channel
        .request_pty("xterm-256color", None, Some((cols, rows, 0, 0)))
        .map_err(|err| format!("无法打开远程终端: {err}"))?;
    channel.shell().map_err(|err| format!("无法启动远程 Shell: {err}"))?;
    Ok(channel)
}

/// One probe on the side session. A stuck `df` or `ps` gives up after 5s so the
/// next SFTP job can run. The shell actor never sees it. The timeout is cleared
/// before the call returns, so a later transfer is not cut off at 5 seconds.
fn exec_probe(ssh: &SideSsh) -> Result<String, String> {
    ssh.call(|session| {
        let _reset = ResetTimeout::arm(session, 5_000);
        exec_command(session, PROBE_COMMAND).map(|item| item.stdout).map_err(|err| err.to_string())
    })?
}

struct ResetTimeout<'a>(&'a Session);

impl<'a> ResetTimeout<'a> {
    fn arm(session: &'a Session, timeout_ms: u32) -> Self {
        session.set_timeout(timeout_ms);
        Self(session)
    }
}

impl Drop for ResetTimeout<'_> {
    fn drop(&mut self) {
        self.0.set_timeout(0);
    }
}

fn metrics_loop(
    ssh: SideSsh,
    probes: Arc<Mutex<Option<ProbeHistory>>>,
    events: broadcast::Sender<String>,
    stop: Arc<AtomicBool>,
    interval: Arc<AtomicU64>,
    session_id: String,
) {
    // These 200ms steps only notice tab close. They are not a second probe
    // and they do not run on the PTY thread.
    while !stop.load(Ordering::Relaxed) {
        let sample = {
            let mut previous = lock(&probes);
            poll_probe(&mut previous, Instant::now(), || exec_probe(&ssh))
        };
        if let Ok(metrics) = sample {
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
