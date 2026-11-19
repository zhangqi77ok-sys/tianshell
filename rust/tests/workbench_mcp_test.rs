use base64::Engine;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};
use serde_json::{json, Value};
use tempfile::tempdir;
use tianshell_core::http_server::{start_workbench, ServerOptions};
use tianshell_core::mcp::{tool_names, JsonRpcRequest, McpHandler};
use tianshell_core::runtime::read_runtime;
use tianshell_core::store::HostStore;
use tianshell_core::vault::load_or_create_master_key;

#[test]
fn mcp_handler_lists_public_hosts_and_workbench_tools() {
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
    let names: Vec<&str> = tools.iter().filter_map(|tool| tool["name"].as_str()).collect();
    assert_eq!(names.len(), tool_names().len());
    for name in tool_names() {
        assert!(names.contains(name), "missing {name}");
    }
    let terminals = handler
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(3)),
            method: "tools/call".into(),
            params: Some(json!({"name": "list_terminal_sessions", "arguments": {}})),
        })
        .unwrap();
    let terminal_text = terminals["result"]["content"][0]["text"].as_str().unwrap();
    assert!(terminal_text.contains("\"attached\": false"));
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

    let (status, _, body) = http(server.port, "POST", "/api/sessions", headers, Some(r#"{"kind":"local","cols":100,"rows":30}"#));
    assert_eq!(status, 200, "{body}");
    let session_id = serde_json::from_str::<Value>(&body).unwrap()["data"]["sessionId"].as_str().unwrap().to_string();
    let (status, _, body) = http(
        server.port,
        "GET",
        &format!("/api/sessions/{session_id}/sftp/list?path=/"),
        headers,
        None,
    );
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("会话未连接到 SSH 主机"), "{body}");

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
    assert!(body.contains("list_hosts") && body.contains("system_metrics") && body.contains("list_terminal_sessions"));
    let (status, _, body) = http_with_bearer(
        server.port,
        "POST",
        "/mcp",
        &server.token,
        Some(&session),
        Some(&format!(
            r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"list_terminal_sessions","arguments":{{}}}}}}"#
        )),
    );
    assert_eq!(status, 200, "{body}");
    let listed: Value = serde_json::from_str(&body).unwrap();
    let listed_text = listed["result"]["content"][0]["text"].as_str().unwrap();
    let listed_value: Value = serde_json::from_str(listed_text).unwrap();
    assert_eq!(listed_value["attached"], true);
    assert!(listed_text.contains(&session_id), "{listed_text}");
    let (status, _, body) = http_with_bearer(
        server.port,
        "POST",
        "/mcp",
        &server.token,
        Some(&session),
        Some(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"session_status","arguments":{}}}"#),
    );
    assert_eq!(status, 200, "{body}");
    assert!(!body.contains(&session_id), "run_command records must not include the interactive tab: {body}");
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

#[test]
fn host_and_session_changes_arrive_on_the_workbench_socket() {
    let dir = tempdir().unwrap();
    let server = start_workbench(ServerOptions {
        data_dir: Some(dir.path().to_path_buf()),
        port: Some(0),
        open_browser: false,
    })
    .expect("workbench");
    let mut sock = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(200))).ok();
    let upgrade = format!(
        "GET /ws?token={} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        server.token,
        port = server.port
    );
    sock.write_all(upgrade.as_bytes()).unwrap();
    let mut pending = Vec::new();
    let header = read_http_head(&mut sock, &mut pending);
    assert!(header.contains("101"), "{header}");
    std::thread::sleep(Duration::from_millis(80));

    let (status, _, body) = http(server.port, "POST", "/api/sessions", Some(&server.token), Some(r#"{"kind":"local"}"#));
    assert_eq!(status, 200, "{body}");
    let (status, _, body) = http(
        server.port,
        "POST",
        "/api/hosts",
        Some(&server.token),
        Some(r#"{"alias":"live-box","host":"10.9.9.9","username":"root","authType":"password","secret":"pw"}"#),
    );
    assert_eq!(status, 200, "{body}");

    let mut text = decode_ws_text(&mut pending);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !(text.contains("session:opened") && text.contains("catalog:changed")) {
        let mut buf = [0u8; 8192];
        match sock.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                pending.extend_from_slice(&buf[..n]);
                text.push_str(&decode_ws_text(&mut pending));
            }
            Err(_) => {}
        }
    }
    assert!(text.contains("session:opened"), "missing session event: {text}");
    assert!(text.contains("catalog:changed"), "missing catalog event: {text}");
    let (status, _, listed) = http(server.port, "GET", "/api/hosts", Some(&server.token), None);
    assert_eq!(status, 200, "{listed}");
    assert!(listed.contains("live-box"), "{listed}");
    drop(server);
}

