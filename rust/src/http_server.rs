use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path as IdPath, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bytes::Bytes;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;
use crate::auth::{authorize, host_is_local, origin_ok};
use crate::bindings::BindingStore;
use crate::download_grant::{DownloadGrants, DownloadKind};
use crate::mcp_config::{cursor_user_mcp_path, write_cursor_mcp_file};
use crate::history::CommandHistory;
use crate::hub::SessionHub;
use crate::mcp::{tool_names, McpHandler, WorkbenchMcp};
use crate::mcp_http::{self, TokenQuery};
use crate::paths::resolve_data_dir;
use crate::runtime::{self, remove_runtime, write_runtime};
use crate::sftp::{self, chmod, list_dir, mkdir, read_text_limited, remove_path, rename, write_bytes};
use crate::ssh::resolve_targets;
use crate::state::{lock, AppState, SseHub};
use crate::store::{HostStore, SettingsStore, SnippetStore, TunnelStore};
use crate::tunnel::TunnelManager;
use crate::util::{hex_encode, restrict_user_dir, restrict_user_file};
use crate::vault::load_or_create_master_key;

pub struct ServerOptions {
    pub data_dir: Option<PathBuf>,
    pub port: Option<u16>,
    pub open_browser: bool,
}

pub struct RunningServer {
    pub port: u16,
    pub token: String,
    pub url: String,
    pub mcp_url: String,
    pub sse_url: String,
    pub data_dir: PathBuf,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn start_workbench(options: ServerOptions) -> Result<RunningServer, String> {
    let data_dir = options.data_dir.unwrap_or_else(resolve_data_dir);
    std::fs::create_dir_all(&data_dir).map_err(|err| err.to_string())?;
    restrict_user_dir(&data_dir);
    let key = load_or_create_master_key(&data_dir.join("vault.key")).map_err(|err| err.to_string())?;
    let hosts = Arc::new(std::sync::Mutex::new(
        HostStore::load(data_dir.join("hosts.json"), key).map_err(|err| err.to_string())?,
    ));
    let snippets_path = data_dir.join("snippets.json");
    let snippets_missing = !snippets_path.exists();
    let mut snippets = SnippetStore::load(&snippets_path).map_err(|err| err.to_string())?;
    if snippets_missing {
        for snippet in default_snippets() {
            snippets.save_draft(&snippet).map_err(|err| err.to_string())?;
        }
    }
    let settings = SettingsStore::load(data_dir.join("settings.json")).map_err(|err| err.to_string())?;
    let interval = settings.get().monitor_interval_ms;
    let token = load_token(&data_dir);
    let history = Arc::new(CommandHistory::open(data_dir.join("command-history.json")).map_err(|err| err.to_string())?);
    history.set_token(&token);
    let hub = SessionHub::new(interval);
    hub.set_history(history.clone());
    let snippets = Arc::new(std::sync::Mutex::new(snippets));
    let tunnel_rules = Arc::new(std::sync::Mutex::new(
        TunnelStore::load(data_dir.join("tunnels.json")).map_err(|err| err.to_string())?,
    ));
    let settings = Arc::new(std::sync::Mutex::new(settings));
    let tunnels = Arc::new(TunnelManager::new());
    let bindings = Arc::new(BindingStore::open(data_dir.join("bindings.json")).map_err(|err| err.to_string())?);
    let state = AppState {
        token: token.clone(),
        data_dir: data_dir.clone(),
        port: Arc::new(std::sync::atomic::AtomicU16::new(0)),
        hosts: hosts.clone(),
        snippets: snippets.clone(),
        tunnel_rules: tunnel_rules.clone(),
        settings: settings.clone(),
        hub: hub.clone(),
        history: history.clone(),
        tunnels: tunnels.clone(),
        mcp: Arc::new(McpHandler::for_workbench(WorkbenchMcp {
            hosts,
            snippets,
            tunnel_rules,
            tunnels,
            settings,
            bindings,
            hub,
            history,
            token: token.clone(),
        })),
        transfers: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        download_grants: Arc::new(std::sync::Mutex::new(DownloadGrants::new())),
        sse: Arc::new(SseHub::default()),
        mcp_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        stopping: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    spawn_catalog_watch(state.clone());
    let listen_port = chosen_port(options.port);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let thread_state = state.clone();
    let thread = std::thread::Builder::new()
        .name("tianshell-http".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = ready_tx.send(Err(err.to_string()));
                    return;
                }
            };
            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::bind(("127.0.0.1", listen_port)).await {
                    Ok(listener) => listener,
                    Err(err) => {
                        let _ = ready_tx.send(Err(format!(
                            "无法在 127.0.0.1:{listen_port} 启动工作台: {err}。如果已经有一个 TianShell 在运行，打开数据目录里的 runtime.json。"
                        )));
                        return;
                    }
                };
                let actual = match listener.local_addr() {
                    Ok(addr) => addr.port(),
                    Err(err) => {
                        let _ = ready_tx.send(Err(err.to_string()));
                        return;
                    }
                };
                thread_state.port.store(actual, Ordering::Relaxed);
                let info = runtime::describe(actual, &thread_state.token);
                if let Err(err) = write_runtime(&thread_state.data_dir, &info) {
                    let _ = ready_tx.send(Err(format!("无法写入 runtime.json: {err}")));
                    return;
                }
                let app = router(thread_state.clone());
                let _ = ready_tx.send(Ok(actual));
                let data_dir = thread_state.data_dir.clone();
                let hub = thread_state.hub.clone();
                let tunnels = thread_state.tunnels.clone();
                let stopping = thread_state.stopping.clone();
                let _ = axum::serve(listener, app)
                    .with_graceful_shutdown(async move {
                        let _ = shutdown_rx.await;
                    })
                    .await;
                stopping.store(true, Ordering::Relaxed);
                hub.close_all();
                tunnels.stop_all();
                remove_runtime(&data_dir, std::process::id());
            });
        })
        .map_err(|err| err.to_string())?;
    let actual = match ready_rx.recv() {
        Ok(Ok(port)) => port,
        Ok(Err(err)) => {
            let _ = thread.join();
            return Err(err);
        }
        Err(_) => {
            let _ = thread.join();
            return Err("工作台线程已退出".into());
        }
    };
    let url = format!("http://127.0.0.1:{actual}/?token={token}");
    if options.open_browser {
        launch_browser(&url);
    }
    Ok(RunningServer {
        port: actual,
        token,
        url,
        mcp_url: format!("http://127.0.0.1:{actual}/mcp"),
        sse_url: format!("http://127.0.0.1:{actual}/sse"),
        data_dir,
        shutdown: Some(shutdown_tx),
        thread: Some(thread),
    })
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/renderer.js", get(script))
        .route("/renderer.css", get(style))
        .route("/ws", get(ws_upgrade))
        .route("/mcp", post(mcp_http::post_mcp).get(mcp_http::get_mcp).delete(mcp_http::delete_mcp).options(mcp_http::options_mcp))
        .route("/sse", get(mcp_http::get_sse))
        .route("/message", post(mcp_http::post_message))
        .route("/api/hosts", get(list_hosts).post(save_host))
        .route("/api/hosts/:id", delete(delete_host))
        .route("/api/groups", post(save_group))
        .route("/api/groups/:id", delete(delete_group))
        .route("/api/history", get(list_history))
        .route("/api/snippets", get(list_snippets).post(save_snippet))
        .route("/api/snippets/:id", delete(delete_snippet))
        .route("/api/tunnels/start", post(start_tunnel))
        .route("/api/tunnels/stop", post(stop_tunnel))
        .route("/api/tunnels", get(list_tunnels).post(save_tunnel))
        .route("/api/tunnels/:id", delete(delete_tunnel))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/sessions", post(open_session))
        .route("/api/sessions/:id", delete(close_session))
        .route("/api/sessions/:id/metrics", get(session_metrics))
        .route("/api/sessions/:id/sftp/list", get(sftp_list))
        .route("/api/sessions/:id/sftp/text", get(sftp_read).put(sftp_write))
        .route("/api/sessions/:id/sftp/mkdir", post(sftp_mkdir))
        .route("/api/sessions/:id/sftp/rename", post(sftp_rename))
        .route("/api/sessions/:id/sftp/chmod", post(sftp_chmod))
        .route("/api/sessions/:id/sftp/delete", post(sftp_delete))
        .route("/api/sessions/:id/sftp/download", get(sftp_download))
        .route("/api/sessions/:id/sftp/archive-download", get(sftp_archive_download))
        .route("/api/sessions/:id/sftp/download-grant", post(issue_download_grant))
        .route("/api/downloads/:id", get(redeem_download))
        .route("/api/mcp/cursor", post(write_cursor_mcp))
        .route("/api/sessions/:id/sftp/upload", post(sftp_upload))
        .route("/api/transfers/:id/cancel", post(cancel_transfer))
        .route("/api/mcp", get(mcp_info))
        .fallback(fallback)
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}

