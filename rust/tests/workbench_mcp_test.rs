use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};
use serde_json::{json, Value};
use tempfile::tempdir;
use tianshell_core::http_server::{start_workbench, ServerOptions};
use tianshell_core::mcp::{JsonRpcRequest, McpHandler};
use tianshell_core::runtime::read_runtime;
use tianshell_core::store::HostStore;
use tianshell_core::vault::load_or_create_master_key;

#[test]
fn mcp_handler_lists_public_hosts_and_seven_tools() {
    let dir = tempdir().unwrap();
    let key = load_or_create_master_key(&dir.path().join("vault.key")).unwrap();
    let mut store = HostStore::load(dir.path().join("hosts.json"), key).unwrap();
    let saved = store
        .save_draft(&json!({
            "alias": "edge",
            "host": "10.1.1.1",
            "username": "ops",
            "authType": "password",
            "secret": "hidden-password"
        }))
        .unwrap();
    assert_eq!(saved.alias, "edge");
    let handler = McpHandler::from_hosts(std::sync::Arc::new(std::sync::Mutex::new(store)));
    let listed = handler
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: "tools/list".into(),
            params: None,
        })
        .unwrap();
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 7);
    let names: Vec<&str> = tools.iter().filter_map(|tool| tool["name"].as_str()).collect();
    for name in ["list_hosts", "run_command", "session_status", "sftp_list", "sftp_read", "sftp_write", "system_metrics"] {
        assert!(names.contains(&name), "missing {name}");
    }
    let hosts = handler
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(2)),
            method: "tools/call".into(),
            params: Some(json!({"name": "list_hosts", "arguments": {}})),
        })
        .unwrap();
    let text = hosts["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("edge"));
    assert!(!text.contains("hidden-password"));
}

#[test]
fn workbench_starts_mcp_with_the_http_server() {
    let dir = tempdir().unwrap();
    let server = start_workbench(ServerOptions {
        data_dir: Some(dir.path().to_path_buf()),
        port: Some(0),
        open_browser: false,
    })
    .expect("workbench");
    let base = format!("http://127.0.0.1:{}", server.port);

    let (status, _, body) = http(server.port, "GET", "/api/hosts", None, None);
    assert_eq!(status, 401, "{body}");

    let (status, _, body) = http(server.port, "GET", "/", None, None);
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("TianShell"));
    assert!(body.contains(&server.token));

    let headers = Some(server.token.as_str());
    let (status, _, body) = http(server.port, "POST", "/api/groups", headers, Some(r#"{"name":"研发机"}"#));
    assert_eq!(status, 200, "{body}");
    let (status, _, body) = http(
        server.port,
        "POST",
        "/api/hosts",
        headers,
        Some(r#"{"alias":"lab","host":"127.0.0.1","username":"root","authType":"password","secret":"pw"}"#),
    );
    assert_eq!(status, 200, "{body}");
    let saved: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(saved["data"]["alias"], "lab");
    assert!(saved["data"].get("secret").is_none());

    let (status, _, body) = http(
        server.port,
        "POST",
        "/api/snippets",
        headers,
        Some(r#"{"name":"负载","category":"监控","command":"uptime"}"#),
    );
    assert_eq!(status, 200, "{body}");
    let (status, _, body) = http(server.port, "PUT", "/api/settings", headers, Some(r#"{"fontSize":16,"theme":"light"}"#));
    assert_eq!(status, 200, "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["data"]["fontSize"], 16);

    let (status, _, body) = http(server.port, "POST", "/api/sessions", headers, Some(r#"{"kind":"local"}"#));
    assert_eq!(status, 200, "{body}");
    let session_id = serde_json::from_str::<Value>(&body).unwrap()["data"]["sessionId"].as_str().unwrap().to_string();

    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;
    let (status, _response_headers, body) = http(server.port, "POST", "/mcp", None, Some(init));
    assert_eq!(status, 401, "{body}");
    let (status, response_headers, body) = http_with_bearer(server.port, "POST", "/mcp", &server.token, None, Some(init));
    assert_eq!(status, 200, "{body}");
    let init_body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(init_body["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(init_body["result"]["serverInfo"]["name"], "tianshell");
    let session = response_headers
        .iter()
        .find(|(name, _)| name == "mcp-session-id")
        .map(|(_, value)| value.clone())
        .expect("Mcp-Session-Id");
    let (status, _, body) = http_with_bearer(
        server.port,
        "POST",
        "/mcp",
        &server.token,
        Some(&session),
        Some(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#),
    );
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("list_hosts") && body.contains("system_metrics"));
    let (status, _, body) = http_with_bearer(
        server.port,
        "POST",
        "/mcp",
        &server.token,
        Some(&session),
        Some(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_hosts","arguments":{}}}"#),
    );
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("lab"));
    assert!(!body.contains("\"secret\""));

    let runtime = read_runtime(dir.path()).expect("runtime.json");
    assert_eq!(runtime.port, server.port);
    assert_eq!(runtime.mcp_url, format!("{base}/mcp"));
    assert_eq!(runtime.token, server.token);
    assert_eq!(runtime.pid, std::process::id());

    let (status, _, body) = http_with_bearer(server.port, "GET", "/sse", &server.token, None, None);
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("endpoint") || body.contains("/message?sessionId="), "{body}");

    let (status, _, _) = http(server.port, "DELETE", &format!("/api/sessions/{session_id}"), headers, None);
    assert_eq!(status, 200);
    drop(server);
    assert!(read_runtime(dir.path()).is_none());
}

fn http(port: u16, method: &str, path: &str, token: Option<&str>, body: Option<&str>) -> (u16, Vec<(String, String)>, String) {
    let mut extra = String::new();
    if let Some(token) = token {
        extra.push_str(&format!("x-tianshell-token: {token}\r\n"));
    }
    exchange(port, method, path, &extra, body)
}

fn http_with_bearer(
    port: u16,
    method: &str,
    path: &str,
    token: &str,
    session: Option<&str>,
    body: Option<&str>,
) -> (u16, Vec<(String, String)>, String) {
    let mut extra = format!("Authorization: Bearer {token}\r\nAccept: application/json, text/event-stream\r\n");
    if let Some(session) = session {
        extra.push_str(&format!("Mcp-Session-Id: {session}\r\n"));
    }
    exchange(port, method, path, &extra, body)
}

fn exchange(port: u16, method: &str, path: &str, extra_headers: &str, body: Option<&str>) -> (u16, Vec<(String, String)>, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let timeout = if path == "/sse" { Duration::from_millis(400) } else { Duration::from_secs(8) };
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
    let body = body.unwrap_or("");
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if !body.is_empty() {
        request.push_str("Content-Type: application/json\r\n");
    }
    request.push_str(extra_headers);
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    let started = Instant::now();
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock || err.kind() == std::io::ErrorKind::TimedOut => {
                if path == "/sse" && !raw.is_empty() {
                    break;
                }
                if started.elapsed() > timeout + Duration::from_secs(1) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> (u16, Vec<(String, String)>, String) {
    let split = raw.windows(4).position(|window| window == b"\r\n\r\n").unwrap_or(raw.len());
    let head = String::from_utf8_lossy(&raw[..split.min(raw.len())]);
    let body = if split + 4 <= raw.len() {
        String::from_utf8_lossy(&raw[split + 4..]).into_owned()
    } else {
        String::new()
    };
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let headers = lines
        .filter_map(|line| line.split_once(':').map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string())))
        .collect();
    (status, headers, body)
}
