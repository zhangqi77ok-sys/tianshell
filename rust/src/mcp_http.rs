use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::stream;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;
use crate::auth::{authorize, origin_ok, AuthError};
use crate::mcp::JsonRpcRequest;
use crate::paths::resolve_data_dir;
use crate::runtime::read_runtime;
use crate::state::{lock, AppState};

#[derive(Debug, Deserialize, Default)]
pub struct TokenQuery {
    pub token: Option<String>,
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
}

pub async fn post_mcp(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    body: Bytes,
) -> Response {
    if let Err(err) = gate(&state, &headers, query.token.as_deref()) {
        return err;
    }
    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(err) => {
            return Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32700, "message": format!("无法解析 JSON-RPC: {err}")}
            }))
            .into_response();
        }
    };
    if let Some(items) = parsed.as_array() {
        let mut responses = Vec::new();
        let mut session_id = session_header(&headers);
        for item in items {
            let (response, next_session) = dispatch_one(&state, item.clone(), session_id.clone()).await;
            if let Some(next) = next_session {
                session_id = Some(next);
            }
            if let Some(response) = response {
                responses.push(response);
            }
        }
        return with_session(Json(Value::Array(responses)).into_response(), session_id.as_deref());
    }
    let (response, session_id) = dispatch_one(&state, parsed, session_header(&headers)).await;
    match response {
        Some(body) => with_session(Json(body).into_response(), session_id.as_deref()),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

pub async fn get_mcp(State(state): State<AppState>, headers: HeaderMap, Query(query): Query<TokenQuery>) -> Response {
    if let Err(err) = gate(&state, &headers, query.token.as_deref()) {
        return err;
    }
    let events = stream::unfold(0u32, |count| async move {
        if count > 0 {
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
        Some((Ok::<_, std::convert::Infallible>(Event::default().comment("tianshell")), count + 1))
    });
    Sse::new(events).keep_alive(KeepAlive::default()).into_response()
}

pub async fn delete_mcp(State(state): State<AppState>, headers: HeaderMap, Query(query): Query<TokenQuery>) -> Response {
    if let Err(err) = gate(&state, &headers, query.token.as_deref()) {
        return err;
    }
    if let Some(session_id) = session_header(&headers) {
        lock(&state.mcp_sessions).remove(&session_id);
    }
    StatusCode::NO_CONTENT.into_response()
}

pub async fn options_mcp(headers: HeaderMap) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    let header = response.headers_mut();
    header.insert("access-control-allow-methods", HeaderValue::from_static("GET, POST, DELETE, OPTIONS"));
    header.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("authorization, content-type, accept, mcp-session-id, mcp-protocol-version, x-tianshell-token"),
    );
    if origin_ok(&headers) {
        if let Some(origin) = headers.get(axum::http::header::ORIGIN) {
            if let Ok(value) = HeaderValue::from_bytes(origin.as_bytes()) {
                header.insert("access-control-allow-origin", value);
            }
        }
    }
    response
}

pub async fn get_sse(State(state): State<AppState>, headers: HeaderMap, Query(query): Query<TokenQuery>) -> Response {
    if let Err(err) = gate(&state, &headers, query.token.as_deref()) {
        return err;
    }
    let id = Uuid::new_v4().to_string();
    let (inbound, mut inbound_rx) = mpsc::unbounded_channel();
    state.sse.insert(id.clone(), inbound);
    let (outbound, outbound_rx) = mpsc::channel::<Result<Event, std::convert::Infallible>>(32);
    let endpoint = format!("/message?sessionId={id}");
    let sse = state.sse.clone();
    let remove_id = id.clone();
    tokio::spawn(async move {
        let hello = Event::default().event("endpoint").data(endpoint);
        if outbound.send(Ok(hello)).await.is_err() {
            sse.remove(&remove_id);
            return;
        }
        while let Some(message) = inbound_rx.recv().await {
            let event = Event::default().event("message").data(message);
            if outbound.send(Ok(event)).await.is_err() {
                break;
            }
        }
        sse.remove(&remove_id);
    });
    Sse::new(ReceiverStream::new(outbound_rx)).keep_alive(KeepAlive::default()).into_response()
}