struct Authed;

#[axum::async_trait]
impl FromRequestParts<AppState> for Authed {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        authorize(&parts.headers, parts.uri.query(), state.token.as_str()).map(|_| Authed).map_err(|err| {
            json_err(
                StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::UNAUTHORIZED),
                err.message(),
            )
        })
    }
}

async fn health() -> Response {
    Json(json!({"ok": true, "service": "tianshell", "mcp": true})).into_response()
}

async fn index(State(state): State<AppState>) -> Response {
    (
        [("content-type", "text/html; charset=utf-8"), ("cache-control", "no-store")],
        render_index(&state.token),
    )
        .into_response()
}

async fn script() -> Response {
    ([(axum::http::header::CONTENT_TYPE, "application/javascript; charset=utf-8")], read_ui("renderer.js")).into_response()
}

async fn style() -> Response {
    ([("content-type", "text/css; charset=utf-8")], read_ui("renderer.css")).into_response()
}

async fn fallback(State(state): State<AppState>, request: axum::extract::Request) -> Response {
    if request.uri().path().starts_with("/api/") {
        if authorize(request.headers(), request.uri().query(), &state.token).is_err() {
            return json_err(StatusCode::UNAUTHORIZED, "未授权的本地访问令牌");
        }
        return json_err(StatusCode::NOT_FOUND, "未知接口路径");
    }
    json_err(StatusCode::NOT_FOUND, "Not found")
}

