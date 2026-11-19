use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;
use crate::monitor::sample_remote_metrics;
use crate::paths::resolve_data_dir;
use crate::runtime::MCP_PROTOCOL;
use crate::sftp::{list_dir, read_text_limited, write_text};
use crate::ssh::{connect_chain, exec_command, resolve_targets, truncate_output};
use crate::state::lock;
use crate::store::HostStore;
use crate::vault::load_or_create_master_key;
use crate::VERSION;

const OUTPUT_LIMIT: usize = 200_000;
const SUPPORTED_PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub alias: String,
    pub host: String,
    pub command: String,
    pub status: String,
    #[serde(rename = "exitCode")]
    pub exit_code: Option<i32>,
    #[serde(rename = "timedOut", skip_serializing_if = "Option::is_none")]
    pub timed_out: Option<bool>,
    pub error: Option<String>,
}

pub struct McpHandler {
    hosts: Arc<Mutex<HostStore>>,
    sessions: Mutex<HashMap<String, SessionRecord>>,
}

impl McpHandler {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let data_dir = resolve_data_dir();
        std::fs::create_dir_all(&data_dir)?;
        let key = load_or_create_master_key(&data_dir.join("vault.key"))?;
        let hosts = HostStore::load(data_dir.join("hosts.json"), key)?;
        Ok(Self::from_hosts(Arc::new(Mutex::new(hosts))))
    }

    pub fn from_hosts(hosts: Arc<Mutex<HostStore>>) -> Self {
        Self {
            hosts,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn handle_request(&self, req: JsonRpcRequest) -> Option<Value> {
        let id = req.id.clone()?;
        let result = match req.method.as_str() {
            "initialize" => Ok(self.handle_initialize(req.params.as_ref())),
            "tools/list" => Ok(self.handle_tools_list()),
            "tools/call" => match self.handle_tools_call(req.params.unwrap_or_else(|| json!({}))) {
                Ok(value) => Ok(value),
                Err(message) => Ok(tool_text(message, true)),
            },
            "ping" | "logging/setLevel" | "notifications/initialized" => Ok(json!({})),
            "resources/list" => Ok(json!({"resources": []})),
            "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
            "prompts/list" => Ok(json!({"prompts": []})),
            _ => Err(format!("未识别的方法: {}", req.method)),
        };
        match result {
            Ok(result) => Some(json!({"jsonrpc": "2.0", "id": id, "result": result})),
            Err(message) => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32603, "message": message}
            })),
        }
    }

    fn handle_initialize(&self, params: Option<&Value>) -> Value {
        let requested = params.and_then(|value| value.get("protocolVersion")).and_then(|v| v.as_str()).unwrap_or("");
        let protocol = if SUPPORTED_PROTOCOLS.contains(&requested) {
            requested
        } else {
            MCP_PROTOCOL
        };
        json!({
            "protocolVersion": protocol,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "tianshell", "version": VERSION},
            "instructions": "TianShell 保存了用户的 SSH 主机（支持分组和跳板机）。工作台启动时会在 127.0.0.1 上同时提供这套 MCP。先用 list_hosts 查看主机别名，再调用 run_command / sftp_list / sftp_read / sftp_write / system_metrics。不要猜测密码。"
        })
    }

    fn handle_tools_list(&self) -> Value {
        json!({
            "tools": [
                tool_def("list_hosts", "列出本机保存的 SSH 主机（含分组与跳板机关联，不含密码私钥）。List saved SSH hosts without secrets.", json!({"type": "object", "properties": {}}), json!([])),
                tool_def("run_command", "连接主机（支持 ProxyJump 跳板链）执行命令并返回输出与退出码。Run one command on a saved host.", json!({
                    "type": "object",
                    "properties": {
                        "host": {"type": "string", "description": "主机 id 或别名"},
                        "command": {"type": "string", "description": "要执行的命令"},
                        "timeout_seconds": {"type": "integer", "description": "超时秒数，默认 30，最大 120"}
                    },
                    "required": ["host", "command"]
                }), json!(["host", "command"])),
                tool_def("session_status", "查看当前 MCP 进程里的命令会话状态。Read exec session status for this MCP process.", json!({
                    "type": "object",
                    "properties": {"session_id": {"type": "string", "description": "可选会话 ID"}}
                }), json!([])),
                tool_def("sftp_list", "通过 SFTP 列出目标主机指定目录的文件与权限。List directory contents over SFTP.", json!({
                    "type": "object",
                    "properties": {
                        "host": {"type": "string", "description": "主机 id 或别名"},
                        "path": {"type": "string", "description": "POSIX 远端路径"}
                    },
                    "required": ["host"]
                }), json!(["host"])),
                tool_def("sftp_read", "通过 SFTP 只读读取配置/日志等 UTF-8 文本文件（上限 256KB）。Read a remote UTF-8 text file.", json!({
                    "type": "object",
                    "properties": {
                        "host": {"type": "string", "description": "主机 id 或别名"},
                        "path": {"type": "string", "description": "远端文件路径"},
                        "max_bytes": {"type": "integer", "description": "最大读取字节数，默认 131072，最大 262144"}
                    },
                    "required": ["host", "path"]
                }), json!(["host", "path"])),
                tool_def("sftp_write", "通过 SFTP 将 UTF-8 文本内容写入远端文件（慎用，会覆盖原文件，上限 512KB）。Write UTF-8 text file over SFTP.", json!({
                    "type": "object",
                    "properties": {
                        "host": {"type": "string", "description": "主机 id 或别名"},
                        "path": {"type": "string", "description": "远端文件路径"},
                        "content": {"type": "string", "description": "完整文件内容（上限 512KB）"}
                    },
                    "required": ["host", "path", "content"]
                }), json!(["host", "path", "content"])),
                tool_def("system_metrics", "采集目标主机 CPU、内存、Swap、磁盘挂载、网卡和 Top 进程状态。Sample server system metrics.", json!({
                    "type": "object",
                    "properties": {"host": {"type": "string", "description": "主机 id 或别名"}},
                    "required": ["host"]
                }), json!(["host"]))
            ]
        })
    }

    fn handle_tools_call(&self, params: Value) -> Result<Value, String> {
        let name = params.get("name").and_then(|v| v.as_str()).ok_or("缺少工具名称")?;
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        match name {
            "list_hosts" => {
                let hosts = lock(&self.hosts);
                let text = serde_json::to_string_pretty(&hosts.list_public()).unwrap_or_else(|_| "[]".into());
                Ok(tool_text(text, false))
            }
            "session_status" => self.tool_session_status(args.get("session_id").and_then(|v| v.as_str())),
            "run_command" => self.tool_run_command(args),
            "sftp_list" => self.tool_sftp_list(args),
            "sftp_read" => self.tool_sftp_read(args),
            "sftp_write" => self.tool_sftp_write(args),
            "system_metrics" => self.tool_system_metrics(args),
            _ => Err(format!("未知的工具: {name}")),
        }
    }

    fn connect(&self, host_query: &str) -> Result<ssh2::Session, String> {
        let chain = {
            let hosts = lock(&self.hosts);
            resolve_targets(&hosts, host_query)?
        };
        let (session, learned) = connect_chain(&chain).map_err(|err| err.to_string())?;
        {
            let mut hosts = lock(&self.hosts);
            for (id, fingerprint) in learned {
                if !fingerprint.is_empty() {
                    let _ = hosts.remember_fingerprint(&id, &fingerprint);
                }
            }
        }
        Ok(session)
    }

    fn tool_run_command(&self, args: Value) -> Result<Value, String> {
        let host = args.get("host").and_then(|v| v.as_str()).ok_or("缺少 host 参数")?;
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
        let timeout = args.get("timeout_seconds").and_then(|v| v.as_u64()).unwrap_or(30).clamp(1, 120);
        let chain = {
            let hosts = lock(&self.hosts);
            resolve_targets(&hosts, host)?
        };
        let leaf = chain.last().cloned().ok_or_else(|| "跳板机链为空".to_string())?;
        let (session, learned) = connect_chain(&chain).map_err(|err| err.to_string())?;
        {
            let mut hosts = lock(&self.hosts);
            for (id, fingerprint) in &learned {
                if !fingerprint.is_empty() {
                    let _ = hosts.remember_fingerprint(id, fingerprint);
                }
            }
        }
        session.set_timeout((timeout * 1000) as u32);
        let session_id = Uuid::new_v4().to_string();
        match exec_command(&session, command) {
            Ok(result) => {
                let (stdout, out_cut) = truncate_output(result.stdout, OUTPUT_LIMIT);
                let (stderr, err_cut) = truncate_output(result.stderr, OUTPUT_LIMIT);
                lock(&self.sessions).insert(
                    session_id.clone(),
                    SessionRecord {
                        id: session_id.clone(),
                        alias: host.to_string(),
                        host: leaf.host.clone(),
                        command: command.to_string(),
                        status: "closed".into(),
                        exit_code: Some(result.exit_code),
                        timed_out: Some(false),
                        error: None,
                    },
                );
                Ok(tool_text(
                    serde_json::to_string_pretty(&json!({
                        "sessionId": session_id,
                        "alias": host,
                        "host": leaf.host,
                        "port": leaf.port,
                        "username": leaf.username,
                        "command": command,
                        "exitCode": result.exit_code,
                        "timedOut": false,
                        "truncated": out_cut || err_cut,
                        "stdout": stdout,
                        "stderr": stderr
                    }))
                    .unwrap_or_default(),
                    false,
                ))
            }
            Err(err) if is_timeout(&err) => {
                lock(&self.sessions).insert(
                    session_id.clone(),
                    SessionRecord {
                        id: session_id.clone(),
                        alias: host.to_string(),
                        host: leaf.host,
                        command: command.to_string(),
                        status: "closed".into(),
                        exit_code: None,
                        timed_out: Some(true),
                        error: Some(err.to_string()),
                    },
                );
                Ok(tool_text(
                    serde_json::to_string_pretty(&json!({
                        "sessionId": session_id,
                        "alias": host,
                        "command": command,
                        "exitCode": null,
                        "timedOut": true,
                        "truncated": false,
                        "stdout": "",
                        "stderr": ""
                    }))
                    .unwrap_or_default(),
                    false,
                ))
            }
            Err(err) => {
                lock(&self.sessions).insert(
                    session_id.clone(),
                    SessionRecord {
                        id: session_id,
                        alias: host.to_string(),
                        host: leaf.host,
                        command: command.to_string(),
                        status: "error".into(),
                        exit_code: None,
                        timed_out: None,
                        error: Some(err.to_string()),
                    },
                );
                Err(format!("执行错误: {err}"))
            }
        }
    }

    fn tool_session_status(&self, sid: Option<&str>) -> Result<Value, String> {
        let map = lock(&self.sessions);
        if let Some(id) = sid {
            let item = map.get(id).ok_or_else(|| format!("没有编号为 {id} 的会话"))?;
            Ok(tool_text(serde_json::to_string_pretty(item).unwrap_or_default(), false))
        } else {
            let list: Vec<&SessionRecord> = map.values().collect();
            Ok(tool_text(serde_json::to_string_pretty(&list).unwrap_or_default(), false))
        }
    }

    fn tool_sftp_list(&self, args: Value) -> Result<Value, String> {
        let host = args.get("host").and_then(|v| v.as_str()).ok_or("缺少 host 参数")?;
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("/");
        let session = self.connect(host)?;
        let entries = list_dir(&session, path).map_err(|err| format!("SFTP 列目录错误: {err}"))?;
        Ok(tool_text(serde_json::to_string_pretty(&entries).unwrap_or_default(), false))
    }

    fn tool_sftp_read(&self, args: Value) -> Result<Value, String> {
        let host = args.get("host").and_then(|v| v.as_str()).ok_or("缺少 host 参数")?;
        let path = args.get("path").and_then(|v| v.as_str()).ok_or("缺少 path 参数")?;
        let max_bytes = args.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(131_072).clamp(256, 262_144) as usize;
        let session = self.connect(host)?;
        let text = read_text_limited(&session, path, max_bytes).map_err(|err| format!("SFTP 读文件错误: {err}"))?;
        Ok(tool_text(
            serde_json::to_string_pretty(&json!({"content": text.content, "truncated": text.truncated})).unwrap_or_default(),
            false,
        ))
    }

    fn tool_sftp_write(&self, args: Value) -> Result<Value, String> {
        let host = args.get("host").and_then(|v| v.as_str()).ok_or("缺少 host 参数")?;
        let path = args.get("path").and_then(|v| v.as_str()).ok_or("缺少 path 参数")?;
        let content = args.get("content").and_then(|v| v.as_str()).ok_or("缺少 content 参数")?;
        if content.len() > 524_288 {
            return Err("内容超过 512KB 上限，请改用工作台上传大文件。".into());
        }
        let session = self.connect(host)?;
        write_text(&session, path, content).map_err(|err| format!("SFTP 写文件错误: {err}"))?;
        Ok(tool_text(format!("已成功写入 {path}"), false))
    }

    fn tool_system_metrics(&self, args: Value) -> Result<Value, String> {
        let host = args.get("host").and_then(|v| v.as_str()).ok_or("缺少 host 参数")?;
        let session = self.connect(host)?;
        let metrics = sample_remote_metrics(&session).map_err(|err| format!("监控采样错误: {err}"))?;
        Ok(tool_text(serde_json::to_string_pretty(&metrics).unwrap_or_default(), false))
    }
}

fn tool_def(name: &str, description: &str, schema: Value, _required: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": schema
    })
}

fn tool_text(text: String, is_error: bool) -> Value {
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error
    })
}

fn is_timeout(err: &crate::ssh::SshError) -> bool {
    let text = err.to_string().to_ascii_lowercase();
    text.contains("timeout") || text.contains("timed out")
}

pub fn run_stdio_loop(handler: McpHandler) -> Result<(), Box<dyn std::error::Error>> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(item) => item,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req = match serde_json::from_str::<JsonRpcRequest>(trimmed) {
            Ok(req) => req,
            Err(err) => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": format!("无法解析 JSON-RPC: {err}")}
                });
                writeln!(stdout, "{}", response)?;
                stdout.flush()?;
                continue;
            }
        };
        if let Some(resp) = handler.handle_request(req) {
            writeln!(stdout, "{}", resp)?;
            stdout.flush()?;
        }
    }
    Ok(())
}