pub async fn post_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    body: Bytes,
) -> Response {
    if let Err(err) = gate(&state, &headers, query.token.as_deref()) {
        return err;
    }
    let Some(session_id) = query.session_id else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(request) = serde_json::from_slice::<JsonRpcRequest>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let mcp = state.mcp.clone();
    let response = match tokio::task::spawn_blocking(move || mcp.handle_request(request)).await {
        Ok(response) => response,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if let Some(response) = response {
        if !state.sse.send(&session_id, response.to_string()) {
            return StatusCode::NOT_FOUND.into_response();
        }
    }
    StatusCode::ACCEPTED.into_response()
}

pub fn run_stdio_attach() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = resolve_data_dir();
    let info = read_runtime(&data_dir).ok_or(
        "TianShell 工作台没有在运行。先执行 `cargo run --bin tianshell -- serve`，MCP 会随工作台一起启动。",
    )?;
    let probe = format!("127.0.0.1:{}", info.port);
    TcpStream::connect_timeout(
        &probe.parse().map_err(|err| format!("地址无效: {err}"))?,
        Duration::from_secs(2),
    )
    .map_err(|_| format!("工作台端口 {} 没有响应。请重新启动 TianShell。", info.port))?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut mcp_session = None;
    for line in stdin.lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (status, headers, body) = http_post(&info.mcp_url, &info.token, mcp_session.as_deref(), trimmed)?;
        if let Some(session_id) = headers.get("mcp-session-id") {
            mcp_session = Some(session_id.clone());
        }
        if status == 202 || body.trim().is_empty() {
            continue;
        }
        writeln!(stdout, "{}", body.trim())?;
        stdout.flush()?;
    }
    Ok(())
}

async fn dispatch_one(state: &AppState, value: Value, session_header: Option<String>) -> (Option<Value>, Option<String>) {
    let Ok(request) = serde_json::from_value::<JsonRpcRequest>(value) else {
        return (
            Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32600, "message": "无效的 JSON-RPC 请求"}
            })),
            session_header,
        );
    };
    let is_initialize = request.method == "initialize";
    if let Some(session_id) = &session_header {
        if !is_initialize && !lock(&state.mcp_sessions).contains(session_id) {
            return (
                Some(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request.id,
                    "error": {"code": -32001, "message": "MCP 会话不存在或已结束"}
                })),
                None,
            );
        }
    }
    if request.id.is_none() {
        return (None, session_header);
    }
    let mcp = state.mcp.clone();
    let response = match tokio::task::spawn_blocking(move || mcp.handle_request(request)).await {
        Ok(response) => response,
        Err(err) => Some(serde_json::json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": {"code": -32603, "message": format!("后台任务失败: {err}")}
        })),
    };
    if is_initialize {
        let session_id = Uuid::new_v4().to_string();
        lock(&state.mcp_sessions).insert(session_id.clone());
        return (response, Some(session_id));
    }
    (response, session_header)
}

fn with_session(mut response: Response, session_id: Option<&str>) -> Response {
    if let Some(session_id) = session_id {
        if let Ok(value) = HeaderValue::from_str(session_id) {
            response.headers_mut().insert("mcp-session-id", value);
        }
    }
    response
}

fn session_header(headers: &HeaderMap) -> Option<String> {
    headers.get("mcp-session-id").and_then(|value| value.to_str().ok()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn gate(state: &AppState, headers: &HeaderMap, query_token: Option<&str>) -> Result<(), Response> {
    let synthetic = query_token.map(|value| format!("token={value}"));
    authorize(headers, synthetic.as_deref(), &state.token).map_err(auth_response)
}

fn auth_response(err: AuthError) -> Response {
    let status = StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::UNAUTHORIZED);
    (status, Json(serde_json::json!({"ok": false, "error": err.message()}))).into_response()
}

fn http_post(url: &str, token: &str, session: Option<&str>, body: &str) -> Result<(u16, HashMap<String, String>, String), String> {
    let (host, port, path) = parse_local_http(url)?;
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|err| err.to_string())?
        .next()
        .ok_or_else(|| "无法解析本机 MCP 地址".to_string())?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(|err| err.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(120))).ok();
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(session) = session {
        request.push_str(&format!("Mcp-Session-Id: {session}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes()).map_err(|err| err.to_string())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|err| err.to_string())?;
    parse_http_response(&raw)
}

fn parse_local_http(url: &str) -> Result<(String, u16, String), String> {
    let rest = url.strip_prefix("http://").ok_or_else(|| "只支持 http://127.0.0.1 的 MCP 地址".to_string())?;
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.parse::<u16>().map_err(|_| "端口无效".to_string())?),
        None => (authority.to_string(), 80),
    };
    if host != "127.0.0.1" && !host.eq_ignore_ascii_case("localhost") && host != "[::1]" {
        return Err("MCP 附着只连接本机回环地址".into());
    }
    Ok((host, port, path))
}

fn parse_http_response(raw: &[u8]) -> Result<(u16, HashMap<String, String>, String), String> {
    let split = raw.windows(4).position(|window| window == b"\r\n\r\n").ok_or_else(|| "HTTP 响应不完整".to_string())?;
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| "HTTP 头不是 UTF-8".to_string())?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status = status_line.split_whitespace().nth(1).and_then(|item| item.parse().ok()).unwrap_or(0);
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let mut body = &raw[split + 4..];
    if let Some(len) = headers.get("content-length").and_then(|value| value.parse::<usize>().ok()) {
        if body.len() > len {
            body = &body[..len];
        }
    }
    Ok((status, headers, String::from_utf8_lossy(body).trim().to_string()))
}