async fn ws_upgrade(State(state): State<AppState>, headers: HeaderMap, Query(query): Query<TokenQuery>, ws: axum::extract::ws::WebSocketUpgrade) -> Response {
    let synthetic = query.token.as_ref().map(|token| format!("token={token}"));
    if authorize(&headers, synthetic.as_deref(), &state.token).is_err() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |socket| async move { ws_loop(socket, state).await })
}

async fn ws_loop(socket: axum::extract::ws::WebSocket, state: AppState) {
    use axum::extract::ws::Message;
    use futures_util::{SinkExt, StreamExt};
    let mut events = state.hub.subscribe();
    let (mut writer, mut reader) = socket.split();
    for info in state.hub.list_sessions() {
        let text = json!({"type": "session:opened", "session": info}).to_string();
        if writer.send(Message::Text(text)).await.is_err() {
            return;
        }
    }
    let outbound = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(text) => {
                    if writer.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    while let Some(Ok(message)) = reader.next().await {
        match message {
            Message::Text(text) => state.hub.dispatch(&text),
            Message::Close(_) => break,
            _ => {}
        }
    }
    outbound.abort();
}

async fn list_hosts(_auth: Authed, State(state): State<AppState>) -> Response {
    let hosts = lock(&state.hosts);
    json_ok(json!({
        "hosts": hosts.list_public(),
        "groups": hosts.list_groups_sorted(),
        "dataDir": state.data_dir
    }))
}

async fn save_host(_auth: Authed, State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    match lock(&state.hosts).save_draft(&body) {
        Ok(host) => {
            notify_catalog(&state, "hosts");
            json_ok(json!(host))
        }
        Err(err) => json_err(StatusCode::BAD_REQUEST, err.to_string()),
    }
}

async fn delete_host(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>) -> Response {
    match lock(&state.hosts).delete(&id) {
        Ok(true) => {
            notify_catalog(&state, "hosts");
            json_ok(json!(true))
        }
        Ok(false) => json_err(StatusCode::BAD_REQUEST, "找不到这台主机"),
        Err(err) => json_err(StatusCode::BAD_REQUEST, err.to_string()),
    }
}

async fn save_group(_auth: Authed, State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    match lock(&state.hosts).save_group_draft(&body) {
        Ok(group) => {
            notify_catalog(&state, "hosts");
            json_ok(json!(group))
        }
        Err(err) => json_err(StatusCode::BAD_REQUEST, err.to_string()),
    }
}

async fn delete_group(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>) -> Response {
    match lock(&state.hosts).delete_group(&id) {
        Ok(()) => {
            notify_catalog(&state, "hosts");
            json_ok(json!(true))
        }
        Err(err) => json_err(StatusCode::BAD_REQUEST, err.to_string()),
    }
}

#[derive(Debug, Deserialize, Default)]
struct HistoryQuery {
    q: Option<String>,
    host: Option<String>,
    limit: Option<usize>,
}

async fn list_history(_auth: Authed, State(state): State<AppState>, Query(query): Query<HistoryQuery>) -> Response {
    let rows = state.history.search(query.q.as_deref(), query.host.as_deref(), query.limit.unwrap_or(80));
    json_ok(json!(rows))
}

async fn list_snippets(_auth: Authed, State(state): State<AppState>) -> Response {
    json_ok(json!(lock(&state.snippets).list()))
}

async fn save_snippet(_auth: Authed, State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    match lock(&state.snippets).save_draft(&body) {
        Ok(snippet) => {
            notify_catalog(&state, "snippets");
            json_ok(json!(snippet))
        }
        Err(err) => json_err(StatusCode::BAD_REQUEST, err.to_string()),
    }
}

async fn delete_snippet(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>) -> Response {
    match lock(&state.snippets).delete(&id) {
        Ok(()) => {
            notify_catalog(&state, "snippets");
            json_ok(json!(true))
        }
        Err(err) => json_err(StatusCode::BAD_REQUEST, err.to_string()),
    }
}

async fn list_tunnels(_auth: Authed, State(state): State<AppState>) -> Response {
    let rules = lock(&state.tunnel_rules).list().to_vec();
    json_ok(json!(state.tunnels.runtimes(&rules)))
}

async fn save_tunnel(_auth: Authed, State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    match lock(&state.tunnel_rules).save_draft(&body) {
        Ok(rule) => {
            notify_catalog(&state, "tunnels");
            json_ok(json!(rule))
        }
        Err(err) => json_err(StatusCode::BAD_REQUEST, err.to_string()),
    }
}

async fn delete_tunnel(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>) -> Response {
    state.tunnels.stop(&id);
    match lock(&state.tunnel_rules).delete(&id) {
        Ok(()) => {
            notify_catalog(&state, "tunnels");
            json_ok(json!(true))
        }
        Err(err) => json_err(StatusCode::BAD_REQUEST, err.to_string()),
    }
}

async fn start_tunnel(_auth: Authed, State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let Some(id) = body.get("id").and_then(|v| v.as_str()).map(str::to_string) else {
        return json_err(StatusCode::BAD_REQUEST, "找不到隧道规则");
    };
    let rule = {
        let rules = lock(&state.tunnel_rules);
        rules.list().iter().find(|rule| rule.id == id).cloned()
    };
    let Some(rule) = rule else {
        return json_err(StatusCode::BAD_REQUEST, "找不到隧道规则");
    };
    let hosts = state.hosts.clone();
    let tunnels = state.tunnels.clone();
    match block(move || {
        let chain = {
            let guard = lock(&hosts);
            resolve_targets(&guard, &rule.host_id)?
        };
        tunnels.start(&rule, &chain)
    })
    .await
    {
        Ok(port) => {
            notify_catalog(&state, "tunnels");
            json_ok(json!({"port": port}))
        }
        Err(err) => json_err(StatusCode::BAD_REQUEST, err),
    }
}

async fn stop_tunnel(_auth: Authed, State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    if let Some(id) = body.get("id").and_then(|v| v.as_str()) {
        state.tunnels.stop(id);
        notify_catalog(&state, "tunnels");
    }
    json_ok(json!(true))
}

async fn get_settings(_auth: Authed, State(state): State<AppState>) -> Response {
    json_ok(json!(lock(&state.settings).get()))
}

async fn put_settings(_auth: Authed, State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    match lock(&state.settings).apply_patch(&body) {
        Ok(settings) => {
            state.hub.set_interval(settings.monitor_interval_ms);
            notify_catalog(&state, "settings");
            json_ok(json!(settings))
        }
        Err(err) => json_err(StatusCode::BAD_REQUEST, err.to_string()),
    }
}

async fn open_session(_auth: Authed, State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let kind = body.get("kind").and_then(|v| v.as_str()).unwrap_or("ssh");
    if kind == "local" {
        let cols = body.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u32;
        let rows = body.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u32;
        return match state.hub.open_local(cols, rows) {
            Ok(info) => json_ok(json!(info)),
            Err(err) => json_err(StatusCode::BAD_REQUEST, err),
        };
    }
    let host_id = body.get("hostId").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let cols = body.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u32;
    let rows = body.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u32;
    let hub = state.hub.clone();
    let hosts = state.hosts.clone();
    match block(move || hub.open_ssh(&hosts, &host_id, cols, rows)).await {
        Ok(info) => json_ok(json!(info)),
        Err(err) => json_err(StatusCode::BAD_REQUEST, err),
    }
}

async fn close_session(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>) -> Response {
    state.hub.close_session(&id);
    json_ok(json!(true))
}

async fn session_metrics(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>) -> Response {
    let hub = state.hub.clone();
    match block(move || hub.sample_metrics(&id)).await {
        Ok(metrics) => json_ok(json!(metrics)),
        Err(err) => json_err(StatusCode::BAD_REQUEST, err),
    }
}

async fn sftp_list(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>, Query(query): Query<PathQuery>) -> Response {
    let path = query.path.unwrap_or_else(|| "/".into());
    match block(move || with_ssh(&state, &id, move |session| list_dir(session, &path).map_err(|err| err.to_string()))).await {
        Ok(entries) => json_ok(json!(entries)),
        Err(err) => json_err(StatusCode::BAD_REQUEST, err),
    }
}

async fn sftp_read(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>, Query(query): Query<PathQuery>) -> Response {
    let path = query.path.unwrap_or_default();
    match block(move || {
        with_ssh(&state, &id, move |session| {
            let text = read_text_limited(session, &path, 256 * 1024).map_err(|err| err.to_string())?;
            Ok(json!({"content": text.content, "truncated": text.truncated}))
        })
    })
    .await
    {
        Ok(payload) => json_ok(payload),
        Err(err) => json_err(StatusCode::BAD_REQUEST, err),
    }
}

async fn sftp_write(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>, Json(body): Json<Value>) -> Response {
    let path = body.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
    match block(move || {
        with_ssh(&state, &id, move |session| {
            sftp::write_text(session, &path, &content).map_err(|err| err.to_string())
        })
    })
    .await
    {
        Ok(()) => json_ok(json!(true)),
        Err(err) => json_err(StatusCode::BAD_REQUEST, err),
    }
}

async fn sftp_mkdir(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>, Json(body): Json<Value>) -> Response {
    let path = body.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
    simple_sftp(state, id, move |session| mkdir(session, &path).map_err(|err| err.to_string())).await
}

async fn sftp_rename(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>, Json(body): Json<Value>) -> Response {
    let old_path = body.get("oldPath").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let new_path = body.get("newPath").and_then(|v| v.as_str()).unwrap_or("").to_string();
    simple_sftp(state, id, move |session| rename(session, &old_path, &new_path).map_err(|err| err.to_string())).await
}

async fn sftp_chmod(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>, Json(body): Json<Value>) -> Response {
    let path = body.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mode = body.get("mode").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    simple_sftp(state, id, move |session| chmod(session, &path, mode).map_err(|err| err.to_string())).await
}

async fn sftp_delete(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>, Json(body): Json<Value>) -> Response {
    let path = body.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
    simple_sftp(state, id, move |session| remove_path(session, &path).map_err(|err| err.to_string())).await
}

async fn simple_sftp(state: AppState, id: String, op: impl FnOnce(&ssh2::Session) -> Result<(), String> + Send + 'static) -> Response {
    match block(move || with_ssh(&state, &id, op)).await {
        Ok(()) => json_ok(json!(true)),
        Err(err) => json_err(StatusCode::BAD_REQUEST, err),
    }
}

async fn sftp_download(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>, Query(query): Query<PathQuery>) -> Response {
    if let Some(response) = reject_download_token(&query) {
        return with_private_download(response);
    }
    with_private_download(stream_file_download(state, id, query.path.unwrap_or_default()).await)
}

async fn stream_file_download(state: AppState, id: String, remote: String) -> Response {
    if remote.is_empty() {
        return json_err(StatusCode::BAD_REQUEST, "缺少 path 参数");
    }
    let filename = base_name(&remote);
    let tid = Uuid::new_v4().to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    lock(&state.transfers).insert(tid.clone(), cancel.clone());
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let ready_slot = Arc::new(std::sync::Mutex::new(Some(ready_tx)));
    let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    let hub = state.hub.clone();
    let session_id = id.clone();
    let remote_path = remote.clone();
    let fname = filename.clone();
    let tid_progress = tid.clone();
    let slot = ready_slot.clone();
    let state_for_task = state.clone();
    tokio::task::spawn_blocking(move || {
        let hub_job = hub.clone();
        let slot_job = slot.clone();
        let session_job = session_id.clone();
        let remote_job = remote_path.clone();
        let fname_job = fname.clone();
        let tid_job = tid_progress.clone();
        let outcome = with_ssh(&state_for_task, &session_id, move |session| {
            let sftp = session.sftp().map_err(|err| err.to_string())?;
            let mut file = sftp.open(Path::new(&remote_job)).map_err(|err| err.to_string())?;
            if let Some(sender) = lock(&slot_job).take() {
                let _ = sender.send(Ok(()));
            }
            let mut buf = [0u8; 32_768];
            let mut sent = 0u64;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    hub_job.broadcast(json!({"type": "transfer:progress", "task": transfer_task(&tid_job, &session_job, "download", &fname_job, &remote_job, sent, sent, "cancelled", None)}));
                    return Ok(());
                }
                let n = file.read(&mut buf).map_err(|err| err.to_string())?;
                if n == 0 {
                    break;
                }
                sent += n as u64;
                if chunk_tx.blocking_send(Ok(Bytes::copy_from_slice(&buf[..n]))).is_err() {
                    return Ok(());
                }
                hub_job.broadcast(json!({"type": "transfer:progress", "task": transfer_task(&tid_job, &session_job, "download", &fname_job, &remote_job, sent, 0, "running", None)}));
            }
            hub_job.broadcast(json!({"type": "transfer:progress", "task": transfer_task(&tid_job, &session_job, "download", &fname_job, &remote_job, sent, sent.max(1), "done", None)}));
            Ok(())
        });
        if let Err(err) = outcome {
            if let Some(sender) = lock(&slot).take() {
                let _ = sender.send(Err(err.clone()));
            }
            hub.broadcast(json!({"type": "transfer:progress", "task": transfer_task(&tid_progress, &session_id, "download", &fname, &remote_path, 0, 0, "error", Some(err))}));
        }
        lock(&state_for_task.transfers).remove(&tid_progress);
    });
    match ready_rx.await {
        Ok(Ok(())) => {
            let body = Body::from_stream(ReceiverStream::new(chunk_rx));
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/octet-stream")
                .header("content-disposition", format!("attachment; filename*=UTF-8''{}", encode_filename(&filename)))
                .body(body)
                .unwrap_or_else(|_| json_err(StatusCode::INTERNAL_SERVER_ERROR, "无法开始下载"))
        }
        Ok(Err(err)) => json_err(StatusCode::BAD_REQUEST, err),
        Err(_) => json_err(StatusCode::BAD_REQUEST, "下载没有开始"),
    }
}

