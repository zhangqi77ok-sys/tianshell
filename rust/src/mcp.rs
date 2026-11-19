use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::monitor::sample_remote_metrics;
use crate::paths::resolve_data_dir;
use crate::sftp::{list_dir, read_text, write_text};
use crate::ssh::{connect_chain, exec_command, SshTarget};
use crate::store::HostStore;
use crate::vault::load_or_create_master_key;

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
    pub error: Option<String>,
}

pub struct McpHandler {
    hosts: HostStore,
    sessions: Mutex<HashMap<String, SessionRecord>>,
}

impl McpHandler {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let data_dir = resolve_data_dir();
        std::fs::create_dir_all(&data_dir)?;
        let key = load_or_create_master_key(&data_dir.join("vault.key"))?;
        let hosts = HostStore::load(data_dir.join("hosts.json"), key)?;
        Ok(Self {
            hosts,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    pub fn handle_request(&self, req: JsonRpcRequest) -> Option<Value> {
        let id = req.id.clone()?;
        let res = match req.method.as_str() {
            "initialize" => Ok(self.handle_initialize()),
            "tools/list" => Ok(self.handle_tools_list()),
            "tools/call" => self.handle_tools_call(req.params.unwrap_or_default()),
            "notifications/initialized" | "ping" => Ok(json!({})),
            _ => Err(format!("未识别的方法: {}", req.method)),
        };
        match res {
            Ok(result) => Some(json!({ "jsonrpc": "2.0", "id": id, "result": result })),
            Err(msg) => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32603, "message": msg }
            })),
        }
    }

    fn handle_initialize(&self) -> Value {
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "tianshell", "version": "0.2.0" },
            "instructions": "TianShell 保存了用户的 SSH 主机（支持分组和跳板机）。先用 list_hosts 查看主机别名，再调用 run_command / sftp_list / sftp_read / sftp_write / system_metrics 管理服务器。"
        })
    }

    fn handle_tools_list(&self) -> Value {
        json!({
            "tools": [
                {
                    "name": "list_hosts",
                    "description": "列出本机保存的 SSH 主机（含分组与跳板机关联，不含密码私钥）。List saved SSH hosts without secrets.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "run_command",
                    "description": "连接主机（支持 ProxyJump 跳板链）执行命令并返回输出与退出码。Run one command on a saved host.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "host": { "type": "string", "description": "主机 id 或别名" },
                            "command": { "type": "string", "description": "要执行的命令" }
                        },
                        "required": ["host", "command"]
                    }
                },
                {
                    "name": "session_status",
                    "description": "查看当前 MCP 进程里的命令会话状态。Read exec session status for this MCP process.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "session_id": { "type": "string", "description": "可选会话 ID" }
                        }
                    }
                },
                {
                    "name": "sftp_list",
                    "description": "通过 SFTP 列出目标主机指定目录的文件与权限。List directory contents over SFTP.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "host": { "type": "string", "description": "主机 id 或别名" },
                            "path": { "type": "string", "description": "POSIX 远端路径", "default": "/" }
                        },
                        "required": ["host"]
                    }
                },
                {
                    "name": "sftp_read",
                    "description": "通过 SFTP 只读读取配置/日志等 UTF-8 文本文件（上限 256KB）。Read a remote UTF-8 text file.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "host": { "type": "string", "description": "主机 id 或别名" },
                            "path": { "type": "string", "description": "远端文件路径" },
                            "max_bytes": { "type": "integer", "description": "最大读取字节数", "default": 131072 }
                        },
                        "required": ["host", "path"]
                    }
                },
                {
                    "name": "sftp_write",
                    "description": "通过 SFTP 将 UTF-8 文本内容写入远端文件（慎用，会覆盖原文件）。Write UTF-8 text file over SFTP.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "host": { "type": "string", "description": "主机 id 或别名" },
                            "path": { "type": "string", "description": "远端文件路径" },
                            "content": { "type": "string", "description": "文件完整文本内容" }
                        },
                        "required": ["host", "path", "content"]
                    }
                },
                {
                    "name": "system_metrics",
                    "description": "采集目标主机 CPU、内存、Swap、磁盘挂载和 Top 进程状态。Sample server system metrics.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "host": { "type": "string", "description": "主机 id 或别名" }
                        },
                        "required": ["host"]
                    }
                }
            ]
        })
    }

    fn handle_tools_call(&self, params: Value) -> Result<Value, String> {
        let name = params.get("name").and_then(|v| v.as_str()).ok_or("缺少工具名称")?;
        let args = params.get("arguments").cloned().unwrap_or(json!({}));

        match name {
            "list_hosts" => Ok(tool_text(serde_json::to_string_pretty(self.hosts.list()).unwrap_or_default(), false)),
            "session_status" => self.tool_session_status(args.get("session_id").and_then(|v| v.as_str())),
            "run_command" => self.tool_run_command(args),
            "sftp_list" => self.tool_sftp_list(args),
            "sftp_read" => self.tool_sftp_read(args),
            "sftp_write" => self.tool_sftp_write(args),
            "system_metrics" => self.tool_system_metrics(args),
            _ => Err(format!("未知的工具: {}", name)),
        }
    }

    fn resolve_chain(&self, host_query: &str) -> Result<Vec<SshTarget>, String> {
        let target_record = self.hosts.list().iter().find(|h| h.alias == host_query || h.id == host_query).ok_or_else(|| format!("未找到主机: {}", host_query))?;
        let mut hops = Vec::new();
        let mut curr = Some(target_record);
        while let Some(r) = curr {
            let pw = r.secret.as_deref().and_then(|s| self.hosts.open_secret(s).ok());
            let pk = r.passphrase.as_deref().and_then(|s| self.hosts.open_secret(s).ok());
            hops.insert(0, SshTarget {
                host: r.host.clone(),
                port: r.port,
                username: r.username.clone(),
                password: pw,
                private_key: pk,
                passphrase: None,
            });
            curr = r.jump_host_id.as_deref().and_then(|jid| self.hosts.list().iter().find(|h| h.id == jid));
        }
        Ok(hops)
    }

    fn tool_run_command(&self, args: Value) -> Result<Value, String> {
        let host = args.get("host").and_then(|v| v.as_str()).ok_or("缺少 host 参数")?;
        let cmd = args.get("command").and_then(|v| v.as_str()).ok_or("缺少 command 参数")?;
        let chain = self.resolve_chain(host)?;
        let sess = connect_chain(&chain).map_err(|e| format!("连接错误: {}", e))?;
        let res = exec_command(&sess, cmd).map_err(|e| format!("执行错误: {}", e))?;

        let sid = uuid::Uuid::new_v4().to_string();
        if let Ok(mut map) = self.sessions.lock() {
            map.insert(sid.clone(), SessionRecord {
                id: sid.clone(),
                alias: host.to_string(),
                host: chain.last().map(|h| h.host.clone()).unwrap_or_default(),
                command: cmd.to_string(),
                status: "closed".to_string(),
                exit_code: Some(res.exit_code),
                error: None,
            });
        }
        Ok(tool_text(json!({
            "sessionId": sid,
            "exitCode": res.exit_code,
            "stdout": res.stdout,
            "stderr": res.stderr
        }).to_string(), false))
    }

    fn tool_session_status(&self, sid: Option<&str>) -> Result<Value, String> {
        let map = self.sessions.lock().map_err(|_| "锁错误")?;
        if let Some(id) = sid {
            let item = map.get(id).ok_or_else(|| format!("未找到会话 {}", id))?;
            Ok(tool_text(serde_json::to_string_pretty(item).unwrap_or_default(), false))
        } else {
            let list: Vec<&SessionRecord> = map.values().collect();
            Ok(tool_text(serde_json::to_string_pretty(&list).unwrap_or_default(), false))
        }
    }

    fn tool_sftp_list(&self, args: Value) -> Result<Value, String> {
        let host = args.get("host").and_then(|v| v.as_str()).ok_or("缺少 host 参数")?;
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("/");
        let chain = self.resolve_chain(host)?;
        let sess = connect_chain(&chain).map_err(|e| format!("连接错误: {}", e))?;
        let entries = list_dir(&sess, path).map_err(|e| format!("SFTP 列目录错误: {}", e))?;
        Ok(tool_text(serde_json::to_string_pretty(&entries).unwrap_or_default(), false))
    }

    fn tool_sftp_read(&self, args: Value) -> Result<Value, String> {
        let host = args.get("host").and_then(|v| v.as_str()).ok_or("缺少 host 参数")?;
        let path = args.get("path").and_then(|v| v.as_str()).ok_or("缺少 path 参数")?;
        let max_b = args.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(131072) as usize;
        let chain = self.resolve_chain(host)?;
        let sess = connect_chain(&chain).map_err(|e| format!("连接错误: {}", e))?;
        let content = read_text(&sess, path, max_b).map_err(|e| format!("SFTP 读文件错误: {}", e))?;
        Ok(tool_text(content, false))
    }

    fn tool_sftp_write(&self, args: Value) -> Result<Value, String> {
        let host = args.get("host").and_then(|v| v.as_str()).ok_or("缺少 host 参数")?;
        let path = args.get("path").and_then(|v| v.as_str()).ok_or("缺少 path 参数")?;
        let content = args.get("content").and_then(|v| v.as_str()).ok_or("缺少 content 参数")?;
        let chain = self.resolve_chain(host)?;
        let sess = connect_chain(&chain).map_err(|e| format!("连接错误: {}", e))?;
        write_text(&sess, path, content).map_err(|e| format!("SFTP 写文件错误: {}", e))?;
        Ok(tool_text(format!("已成功写入 {}", path), false))
    }

    fn tool_system_metrics(&self, args: Value) -> Result<Value, String> {
        let host = args.get("host").and_then(|v| v.as_str()).ok_or("缺少 host 参数")?;
        let chain = self.resolve_chain(host)?;
        let sess = connect_chain(&chain).map_err(|e| format!("连接错误: {}", e))?;
        let metrics = sample_remote_metrics(&sess).map_err(|e| format!("监控采样错误: {}", e))?;
        Ok(tool_text(serde_json::to_string_pretty(&metrics).unwrap_or_default(), false))
    }
}

fn tool_text(text: String, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    })
}

pub fn run_stdio_loop(handler: McpHandler) -> Result<(), Box<dyn std::error::Error>> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(trimmed) {
            if let Some(resp) = handler.handle_request(req) {
                let out = serde_json::to_string(&resp)?;
                writeln!(stdout, "{}", out)?;
                stdout.flush()?;
            }
        }
    }
    Ok(())
}
