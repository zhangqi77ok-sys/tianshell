//! MCP hands: drive an open terminal, remember commands, and touch files
//! without borrowing the interactive SSH actor thread.

use std::time::Duration;
use base64::Engine;
use serde_json::{json, Value};
use uuid::Uuid;
use crate::history::HistoryDraft;
use crate::mcp::{tool_text, McpHandler, SessionRecord};
use crate::redact::{contains_secret, redact_secrets};
use crate::screen::{parse_exec_cwd, strip_exec_cwd, wrap_exec};
use crate::sftp::{self, read_bytes_limited};
use crate::ssh::{exec_command, resolve_targets, truncate_output};
use crate::state::lock;
use crate::terminal::{is_local_host, route_run, RunRoute};

const OUTPUT_LIMIT: usize = 200_000;

impl McpHandler {
    pub(crate) fn tool_run_command(&self, args: Value) -> Result<Value, String> {
        let command = required_command(&args)?;
        let new_session = args.get("new_session").and_then(|v| v.as_bool()).unwrap_or(false);
        let wait = args.get("wait").and_then(|v| v.as_bool()).unwrap_or(true);
        let timeout = timeout_secs(&args);
        let session_id = text_arg(&args, "session_id");
        let host = text_arg(&args, "host");
        if !wait {
            return self.start_on_open_tab(session_id.as_deref(), host.as_deref(), command, "mcp");
        }
        match self.route(session_id.as_deref(), host.as_deref(), new_session)? {
            Some(id) => self.finish_on_tab(&id, command, timeout, "mcp"),
            None if is_local_host(host.as_deref()) => {
                let id = self.open_local_tab(new_session)?;
                self.finish_on_tab(&id, command, timeout, "mcp")
            }
            None => {
                let host = host.ok_or("缺少 host 或 session_id")?;
                self.run_dialed(&host, command, timeout)
            }
        }
    }

    pub(crate) fn tool_open_terminal(&self, args: Value) -> Result<Value, String> {
        let hub = self.hub()?;
        let new_session = args.get("new_session").and_then(|v| v.as_bool()).unwrap_or(false);
        let cols = args.get("cols").and_then(|v| v.as_u64()).unwrap_or(120) as u32;
        let rows = args.get("rows").and_then(|v| v.as_u64()).unwrap_or(32) as u32;
        let host = text_arg(&args, "host");
        let kind = text_arg(&args, "kind").unwrap_or_default();
        if kind == "local" || is_local_host(host.as_deref()) {
            if !new_session {
                if let Some(id) = self.route(None, Some("local"), false)? {
                    return self.terminal_json(&id);
                }
            }
            let info = hub.open_local(cols, rows)?;
            return self.terminal_json(&info.session_id);
        }
        let host = host.ok_or("缺少 host，或把 kind 设为 local")?;
        if !new_session {
            if let Some(id) = self.route(None, Some(&host), false)? {
                return self.terminal_json(&id);
            }
        }
        let info = hub.open_ssh(&self.hosts, &host, cols, rows)?;
        self.terminal_json(&info.session_id)
    }

    pub(crate) fn tool_close_terminal(&self, args: Value) -> Result<Value, String> {
        let id = text_arg(&args, "session_id").ok_or("缺少 session_id")?;
        self.hub()?.close_session(&id);
        Ok(tool_text(json_text(&json!({"closed": true, "sessionId": id})), false))
    }

    pub(crate) fn tool_read_terminal(&self, args: Value) -> Result<Value, String> {
        let id = self.one_tab(text_arg(&args, "session_id").as_deref(), text_arg(&args, "host").as_deref())?;
        let max_chars = args.get("max_chars").and_then(|v| v.as_u64()).unwrap_or(12_000) as usize;
        let value = self.hub()?.read_screen(&id, max_chars)?;
        Ok(tool_text(json_text(&value), false))
    }