async fn sftp_archive_download(
    _auth: Authed,
    State(state): State<AppState>,
    IdPath(id): IdPath<String>,
    Query(query): Query<PathQuery>,
) -> Response {
    if let Some(response) = reject_download_token(&query) {
        return with_private_download(response);
    }
    with_private_download(stream_archive_download(state, id, query.path.unwrap_or_default()).await)
}

async fn stream_archive_download(state: AppState, id: String, remote: String) -> Response {
    if remote.is_empty() {
        return json_err(StatusCode::BAD_REQUEST, "缺少 path 参数");
    }
    if remote.chars().any(|c| matches!(c, ';' | '|' | '&' | '$' | '>' | '<' | '`' | '\n' | '\r')) {
        return json_err(StatusCode::BAD_REQUEST, "路径包含非法字符");
    }

    let norm = if remote == "/" { "/" } else { remote.trim_end_matches('/') };
    let (parent, dirname) = match norm.rfind('/') {
        Some(0) => ("/".to_string(), norm[1..].to_string()),
        Some(idx) => (norm[..idx].to_string(), norm[idx + 1..].to_string()),
        None => (".".to_string(), norm.to_string()),
    };
    let dirname = if dirname.is_empty() { "root".to_string() } else { dirname };
    let filename = format!("{dirname}.tar.gz");

    let tid = Uuid::new_v4().to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    lock(&state.transfers).insert(tid.clone(), cancel.clone());
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let ready_slot = Arc::new(std::sync::Mutex::new(Some(ready_tx)));
    let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    let hub = state.hub.clone();
    let session_id = id.clone();
    let remote_path = remote.to_string();
    let fname = filename.clone();
    let tid_progress = tid.clone();
    let slot = ready_slot.clone();
    let state_for_task = state.clone();

    tokio::task::spawn_blocking(move || {
        let hub_job = hub.clone();
        let slot_job = slot.clone();
        let session_job = session_id.clone();
        let remote_job = remote_path.clone();
        let fname_job = fname.clone();
        let tid_job = tid_progress.clone();
        let outcome = with_ssh(&state_for_task, &session_id, move |session| {
            session.set_blocking(true);
            let mut channel = session.channel_session().map_err(|err| err.to_string())?;
            let escaped_parent = parent.replace('\'', "'\\''");
            let escaped_dirname = dirname.replace('\'', "'\\''");
            let cmd = format!("tar -czf - -C '{escaped_parent}' '{escaped_dirname}'");
            channel.exec(&cmd).map_err(|err| format!("执行远端打包失败: {err}"))?;

            if let Some(sender) = lock(&slot_job).take() {
                let _ = sender.send(Ok(()));
            }
            let mut buf = [0u8; 32_768];
            let mut sent = 0u64;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    hub_job.broadcast(json!({
                        "type": "transfer:progress",
                        "task": transfer_task(&tid_job, &session_job, "download", &fname_job, &remote_job, sent, sent, "cancelled", None)
                    }));
                    return Ok(());
                }
                let n = channel.read(&mut buf).map_err(|err| err.to_string())?;
                if n == 0 {
                    break;
                }
                sent += n as u64;
                if chunk_tx.blocking_send(Ok(Bytes::copy_from_slice(&buf[..n]))).is_err() {
                    return Ok(());
                }
                hub_job.broadcast(json!({
                    "type": "transfer:progress",
                    "task": transfer_task(&tid_job, &session_job, "download", &fname_job, &remote_job, sent, 0, "running", None)
                }));
            }
            channel.wait_close().ok();
            hub_job.broadcast(json!({
                "type": "transfer:progress",
                "task": transfer_task(&tid_job, &session_job, "download", &fname_job, &remote_job, sent, sent.max(1), "done", None)
            }));
            Ok(())
        });
        if let Err(err) = outcome {
            if let Some(sender) = lock(&slot).take() {
                let _ = sender.send(Err(err.clone()));
            }
            hub.broadcast(json!({
                "type": "transfer:progress",
                "task": transfer_task(&tid_progress, &session_id, "download", &fname, &remote_path, 0, 0, "error", Some(err))
            }));
        }
        lock(&state_for_task.transfers).remove(&tid_progress);
    });

    match ready_rx.await {
        Ok(Ok(())) => {
            let body = Body::from_stream(ReceiverStream::new(chunk_rx));
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/gzip")
                .header(
                    "content-disposition",
                    format!("attachment; filename*=UTF-8''{}", encode_filename(&filename)),
                )
                .body(body)
                .unwrap_or_else(|_| json_err(StatusCode::INTERNAL_SERVER_ERROR, "无法开始归档下载"))
        }
        Ok(Err(err)) => json_err(StatusCode::BAD_REQUEST, err),
        Err(_) => json_err(StatusCode::BAD_REQUEST, "归档下载没有开始"),
    }
}