fn read_http_head(stream: &mut TcpStream, pending: &mut Vec<u8>) -> String {
    let mut buf = [0u8; 2048];
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                pending.extend_from_slice(&buf[..n]);
                if let Some(split) = pending.windows(4).position(|window| window == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&pending[..split]).into_owned();
                    pending.drain(..split + 4);
                    return head;
                }
            }
            Err(_) => {}
        }
    }
    String::from_utf8_lossy(pending).into_owned()
}

fn decode_ws_text(buf: &mut Vec<u8>) -> String {
    let mut out = String::new();
    let mut offset = 0;
    while buf.len() - offset >= 2 {
        let opcode = buf[offset] & 0x0f;
        let mut len = (buf[offset + 1] & 0x7f) as usize;
        let mut header = 2usize;
        if len == 126 {
            if buf.len() - offset < 4 {
                break;
            }
            len = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]) as usize;
            header = 4;
        } else if len == 127 {
            break;
        }
        if buf.len() < offset + header + len {
            break;
        }
        if opcode == 0x1 {
            out.push_str(&String::from_utf8_lossy(&buf[offset + header..offset + header + len]));
        }
        offset += header + len;
    }
    buf.drain(..offset);
    out
}

#[test]
fn hands_use_the_open_tab_and_leave_secrets_out_of_memory() {
    let dir = tempdir().unwrap();
    let server = start_workbench(ServerOptions {
        data_dir: Some(dir.path().to_path_buf()),
        port: Some(0),
        open_browser: false,
    })
    .expect("workbench");
    let headers = Some(server.token.as_str());
    let (status, _, body) = http(
        server.port,
        "POST",
        "/api/hosts",
        headers,
        Some(r#"{"alias":"lab","host":"10.2.2.2","username":"ops","authType":"password","secret":"hidden-password"}"#),
    );
    assert_eq!(status, 200, "{body}");
    let (status, _, body) = http(server.port, "POST", "/api/sessions", headers, Some(r#"{"kind":"local"}"#));
    assert_eq!(status, 200, "{body}");
    let session_id = serde_json::from_str::<Value>(&body).unwrap()["data"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;
    let (status, response_headers, body) = http_with_bearer(server.port, "POST", "/mcp", &server.token, None, Some(init));
    assert_eq!(status, 200, "{body}");
    let mcp_session = response_headers
        .iter()
        .find(|(name, _)| name == "mcp-session-id")
        .map(|(_, value)| value.clone())
        .unwrap();

    let run = format!(
        r#"{{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{{"name":"run_command","arguments":{{"host":"local","session_id":"{session_id}","command":"printf tianshell-hands-ok","timeout_seconds":15}}}}}}"#
    );
    let (status, _, body) = http_with_bearer(server.port, "POST", "/mcp", &server.token, Some(&mcp_session), Some(&run));
    assert_eq!(status, 200, "{body}");
    let payload = tool_json(&body);
    assert_eq!(payload["attached"], true, "{body}");
    assert_eq!(payload["sessionId"], session_id, "{body}");
    assert_eq!(payload["exitCode"], 0, "{body}");
    assert!(payload["stdout"].as_str().unwrap_or("").contains("tianshell-hands-ok"), "{body}");

    let listed = mcp_call(server.port, &server.token, &mcp_session, "list_terminal_sessions", "{}");
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "{listed}");
    assert_eq!(sessions[0]["sessionId"], session_id);

    let secret = mcp_call(
        server.port,
        &server.token,
        &mcp_session,
        "run_command",
        &format!(r#"{{"host":"local","session_id":"{session_id}","command":"printf done-$AWS_SECRET_ACCESS_KEY; export AWS_SECRET_ACCESS_KEY=supersecretvalue","timeout_seconds":15}}"#),
    );
    let secret_text = secret.to_string();
    assert!(!secret_text.contains("supersecretvalue"), "{secret_text}");
    let raw = std::fs::read_to_string(dir.path().join("command-history.json")).unwrap();
    assert!(raw.contains("tianshell-hands-ok"), "{raw}");
    assert!(!raw.contains("supersecretvalue"), "{raw}");
    assert!(!raw.contains(&server.token), "{raw}");

    let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\\nabc\\n-----END OPENSSH PRIVATE KEY-----";
    let note = mcp_call(
        server.port,
        &server.token,
        &mcp_session,
        "set_host_note",
        &format!(r#"{{"host":"lab","notes":"{pem}"}}"#),
    );
    assert_eq!(note["isError"], true, "{note}");
    assert!(!note.to_string().contains("BEGIN OPENSSH"));
    let noted = mcp_call(
        server.port,
        &server.token,
        &mcp_session,
        "set_host_note",
        r#"{"host":"lab","notes":"postgres on 5432"}"#,
    );
    assert!(noted.to_string().contains("postgres on 5432"), "{noted}");
    let hosts = mcp_call(server.port, &server.token, &mcp_session, "list_hosts", "{}");
    let hosts_text = hosts.to_string();
    assert!(hosts_text.contains("postgres on 5432"), "{hosts_text}");
    assert!(!hosts_text.contains("hidden-password"), "{hosts_text}");

    let (status, _, body) = http(
        server.port,
        "POST",
        "/api/snippets",
        headers,
        Some(r#"{"name":"db","category":"ops","command":"mysql --password=sekritvalue"}"#),
    );
    assert_eq!(status, 200, "{body}");
    let snippets = mcp_call(server.port, &server.token, &mcp_session, "list_snippets", "{}");
    let snippets_text = snippets.to_string();
    assert!(!snippets_text.contains("sekritvalue"), "{snippets_text}");
    assert!(snippets_text.contains("[redacted]"), "{snippets_text}");

    let binding = mcp_call(
        server.port,
        &server.token,
        &mcp_session,
        "save_binding",
        r#"{"name":"api","localPath":"/src/api","host":"lab","remotePath":"/srv/api","note":"port 8080"}"#,
    );
    assert_eq!(binding["remotePath"], "/srv/api", "{binding}");
    drop(server);
    let history = tianshell_core::history::CommandHistory::open(dir.path().join("command-history.json")).unwrap();
    let again = history.search(Some("tianshell-hands-ok"), None, 5);
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].exit_code, Some(0));
    assert!(again[0].cwd.is_some(), "{again:?}");
}

#[test]
fn exit_probe_stays_off_the_open_terminal() {
    let dir = tempdir().unwrap();
    let server = start_workbench(ServerOptions {
        data_dir: Some(dir.path().to_path_buf()),
        port: Some(0),
        open_browser: false,
    })
    .expect("workbench");
    let mut sock = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(200))).ok();
    let upgrade = format!(
        "GET /ws?token={} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        server.token,
        port = server.port
    );
    sock.write_all(upgrade.as_bytes()).unwrap();
    let mut pending = Vec::new();
    let header = read_http_head(&mut sock, &mut pending);
    assert!(header.contains("101"), "{header}");

    let (status, _, body) = http(
        server.port,
        "POST",
        "/api/sessions",
        Some(&server.token),
        Some(r#"{"kind":"local"}"#),
    );
    assert_eq!(status, 200, "{body}");
    let session_id = serde_json::from_str::<Value>(&body).unwrap()["data"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;
    let (status, response_headers, body) =
        http_with_bearer(server.port, "POST", "/mcp", &server.token, None, Some(init));
    assert_eq!(status, 200, "{body}");
    let mcp_session = response_headers
        .iter()
        .find(|(name, _)| name == "mcp-session-id")
        .map(|(_, value)| value.clone())
        .unwrap();
    let run = format!(
        r#"{{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{{"name":"run_command","arguments":{{"host":"local","session_id":"{session_id}","command":"printf tianshell-probe-ok","timeout_seconds":15}}}}}}"#
    );
    let (status, _, body) = http_with_bearer(
        server.port,
        "POST",
        "/mcp",
        &server.token,
        Some(&mcp_session),
        Some(&run),
    );
    assert_eq!(status, 200, "{body}");
    let payload = tool_json(&body);
    assert_eq!(payload["exitCode"], 0, "{body}");
    let cwd = payload["cwd"].as_str().unwrap_or("");
    assert!(
        !cwd.is_empty() && (cwd.starts_with('/') || cwd.contains(':')),
        "{body}"
    );
    let stdout = payload["stdout"].as_str().unwrap_or("").to_string();
    assert!(stdout.contains("tianshell-probe-ok"), "{stdout}");
    assert_probe_hidden(&stdout);

    let mut screen = String::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut raw = String::new();
    loop {
        raw.push_str(&decode_ws_text(&mut pending));
        if Instant::now() >= deadline {
            break;
        }
        let mut buf = [0u8; 8192];
        match sock.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => pending.extend_from_slice(&buf[..n]),
            Err(_) if Instant::now() >= deadline => break,
            Err(_) => {}
        }
    }
    raw.push_str(&decode_ws_text(&mut pending));
    let stream = serde_json::Deserializer::from_str(&raw).into_iter::<Value>();
    for msg in stream.flatten() {
        if msg.get("type").and_then(|value| value.as_str()) != Some("term:data") {
            continue;
        }
        if msg.get("sessionId").and_then(|value| value.as_str()) != Some(session_id.as_str()) {
            continue;
        }
        let Some(data) = msg.get("data").and_then(|value| value.as_str()) else {
            continue;
        };
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
            continue;
        };
        screen.push_str(&String::from_utf8_lossy(&bytes));
    }
    assert!(
        screen.contains("tianshell-probe-ok"),
        "the person should see the command output: {screen:?}"
    );
    assert_probe_hidden(&screen);
    drop(server);
}

fn assert_probe_hidden(text: &str) {
    assert!(!text.contains("__TS_END_"), "exit marker leaked: {text:?}");
    assert!(!text.contains("__ts_ec"), "probe variable leaked: {text:?}");
    assert!(!text.contains("stty"), "echo toggle leaked: {text:?}");
    assert!(!text.contains("ec=$?"), "quiet probe leaked: {text:?}");
}

#[test]
fn note_only_host_edit_keeps_auth_and_secret() {
    let dir = tempdir().unwrap();
    let server = start_workbench(ServerOptions {
        data_dir: Some(dir.path().to_path_buf()),
        port: Some(0),
        open_browser: false,
    })
    .expect("workbench");
    let (status, _, body) = http(
        server.port,
        "POST",
        "/api/hosts",
        Some(&server.token),
        Some(
            r#"{"alias":"lab","host":"127.0.0.1","username":"ops","authType":"password","secret":"hidden-password","notes":"old"}"#,
        ),
    );
    assert_eq!(status, 200, "{body}");
    let saved: Value = serde_json::from_str(&body).unwrap();
    let id = saved["data"]["id"].as_str().unwrap();
    assert_eq!(saved["data"]["authType"], "password");
    let edit = format!(
        r#"{{"id":"{id}","alias":"lab","host":"127.0.0.1","username":"ops","authType":"password","secret":"","notes":"only the note"}}"#
    );
    let (status, _, body) = http(
        server.port,
        "POST",
        "/api/hosts",
        Some(&server.token),
        Some(&edit),
    );
    assert_eq!(status, 200, "{body}");
    let edited: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(edited["data"]["notes"], "only the note", "{body}");
    assert_eq!(edited["data"]["authType"], "password", "{body}");
    let switched = format!(
        r#"{{"id":"{id}","alias":"lab","host":"127.0.0.1","username":"ops","authType":"privateKey","secret":"","notes":"only the note"}}"#
    );
    let (status, _, body) = http(
        server.port,
        "POST",
        "/api/hosts",
        Some(&server.token),
        Some(&switched),
    );
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("请填写私钥"), "{body}");
    let key = load_or_create_master_key(&dir.path().join("vault.key")).unwrap();
    let store = HostStore::load(dir.path().join("hosts.json"), key).unwrap();
    let host = store.list().iter().find(|item| item.id == id).unwrap();
    assert_eq!(store.open_secret(&host.secret).unwrap(), "hidden-password");
    drop(server);
}

fn tool_json(body: &str) -> Value {
    let envelope: Value = serde_json::from_str(body).unwrap();
    let text = envelope["result"]["content"][0]["text"].as_str().unwrap();
    let mut parsed: Value = serde_json::from_str(text).unwrap_or(json!({"text": text}));
    if envelope["result"]["isError"] == true {
        parsed["isError"] = json!(true);
    }
    parsed
}

fn mcp_call(port: u16, token: &str, session: &str, name: &str, arguments: &str) -> Value {
    let body = format!(r#"{{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#);
    let (status, _, response) = http_with_bearer(port, "POST", "/mcp", token, Some(session), Some(&body));
    assert_eq!(status, 200, "{response}");
    tool_json(&response)
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
    let timeout = if path == "/sse" { Duration::from_millis(400) } else { Duration::from_secs(25) };
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