    pub(crate) fn tool_write_terminal(&self, args: Value) -> Result<Value, String> {
        let data = args.get("data").and_then(|v| v.as_str()).or_else(|| args.get("command").and_then(|v| v.as_str())).ok_or("缺少 data")?;
        if data.chars().count() > 8000 || data.contains('\0') {
            return Err("输入为空、过长，或含有空字符".into());
        }
        let submit = args.get("submit").and_then(|v| v.as_bool()).unwrap_or(false);
        let id = self.one_tab(text_arg(&args, "session_id").as_deref(), text_arg(&args, "host").as_deref())?;
        let payload = if submit && !data.ends_with('\n') && !data.ends_with('\r') {
            format!("{data}\r")
        } else {
            data.to_string()
        };
        self.hub()?.write_input(&id, &payload, "mcp")?;
        Ok(tool_text(
            json_text(&json!({"sessionId": id, "submitted": submit, "attached": true})),
            false,
        ))
    }

    pub(crate) fn tool_interrupt_terminal(&self, args: Value) -> Result<Value, String> {
        let id = self.one_tab(text_arg(&args, "session_id").as_deref(), text_arg(&args, "host").as_deref())?;
        self.hub()?.interrupt(&id)?;
        Ok(tool_text(json_text(&json!({"sessionId": id, "interrupted": true})), false))
    }

    pub(crate) fn tool_start_command(&self, args: Value) -> Result<Value, String> {
        let command = required_command(&args)?;
        self.start_on_open_tab(text_arg(&args, "session_id").as_deref(), text_arg(&args, "host").as_deref(), command, "mcp")
    }

    pub(crate) fn tool_poll_command(&self, args: Value) -> Result<Value, String> {
        let job_id = text_arg(&args, "job_id").or_else(|| text_arg(&args, "jobId")).ok_or("缺少 job_id")?;
        let hub = self.hub()?;
        let mut view = hub.poll_job(&job_id)?;
        view.command = self.scrub(&view.command);
        view.stdout = self.scrub(&view.stdout);
        Ok(tool_text(json_text(&serde_json::to_value(&view).unwrap_or(json!({}))), false))
    }

    pub(crate) fn tool_cancel_command(&self, args: Value) -> Result<Value, String> {
        let hub = self.hub()?;
        if let Some(job_id) = text_arg(&args, "job_id").or_else(|| text_arg(&args, "jobId")) {
            let view = hub.cancel_job(&job_id)?;
            return Ok(tool_text(json_text(&serde_json::to_value(&view).unwrap_or(json!({}))), false));
        }
        let id = self.one_tab(text_arg(&args, "session_id").as_deref(), text_arg(&args, "host").as_deref())?;
        hub.interrupt(&id)?;
        Ok(tool_text(json_text(&json!({"sessionId": id, "cancelled": true})), false))
    }

    pub(crate) fn tool_search_history(&self, args: Value) -> Result<Value, String> {
        let query = text_arg(&args, "query").or_else(|| text_arg(&args, "q"));
        let host = text_arg(&args, "host");
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
        let rows = self.history.search(query.as_deref(), host.as_deref(), limit);
        Ok(tool_text(json_text(&json!({"entries": rows})), false))
    }

    pub(crate) fn tool_list_groups(&self) -> Result<Value, String> {
        let hosts = lock(&self.hosts);
        Ok(tool_text(json_text(&json!(hosts.list_groups_sorted())), false))
    }

    pub(crate) fn tool_list_snippets(&self) -> Result<Value, String> {
        let Some(snippets) = &self.snippets else {
            return Ok(tool_text(json_text(&json!({"snippets": []})), false));
        };
        let rows: Vec<Value> = lock(snippets).list().iter().map(|snippet| self.public_snippet(snippet)).collect();
        Ok(tool_text(json_text(&json!(rows)), false))
    }

    pub(crate) fn tool_save_snippet(&self, args: Value) -> Result<Value, String> {
        let snippets = self.snippets.as_ref().ok_or("这个进程没有片段库")?;
        let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        if contains_secret(command) {
            return Err("快捷命令里不能保存私钥、密码或令牌".into());
        }
        let saved = lock(snippets).save_draft(&args).map_err(|err| err.to_string())?;
        Ok(tool_text(json_text(&self.public_snippet(&saved)), false))
    }