async fn sftp_upload(
    _auth: Authed,
    State(state): State<AppState>,
    IdPath(id): IdPath<String>,
    Query(query): Query<PathQuery>,
    body: Bytes,
) -> Response {
    let remote = query.path.unwrap_or_default();
    if remote.is_empty() {
        return json_err(StatusCode::BAD_REQUEST, "缺少 path 参数");
    }
    if body.len() > 64 * 1024 * 1024 {
        return json_err(StatusCode::BAD_REQUEST, "上传内容超过 64MB");
    }
    let filename = base_name(&remote);
    let tid = Uuid::new_v4().to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    lock(&state.transfers).insert(tid.clone(), cancel.clone());
    let hub = state.hub.clone();
    let bytes = body.to_vec();
    let session_id = id.clone();
    let remote_path = remote.clone();
    let fname = filename.clone();
    let tid_progress = tid.clone();
    let state_for_task = state.clone();
    let result = block(move || {
        let total = bytes.len() as u64;
        hub.broadcast(json!({"type": "transfer:progress", "task": transfer_task(&tid_progress, &session_id, "upload", &fname, &remote_path, 0, total, "running", None)}));
        let hub_job = hub.clone();
        let session_job = session_id.clone();
        let remote_job = remote_path.clone();
        let fname_job = fname.clone();
        let tid_job = tid_progress.clone();
        let outcome = with_ssh(&state_for_task, &session_id, move |session| {
            write_bytes(session, &remote_job, &bytes, |sent, total| {
                if cancel.load(Ordering::Relaxed) {
                    return false;
                }
                hub_job.broadcast(json!({"type": "transfer:progress", "task": transfer_task(&tid_job, &session_job, "upload", &fname_job, &remote_job, sent, total, "running", None)}));
                true
            })
            .map_err(|err| err.to_string())
        });
        match &outcome {
            Ok(()) => hub.broadcast(json!({"type": "transfer:progress", "task": transfer_task(&tid_progress, &session_id, "upload", &fname, &remote_path, total, total, "done", None)})),
            Err(err) => {
                let status = if err.contains("取消") { "cancelled" } else { "error" };
                hub.broadcast(json!({"type": "transfer:progress", "task": transfer_task(&tid_progress, &session_id, "upload", &fname, &remote_path, 0, total, status, Some(err.clone()))}));
            }
        }
        lock(&state_for_task.transfers).remove(&tid_progress);
        outcome.map(|_| tid_progress)
    })
    .await;
    match result {
        Ok(tid) => json_ok(json!({"tid": tid})),
        Err(err) => json_err(StatusCode::BAD_REQUEST, err),
    }
}

async fn cancel_transfer(_auth: Authed, State(state): State<AppState>, IdPath(id): IdPath<String>) -> Response {
    if let Some(flag) = lock(&state.transfers).get(&id) {
        flag.store(true, Ordering::Relaxed);
    }
    json_ok(json!(true))
}

async fn mcp_info(_auth: Authed, State(state): State<AppState>) -> Response {
    let port = state.port.load(Ordering::Relaxed);
    json_ok(json!({
        "transport": "streamable-http",
        "protocolVersion": runtime::MCP_PROTOCOL,
        "url": format!("http://127.0.0.1:{port}/mcp"),
        "sseUrl": format!("http://127.0.0.1:{port}/sse"),
        "messageUrl": format!("http://127.0.0.1:{port}/message"),
        "authorization": "Bearer",
        "tools": tool_names()
    }))
}

fn with_ssh<T: Send + 'static>(
    state: &AppState,
    session_id: &str,
    op: impl FnOnce(&ssh2::Session) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    state.hub.with_ssh(session_id, op)
}

async fn block<T: Send + 'static>(op: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    match tokio::task::spawn_blocking(op).await {
        Ok(result) => result,
        Err(err) => Err(format!("后台任务失败: {err}")),
    }
}

fn notify_catalog(state: &AppState, kind: &str) {
    state.hub.broadcast(json!({"type": "catalog:changed", "kind": kind}));
}