    pub(crate) fn tool_run_snippet(&self, args: Value) -> Result<Value, String> {
        let snippets = self.snippets.as_ref().ok_or("这个进程没有片段库")?;
        let query = text_arg(&args, "name").or_else(|| text_arg(&args, "id")).ok_or("缺少片段名称或 id")?;
        let command = {
            let list = lock(snippets);
            let matches: Vec<_> = list
                .list()
                .iter()
                .filter(|snippet| snippet.id == query || snippet.name.eq_ignore_ascii_case(&query))
                .cloned()
                .collect();
            match matches.len() {
                0 => return Err(format!("没有叫「{query}」的快捷命令")),
                1 => matches[0].command.clone(),
                _ => return Err("有多条同名快捷命令，请改用 id".into()),
            }
        };
        let mut args = args;
        if let Some(map) = args.as_object_mut() {
            map.insert("command".into(), json!(command));
        }
        let mut value = self.tool_run_command(args)?;
        if let Some(text) = value.pointer("/content/0/text").and_then(|v| v.as_str()) {
            if let Ok(mut body) = serde_json::from_str::<Value>(text) {
                body["source"] = json!("snippet");
                value["content"][0]["text"] = json!(json_text(&body));
            }
        }
        Ok(value)
    }

    pub(crate) fn tool_list_tunnels(&self) -> Result<Value, String> {
        let Some(rules) = &self.tunnel_rules else {
            return Ok(tool_text(json_text(&json!({"tunnels": []})), false));
        };
        let rules = lock(rules);
        let listed = if let Some(tunnels) = &self.tunnels {
            tunnels.runtimes(rules.list())
        } else {
            rules
                .list()
                .iter()
                .map(|rule| {
                    let mut value = serde_json::to_value(rule).unwrap_or(json!({}));
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert("active".into(), json!(false));
                    }
                    value
                })
                .collect()
        };
        Ok(tool_text(json_text(&json!(listed)), false))
    }

    pub(crate) fn tool_start_tunnel(&self, args: Value) -> Result<Value, String> {
        let tunnels = self.tunnels.as_ref().ok_or("附着到工作台后才能启动隧道。端口转发和交互终端不是同一条 SSH。")?;
        let rules = self.tunnel_rules.as_ref().ok_or("没有隧道规则")?;
        let rule = {
            let rules = lock(rules);
            find_rule(rules.list(), &args)?.clone()
        };
        let chain = {
            let hosts = lock(&self.hosts);
            resolve_targets(&hosts, &rule.host_id)?
        };
        let port = tunnels.start(&rule, &chain)?;
        Ok(tool_text(
            json_text(&json!({"id": rule.id, "name": rule.name, "port": port, "active": true})),
            false,
        ))
    }

    pub(crate) fn tool_stop_tunnel(&self, args: Value) -> Result<Value, String> {
        let tunnels = self.tunnels.as_ref().ok_or("附着到工作台后才能停止隧道")?;
        let rules = self.tunnel_rules.as_ref().ok_or("没有隧道规则")?;
        let rule = {
            let rules = lock(rules);
            find_rule(rules.list(), &args)?.clone()
        };
        tunnels.stop(&rule.id);
        Ok(tool_text(json_text(&json!({"id": rule.id, "active": false})), false))
    }

    pub(crate) fn tool_get_settings(&self) -> Result<Value, String> {
        let Some(settings) = &self.settings else {
            return Ok(tool_text(json_text(&json!({})), false));
        };
        Ok(tool_text(json_text(&json!(lock(settings).get())), false))
    }

    pub(crate) fn tool_set_host_note(&self, args: Value) -> Result<Value, String> {
        let host = text_arg(&args, "host").ok_or("缺少 host")?;
        let notes = match args.get("notes") {
            None => return Err("缺少 notes".into()),
            Some(value) if value.is_null() => None,
            Some(value) => Some(value.as_str().ok_or("notes 需要是文本")?.to_string()),
        };
        let saved = lock(&self.hosts).set_notes(&host, notes).map_err(|err| err.to_string())?;
        Ok(tool_text(json_text(&serde_json::to_value(saved).unwrap_or(json!({}))), false))
    }

    pub(crate) fn tool_list_bindings(&self) -> Result<Value, String> {
        let Some(bindings) = &self.bindings else {
            return Ok(tool_text(json_text(&json!({"bindings": []})), false));
        };
        Ok(tool_text(json_text(&json!({"bindings": bindings.list()})), false))
    }

    pub(crate) fn tool_save_binding(&self, args: Value) -> Result<Value, String> {
        let bindings = self.bindings.as_ref().ok_or("这个进程没有路径绑定")?;
        let host_query = text_arg(&args, "host").ok_or("缺少 host")?;
        let (host_id, alias) = {
            let hosts = lock(&self.hosts);
            let host = hosts
                .list()
                .iter()
                .find(|host| host.id == host_query || host.alias.eq_ignore_ascii_case(&host_query))
                .ok_or_else(|| format!("没有找到主机「{host_query}」"))?;
            (host.id.clone(), host.alias.clone())
        };
        let saved = bindings.save(
            text_arg(&args, "id"),
            args.get("name").and_then(|v| v.as_str()).unwrap_or("project"),
            args.get("localPath").or_else(|| args.get("local_path")).and_then(|v| v.as_str()).unwrap_or(""),
            &host_id,
            &alias,
            args.get("remotePath").or_else(|| args.get("remote_path")).and_then(|v| v.as_str()).unwrap_or(""),
            args.get("note").and_then(|v| v.as_str()).map(str::to_string),
        )?;
        Ok(tool_text(json_text(&serde_json::to_value(saved).unwrap_or(json!({}))), false))
    }

    pub(crate) fn tool_delete_binding(&self, args: Value) -> Result<Value, String> {
        let bindings = self.bindings.as_ref().ok_or("这个进程没有路径绑定")?;
        let id = text_arg(&args, "id").ok_or("缺少 id")?;
        bindings.delete(&id)?;
        Ok(tool_text(json_text(&json!({"deleted": true})), false))
    }

    pub(crate) fn tool_sftp_download(&self, args: Value) -> Result<Value, String> {
        let host = text_arg(&args, "host").ok_or("缺少 host 参数")?;
        let path = text_arg(&args, "path").ok_or("缺少 path 参数")?;
        let max_bytes = args.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(131_072).clamp(1, 262_144) as usize;
        let session = self.connect(&host)?;
        let (bytes, truncated) = read_bytes_limited(&session, &path, max_bytes).map_err(|err| err.to_string())?;
        Ok(tool_text(
            json_text(&json!({
                "path": path,
                "encoding": "base64",
                "content": base64::engine::general_purpose::STANDARD.encode(bytes),
                "truncated": truncated,
                "connection": "new"
            })),
            false,
        ))
    }

    pub(crate) fn tool_sftp_upload(&self, args: Value) -> Result<Value, String> {
        let host = text_arg(&args, "host").ok_or("缺少 host 参数")?;
        let path = text_arg(&args, "path").ok_or("缺少 path 参数")?;
        let encoded = args.get("content_base64").or_else(|| args.get("contentBase64")).and_then(|v| v.as_str()).ok_or("缺少 content_base64")?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| "content_base64 不是合法的 base64".to_string())?;
        if bytes.len() > 524_288 {
            return Err("内容超过 512KB 上限".into());
        }
        if bytes_look_like_key(&bytes) {
            return Err("拒绝写入私钥内容".into());
        }
        let session = self.connect(&host)?;
        sftp::write_bytes(&session, &path, &bytes, |_, _| true).map_err(|err| err.to_string())?;
        Ok(tool_text(json_text(&json!({"path": path, "bytes": bytes.len(), "connection": "new"})), false))
    }

    fn finish_on_tab(&self, session_id: &str, command: &str, timeout: u64, source: &str) -> Result<Value, String> {
        let hub = self.hub()?;
        let started = hub.begin_command(session_id, command, source)?;
        let mut view = hub.wait_job(&started.job_id, Duration::from_secs(timeout))?;
        view.command = self.scrub(&view.command);
        view.stdout = self.scrub(&view.stdout);
        let mut value = serde_json::to_value(&view).unwrap_or(json!({}));
        if let Some(obj) = value.as_object_mut() {
            obj.insert("attached".into(), json!(true));
            obj.insert("stderr".into(), json!(""));
            obj.insert("source".into(), json!(source));
        }
        Ok(tool_text(json_text(&value), false))
    }

    fn start_on_open_tab(&self, session_id: Option<&str>, host: Option<&str>, command: &str, source: &str) -> Result<Value, String> {
        let id = match self.route(session_id, host, false)? {
            Some(id) => id,
            None if is_local_host(host) => self.open_local_tab(false)?,
            None => {
                return Err("没有已打开的终端。先 open_terminal，或让 run_command 在没有标签时另开一条连接并等待。".into());
            }
        };
        let view = self.hub()?.begin_command(&id, command, source)?;
        let mut value = serde_json::to_value(&view).unwrap_or(json!({}));
        if let Some(obj) = value.as_object_mut() {
            obj.insert("attached".into(), json!(true));
            obj.insert("stderr".into(), json!(""));
        }
        Ok(tool_text(json_text(&value), false))
    }

    fn run_dialed(&self, host: &str, command: &str, timeout: u64) -> Result<Value, String> {
        let chain = {
            let hosts = lock(&self.hosts);
            resolve_targets(&hosts, host)?
        };
        let leaf = chain.last().cloned().ok_or_else(|| "跳板机链为空".to_string())?;
        let (session, learned) = crate::ssh::connect_chain(&chain).map_err(|err| err.to_string())?;
        {
            let mut hosts = lock(&self.hosts);
            for (id, fingerprint) in &learned {
                if !fingerprint.is_empty() {
                    let _ = hosts.remember_fingerprint(id, fingerprint);
                }
            }
        }
        session.set_timeout((timeout * 1000) as u32);
        let wrapped = wrap_exec(command);
        let redacted = self.scrub(command);
        let session_id = Uuid::new_v4().to_string();
        match exec_command(&session, &wrapped) {
            Ok(result) => {
                let cwd = parse_exec_cwd(&result.stderr);
                let stderr = strip_exec_cwd(&result.stderr);
                let (stdout, out_cut) = truncate_output(result.stdout, OUTPUT_LIMIT);
                let (stderr, err_cut) = truncate_output(stderr, OUTPUT_LIMIT);
                self.remember_exec(&session_id, host, &leaf.host, &redacted, Some(result.exit_code), false, None);
                self.history.record(HistoryDraft {
                    host_id: Some(leaf.id.clone()),
                    alias: Some(leaf.alias.clone()),
                    session_kind: "exec".into(),
                    session_id: Some(session_id.clone()),
                    command: command.to_string(),
                    cwd: cwd.clone(),
                    exit_code: Some(result.exit_code),
                    ok: Some(result.exit_code == 0),
                    source: "mcp".into(),
                });
                Ok(tool_text(
                    json_text(&json!({
                        "attached": false,
                        "connection": "new",
                        "sessionId": session_id,
                        "alias": leaf.alias,
                        "host": leaf.host,
                        "port": leaf.port,
                        "username": leaf.username,
                        "command": redacted,
                        "cwd": cwd,
                        "exitCode": result.exit_code,
                        "ok": result.exit_code == 0,
                        "timedOut": false,
                        "truncated": out_cut || err_cut,
                        "streams": "separate",
                        "stdout": self.scrub(&stdout),
                        "stderr": self.scrub(&stderr)
                    })),
                    false,
                ))
            }
            Err(err) if is_timeout_text(&err.to_string()) => {
                self.remember_exec(&session_id, host, &leaf.host, &redacted, None, true, Some(err.to_string()));
                self.history.record(HistoryDraft {
                    host_id: Some(leaf.id),
                    alias: Some(leaf.alias),
                    session_kind: "exec".into(),
                    session_id: Some(session_id.clone()),
                    command: command.to_string(),
                    cwd: None,
                    exit_code: None,
                    ok: Some(false),
                    source: "mcp".into(),
                });
                Ok(tool_text(
                    json_text(&json!({
                        "attached": false,
                        "connection": "new",
                        "sessionId": session_id,
                        "command": redacted,
                        "exitCode": null,
                        "timedOut": true,
                        "streams": "separate",
                        "stdout": "",
                        "stderr": ""
                    })),
                    false,
                ))
            }
            Err(err) => {
                self.remember_exec(&session_id, host, &leaf.host, &redacted, None, false, Some(err.to_string()));
                Err(format!("执行错误: {}", self.scrub(&err.to_string())))
            }
        }
    }

    fn remember_exec(
        &self,
        id: &str,
        alias: &str,
        host: &str,
        command: &str,
        exit_code: Option<i32>,
        timed_out: bool,
        error: Option<String>,
    ) {
        lock(&self.sessions).insert(
            id.to_string(),
            SessionRecord {
                id: id.to_string(),
                alias: alias.to_string(),
                host: host.to_string(),
                command: command.to_string(),
                status: if error.is_some() { "error" } else { "closed" }.into(),
                exit_code,
                timed_out: Some(timed_out),
                error: error.map(|text| self.scrub(&text)),
            },
        );
    }

    fn route(&self, session_id: Option<&str>, host: Option<&str>, new_session: bool) -> Result<Option<String>, String> {
        let Some(hub) = &self.terminals else {
            if session_id.is_some() {
                return Err("没有附着到工作台，不能使用已打开的终端".into());
            }
            return Ok(None);
        };
        match route_run(&hub.list_views(), session_id, host, new_session) {
            RunRoute::Attach(id) => Ok(Some(id)),
            RunRoute::Dial => Ok(None),
            RunRoute::Ambiguous(ids) => Err(format!(
                "有多个已打开的终端匹配。请传 session_id，不要另开连接。候选: {}",
                ids.join(", ")
            )),
            RunRoute::NoSuchSession => Err("没有这个 session_id".into()),
        }
    }

    fn one_tab(&self, session_id: Option<&str>, host: Option<&str>) -> Result<String, String> {
        self.route(session_id, host, false)?
            .ok_or_else(|| "没有匹配的已打开终端".to_string())
    }

    fn open_local_tab(&self, new_session: bool) -> Result<String, String> {
        let hub = self.hub()?;
        if !new_session {
            if let Some(id) = self.route(None, Some("local"), false)? {
                return Ok(id);
            }
        }
        Ok(hub.open_local(120, 32)?.session_id)
    }

    fn terminal_json(&self, session_id: &str) -> Result<Value, String> {
        let hub = self.hub()?;
        let view = hub
            .list_views()
            .into_iter()
            .find(|view| view.session_id == session_id)
            .ok_or_else(|| "终端刚打开，但列表里还没有它".to_string())?;
        Ok(tool_text(json_text(&serde_json::to_value(view).unwrap_or(json!({}))), false))
    }

    fn hub(&self) -> Result<&std::sync::Arc<crate::hub::SessionHub>, String> {
        self.terminals.as_ref().ok_or_else(|| "没有附着到工作台。请连工作台的 MCP，或用 tianshell-mcp --attach。".to_string())
    }

    fn public_snippet(&self, snippet: &crate::types::Snippet) -> Value {
        json!({
            "id": snippet.id,
            "name": snippet.name,
            "category": snippet.category,
            "command": redact_secrets(&snippet.command, &[]),
            "description": snippet.description.as_ref().map(|text| redact_secrets(text, &[])),
        })
    }
}

fn required_command(args: &Value) -> Result<&str, String> {
    let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("").trim();
    if command.is_empty() {
        return Err("命令是空的".into());
    }
    if command.chars().count() > 8000 {
        return Err("命令太长了（上限 8000 个字符）".into());
    }
    if command.contains('\0') {
        return Err("命令里不能有空字符".into());
    }
    Ok(command)
}

fn text_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn timeout_secs(args: &Value) -> u64 {
    args.get("timeout_seconds").and_then(|v| v.as_u64()).unwrap_or(30).clamp(1, 120)
}

fn json_text(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".into())
}

fn is_timeout_text(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("timeout") || text.contains("timed out")
}

fn bytes_look_like_key(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    head.contains("PRIVATE KEY") || head.contains("-----BEGIN")
}

fn find_rule<'a>(rules: &'a [crate::types::TunnelRule], args: &Value) -> Result<&'a crate::types::TunnelRule, String> {
    let query = text_arg(args, "id").or_else(|| text_arg(args, "name")).ok_or("缺少隧道 id 或 name")?;
    rules
        .iter()
        .find(|rule| rule.id == query || rule.name.eq_ignore_ascii_case(&query))
        .ok_or_else(|| format!("没有叫「{query}」的隧道规则"))
}