fn spawn_catalog_watch(state: AppState) {
    std::thread::spawn(move || {
        let mut seen = catalog_stamp(&state.data_dir);
        while !state.stopping.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(400));
            let now = catalog_stamp(&state.data_dir);
            if now != seen {
                seen = now;
                state.hub.broadcast(json!({"type": "catalog:changed", "kind": "disk"}));
            }
        }
    });
}

fn catalog_stamp(dir: &Path) -> String {
    ["hosts.json", "snippets.json", "tunnels.json", "settings.json", "command-history.json", "bindings.json"]
        .iter()
        .map(|name| {
            let meta = std::fs::metadata(dir.join(name)).ok();
            let mtime = meta
                .as_ref()
                .and_then(|item| item.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|dur| dur.as_millis())
                .unwrap_or(0);
            let len = meta.map(|item| item.len()).unwrap_or(0);
            format!("{name}:{mtime}:{len}")
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn json_ok(data: Value) -> Response {
    Json(json!({"ok": true, "data": data})).into_response()
}

fn json_err(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"ok": false, "error": message.into()}))).into_response()
}

fn transfer_task(
    id: &str,
    session_id: &str,
    direction: &str,
    filename: &str,
    remote_path: &str,
    transferred: u64,
    total: u64,
    status: &str,
    error: Option<String>,
) -> Value {
    json!({
        "id": id,
        "sessionId": session_id,
        "direction": direction,
        "filename": filename,
        "remotePath": remote_path,
        "transferred": transferred,
        "total": total,
        "status": status,
        "error": error
    })
}

#[derive(Deserialize, Default)]
struct PathQuery {
    path: Option<String>,
    /// 只用来拒绝把长期令牌放进下载地址。不要记录、不要写回响应。
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GrantBody {
    path: Option<String>,
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CursorWriteBody {
    confirm: Option<bool>,
}

fn reject_download_token(query: &PathQuery) -> Option<Response> {
    let carries = query.token.as_deref().is_some_and(|value| !value.trim().is_empty());
    if carries {
        Some(json_err(StatusCode::BAD_REQUEST, "下载地址不能携带访问令牌，请使用一次性下载票"))
    } else {
        None
    }
}

fn with_private_download(mut response: Response) -> Response {
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str("no-store") {
        headers.insert("cache-control", value);
    }
    if let Ok(value) = HeaderValue::from_str("no-referrer") {
        headers.insert("referrer-policy", value);
    }
    response
}

async fn issue_download_grant(
    _auth: Authed,
    State(state): State<AppState>,
    IdPath(id): IdPath<String>,
    Json(body): Json<GrantBody>,
) -> Response {
    let remote = body.path.unwrap_or_default();
    if remote.is_empty() {
        return json_err(StatusCode::BAD_REQUEST, "缺少 path 参数");
    }
    let kind = match body.kind.as_deref() {
        Some("archive") => DownloadKind::Archive,
        Some("file") | None => DownloadKind::File,
        Some(_) => return json_err(StatusCode::BAD_REQUEST, "kind 只能是 file 或 archive"),
    };
    if kind == DownloadKind::Archive && archive_path_rejected(&remote) {
        return json_err(StatusCode::BAD_REQUEST, "路径包含非法字符");
    }
    if state.hub.host_id(&id).is_err() {
        return json_err(StatusCode::BAD_REQUEST, "找不到这个会话");
    }
    let grant_id = {
        let mut grants = lock(&state.download_grants);
        match grants.issue(&id, &remote, kind, Instant::now()) {
            Ok(grant_id) => grant_id,
            Err(message) => return json_err(StatusCode::TOO_MANY_REQUESTS, message),
        }
    };
    json_ok(json!({ "href": format!("/api/downloads/{grant_id}") }))
}

async fn redeem_download(State(state): State<AppState>, headers: HeaderMap, IdPath(id): IdPath<String>) -> Response {
    if !host_is_local(&headers) {
        return json_err(StatusCode::FORBIDDEN, "仅允许本机访问");
    }
    if !origin_ok(&headers) {
        return json_err(StatusCode::FORBIDDEN, "拒绝跨站来源");
    }
    let grant = {
        let mut grants = lock(&state.download_grants);
        match grants.redeem(&id, Instant::now()) {
            Ok(grant) => grant,
            Err(message) => return json_err(StatusCode::UNAUTHORIZED, message),
        }
    };
    let response = match grant.kind {
        DownloadKind::File => stream_file_download(state, grant.session_id, grant.path).await,
        DownloadKind::Archive => stream_archive_download(state, grant.session_id, grant.path).await,
    };
    with_private_download(response)
}

async fn write_cursor_mcp(_auth: Authed, State(state): State<AppState>, Json(body): Json<CursorWriteBody>) -> Response {
    if body.confirm != Some(true) {
        return json_err(StatusCode::BAD_REQUEST, "需要确认后才会写入 Cursor 配置");
    }
    let path = match cursor_user_mcp_path() {
        Ok(path) => path,
        Err(message) => return json_err(StatusCode::BAD_REQUEST, message),
    };
    let port = state.port.load(Ordering::Relaxed);
    let url = format!("http://127.0.0.1:{port}/mcp");
    match write_cursor_mcp_file(&path, &url, &state.token) {
        Ok(action) => json_ok(json!({
            "action": action.as_str(),
            "path": path.display().to_string()
        })),
        Err(message) => json_err(StatusCode::BAD_REQUEST, message),
    }
}

fn archive_path_rejected(remote: &str) -> bool {
    remote.chars().any(|c| matches!(c, ';' | '|' | '&' | '$' | '>' | '<' | '`' | '\n' | '\r'))
}

fn render_index(token: &str) -> String {
    let raw = read_ui("index.html");
    let inject = format!(
        "<script>window.__TIANSHELL_TOKEN__={};</script>",
        serde_json::to_string(token).unwrap_or_else(|_| "\"\"".into())
    );
    match raw.rfind("</head>") {
        Some(pos) => format!("{}{}{}", &raw[..pos], inject, &raw[pos..]),
        None => raw,
    }
}

fn read_ui(name: &str) -> String {
    let mut dirs = Vec::new();
    if let Ok(dir) = std::env::var("TIANSHELL_UI") {
        if !dir.trim().is_empty() {
            dirs.push(PathBuf::from(dir.trim()));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("ui"));
        dirs.push(cwd.join("dist"));
    }
    for dir in dirs {
        if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
            if !text.is_empty() {
                return text;
            }
        }
    }
    match name {
        "index.html" => include_str!("../../ui/index.html").to_string(),
        "renderer.js" => include_str!("../../ui/renderer.js").to_string(),
        "renderer.css" => include_str!("../../ui/renderer.css").to_string(),
        _ => String::new(),
    }
}

fn base_name(path: &str) -> String {
    path.rsplit('/').next().filter(|item| !item.is_empty()).unwrap_or("download.bin").to_string()
}

fn encode_filename(name: &str) -> String {
    let mut out = String::new();
    for byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_') {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn chosen_port(explicit: Option<u16>) -> u16 {
    if let Some(port) = explicit {
        return port;
    }
    std::env::var("TIANSHELL_PORT").ok().and_then(|value| value.trim().parse().ok()).unwrap_or(53921)
}

fn load_token(dir: &Path) -> String {
    if let Ok(value) = std::env::var("TIANSHELL_TOKEN") {
        let token = value.trim();
        if !token.is_empty() {
            return token.to_string();
        }
    }
    let path = dir.join("mcp.token");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let token = existing.trim();
        if !token.is_empty() {
            return token.to_string();
        }
    }
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = hex_encode(&bytes);
    let _ = std::fs::write(&path, &token);
    restrict_user_file(&path);
    token
}

fn default_snippets() -> Vec<Value> {
    vec![
        json!({"name": "系统负载与运行时间", "category": "诊断", "command": "uptime && free -h"}),
        json!({"name": "监听端口检查", "category": "网络", "command": "ss -tulnp"}),
        json!({"name": "磁盘空间占用", "category": "磁盘", "command": "df -hT"}),
        json!({"name": "内存占用 Top10", "category": "进程", "command": "ps aux --sort=-%mem | head -n 11"}),
    ]
}

fn launch_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(["/c", "start", "", url]).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}
