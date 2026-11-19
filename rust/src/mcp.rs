use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::bindings::BindingStore;
use crate::history::{load_token_quietly, CommandHistory};
use crate::hub::SessionHub;
use crate::monitor::sample_remote_metrics;
use crate::paths::resolve_data_dir;
use crate::redact::redact_secrets;
use crate::runtime::MCP_PROTOCOL;
use crate::sftp::{list_dir, mkdir, read_text_limited, write_text};
use crate::ssh::{connect_chain, resolve_targets};
use crate::state::lock;
use crate::store::{HostStore, SettingsStore, SnippetStore, TunnelStore};
use crate::tunnel::TunnelManager;
use crate::vault::load_or_create_master_key;
use crate::VERSION;

const SUPPORTED_PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

pub fn tool_names() -> &'static [&'static str] {
    &[
        "list_hosts",
        "list_groups",
        "list_terminal_sessions",
        "open_terminal",
        "close_terminal",
        "read_terminal",
        "write_terminal",
        "interrupt_terminal",
        "run_command",
        "start_command",
        "poll_command",
        "cancel_command",
        "session_status",
        "search_history",
        "list_snippets",
        "save_snippet",
        "run_snippet",
        "list_tunnels",
        "start_tunnel",
        "stop_tunnel",
        "get_settings",
        "list_bindings",
        "save_binding",
        "delete_binding",
        "set_host_note",
        "sftp_list",
        "sftp_mkdir",
        "sftp_read",
        "sftp_write",
        "sftp_download",
        "sftp_upload",
        "system_metrics",
    ]
}

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

pub struct WorkbenchMcp {
    pub hosts: Arc<Mutex<HostStore>>,
    pub snippets: Arc<Mutex<SnippetStore>>,
    pub tunnel_rules: Arc<Mutex<TunnelStore>>,
    pub tunnels: Arc<TunnelManager>,
    pub settings: Arc<Mutex<SettingsStore>>,
    pub bindings: Arc<BindingStore>,
    pub hub: Arc<SessionHub>,
    pub history: Arc<CommandHistory>,
    pub token: String,
}

pub struct McpHandler {
    pub(crate) hosts: Arc<Mutex<HostStore>>,
    pub(crate) snippets: Option<Arc<Mutex<SnippetStore>>>,
    pub(crate) tunnel_rules: Option<Arc<Mutex<TunnelStore>>>,
    pub(crate) tunnels: Option<Arc<TunnelManager>>,
    pub(crate) settings: Option<Arc<Mutex<SettingsStore>>>,
    pub(crate) bindings: Option<Arc<BindingStore>>,
    pub(crate) terminals: Option<Arc<SessionHub>>,
    pub(crate) history: Arc<CommandHistory>,
    pub(crate) token: String,
    pub(crate) sessions: Mutex<HashMap<String, SessionRecord>>,
}

impl McpHandler {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let data_dir = resolve_data_dir();
        std::fs::create_dir_all(&data_dir)?;
        let key = load_or_create_master_key(&data_dir.join("vault.key"))?;
        let hosts = HostStore::load(data_dir.join("hosts.json"), key)?;
        let snippets = SnippetStore::load(data_dir.join("snippets.json"))?;
        let tunnel_rules = TunnelStore::load(data_dir.join("tunnels.json"))?;
        let settings = SettingsStore::load(data_dir.join("settings.json"))?;
        let bindings = BindingStore::open(data_dir.join("bindings.json"))?;
        let history = CommandHistory::open(data_dir.join("command-history.json"))?;
        let token = load_token_quietly(&data_dir);
        history.set_token(&token);
        Ok(Self {
            hosts: Arc::new(Mutex::new(hosts)),
            snippets: Some(Arc::new(Mutex::new(snippets))),
            tunnel_rules: Some(Arc::new(Mutex::new(tunnel_rules))),
            tunnels: None,
            settings: Some(Arc::new(Mutex::new(settings))),
            bindings: Some(Arc::new(bindings)),
            terminals: None,
            history: Arc::new(history),
            token,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    pub fn from_hosts(hosts: Arc<Mutex<HostStore>>) -> Self {
        Self::with_terminals(hosts, None)
    }

    pub fn with_terminals(hosts: Arc<Mutex<HostStore>>, terminals: Option<Arc<SessionHub>>) -> Self {
        Self {
            hosts,
            snippets: None,
            tunnel_rules: None,
            tunnels: None,
            settings: None,
            bindings: None,
            terminals,
            history: Arc::new(CommandHistory::memory()),
            token: String::new(),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn for_workbench(parts: WorkbenchMcp) -> Self {
        parts.history.set_token(&parts.token);
        Self {
            hosts: parts.hosts,
            snippets: Some(parts.snippets),
            tunnel_rules: Some(parts.tunnel_rules),
            tunnels: Some(parts.tunnels),
            settings: Some(parts.settings),
            bindings: Some(parts.bindings),
            terminals: Some(parts.hub),
            history: parts.history,
            token: parts.token,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn handle_request(&self, req: JsonRpcRequest) -> Option<Value> {
        let id = req.id.clone()?;
        let result = match req.method.as_str() {
            "initialize" => Ok(self.handle_initialize(req.params.as_ref())),
            "tools/list" => Ok(self.handle_tools_list()),
            "tools/call" => {
                let value = match self.handle_tools_call(req.params.unwrap_or_else(|| json!({}))) {
                    Ok(value) => value,
                    Err(message) => tool_text(self.scrub(&message), true),
                };
                Ok(self.scrub_tool(value))
            }
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
            "instructions": "TianShell 是会话和记忆中枢。先 list_terminal_sessions 或 open_terminal，用 read_terminal 看屏幕，用 write_terminal / run_command 打进已经打开的标签（用户能在窗口里看见）。只有没有匹配标签，或 new_session 为 true 时，run_command 才另开一条连接。长命令用 start_command、poll_command、cancel_command，避免一直阻塞。文件工具会另开一条 SSH，不会占用交互终端那条线程。主机、片段、隧道、历史和笔记都不含私钥、密码或令牌。不要猜测密码。"
        })
    }

    fn handle_tools_list(&self) -> Value {
        json!({
            "tools": [
                tool_def("list_hosts", "列出本机保存的 SSH 主机（含分组与跳板机关联，不含密码私钥）。List saved SSH hosts without secrets.", json!({"type": "object", "properties": {}}), json!([])),
                tool_def("list_terminal_sessions", "列出工作台里已经打开的交互终端。run_command 会优先打进这里。List open interactive tabs, including cwd and last exit.", json!({"type": "object", "properties": {}}), json!([])),
                tool_def("run_command", "优先把命令打进已经打开的匹配终端并等待退出码。没有标签，或 new_session 为 true 时，才另开一条 SSH。Prefer an open tab; dial only when none matches or new_session is true.", json!({
                    "type": "object",
                    "properties": {
                        "host": {"type": "string", "description": "主机 id、别名，或 local"},
                        "command": {"type": "string", "description": "要执行的命令"},
                        "session_id": {"type": "string", "description": "指定已经打开的终端"},
                        "timeout_seconds": {"type": "integer", "description": "超时秒数，默认 30，最大 120"},
                        "new_session": {"type": "boolean", "description": "为 true 时不复用已打开的终端"},
                        "wait": {"type": "boolean", "description": "为 false 时立刻返回 jobId，用 poll_command 接着看"}
                    },
                    "required": ["command"]
                }), json!(["command"])),
                tool_def("open_terminal", "按主机别名打开交互终端；已有匹配标签时直接返回它。Open or reuse an interactive tab.", json!({"type":"object","properties":{"host":{"type":"string"},"kind":{"type":"string","description":"ssh 或 local"},"new_session":{"type":"boolean"},"cols":{"type":"integer"},"rows":{"type":"integer"}}}), json!([])),
                tool_def("close_terminal", "关闭一个交互终端。Close an interactive tab.", json!({"type":"object","properties":{"session_id":{"type":"string"}},"required":["session_id"]}), json!(["session_id"])),
                tool_def("read_terminal", "读取已打开终端的近期屏幕、cwd 和上次退出码。Read the live screen of an open tab.", json!({"type":"object","properties":{"session_id":{"type":"string"},"host":{"type":"string"},"max_chars":{"type":"integer"}}}), json!([])),
                tool_def("write_terminal", "向已打开的终端发送按键。submit 为 true 时补上回车。Type into an open tab.", json!({"type":"object","properties":{"session_id":{"type":"string"},"host":{"type":"string"},"data":{"type":"string"},"submit":{"type":"boolean"}}}), json!([])),
                tool_def("interrupt_terminal", "向已打开的终端发送 Ctrl+C。Interrupt the foreground command in a tab.", json!({"type":"object","properties":{"session_id":{"type":"string"},"host":{"type":"string"}}}), json!([])),
                tool_def("start_command", "把命令打进已打开的终端并立刻返回 jobId，不阻塞。Start a command on an open tab without waiting.", json!({"type":"object","properties":{"session_id":{"type":"string"},"host":{"type":"string"},"command":{"type":"string"}},"required":["command"]}), json!(["command"])),
                tool_def("poll_command", "查看 start_command 或 run_command 的 jobId。Poll a command started on an open tab.", json!({"type":"object","properties":{"job_id":{"type":"string"}},"required":["job_id"]}), json!(["job_id"])),
                tool_def("cancel_command", "取消一条还在跑的命令（Ctrl+C）。Cancel a running tab command.", json!({"type":"object","properties":{"job_id":{"type":"string"},"session_id":{"type":"string"}}}), json!([])),
                tool_def("search_history", "搜索数据目录里的命令历史，最近的在前。Search persistent command history.", json!({"type":"object","properties":{"query":{"type":"string"},"host":{"type":"string"},"limit":{"type":"integer"}}}), json!([])),
                tool_def("list_groups", "列出主机分组，不含凭据。List host groups.", json!({"type":"object","properties":{}}), json!([])),
                tool_def("list_snippets", "列出快捷命令。秘密样式的参数会被抹掉。List snippets without secret values.", json!({"type":"object","properties":{}}), json!([])),
                tool_def("save_snippet", "保存一条快捷命令。拒绝私钥、密码和令牌。Save a snippet.", json!({"type":"object","properties":{"name":{"type":"string"},"command":{"type":"string"},"category":{"type":"string"},"description":{"type":"string"}},"required":["name","command"]}), json!(["name","command"])),
                tool_def("run_snippet", "按名称或 id 执行已保存的快捷命令，优先打进已打开的终端。Run a saved snippet.", json!({"type":"object","properties":{"name":{"type":"string"},"id":{"type":"string"},"host":{"type":"string"},"session_id":{"type":"string"},"timeout_seconds":{"type":"integer"},"new_session":{"type":"boolean"}}}), json!([])),
                tool_def("list_tunnels", "列出端口转发规则和是否在跑。不含凭据。List tunnel rules.", json!({"type":"object","properties":{}}), json!([])),
                tool_def("start_tunnel", "按规则 id 或名称启动已保存的端口转发。这条连接独立于交互终端。Start a saved port forward.", json!({"type":"object","properties":{"id":{"type":"string"},"name":{"type":"string"}}}), json!([])),
                tool_def("stop_tunnel", "停止一条端口转发。Stop a port forward.", json!({"type":"object","properties":{"id":{"type":"string"},"name":{"type":"string"}}}), json!([])),
                tool_def("get_settings", "读取非机密的工作台设置。Read non-secret settings.", json!({"type":"object","properties":{}}), json!([])),
                tool_def("list_bindings", "列出本地项目路径和远端目录的绑定。List project path bindings.", json!({"type":"object","properties":{}}), json!([])),
                tool_def("save_binding", "记住本地路径对应哪台主机的哪个远端目录。Save a project path binding.", json!({"type":"object","properties":{"name":{"type":"string"},"localPath":{"type":"string"},"host":{"type":"string"},"remotePath":{"type":"string"},"note":{"type":"string"},"id":{"type":"string"}},"required":["localPath","host","remotePath"]}), json!(["localPath","host","remotePath"])),
                tool_def("delete_binding", "删除一条路径绑定。Delete a project path binding.", json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}), json!(["id"])),
                tool_def("set_host_note", "写主机备注（端口、服务名、注意事项）。拒绝私钥、密码和令牌。Set a host note without secrets.", json!({"type":"object","properties":{"host":{"type":"string"},"notes":{"type":"string"}},"required":["host"]}), json!(["host"])),
                tool_def("session_status", "查看 run_command 留下的命令记录。不含界面里的交互终端，那些用 list_terminal_sessions。Read exec records for this MCP process.", json!({
                    "type": "object",
                    "properties": {"session_id": {"type": "string", "description": "可选会话 ID"}}
                }), json!([])),
                tool_def("sftp_list", "另开一条 SSH 用 SFTP 列目录，不占用交互终端那条线程。List a directory over a separate SSH login.", json!({
                    "type": "object",
                    "properties": {
                        "host": {"type": "string", "description": "主机 id 或别名"},
                        "path": {"type": "string", "description": "POSIX 远端路径"}
                    },
                    "required": ["host"]
                }), json!(["host"])),
                tool_def("sftp_mkdir", "通过 SFTP 新建远端目录。Create a remote directory over SFTP.", json!({
                    "type": "object",
                    "properties": {
                        "host": {"type": "string", "description": "主机 id 或别名"},
                        "path": {"type": "string", "description": "要创建的 POSIX 目录"}
                    },
                    "required": ["host", "path"]
                }), json!(["host", "path"])),
                tool_def("sftp_read", "通过 SFTP 只读读取配置/日志等 UTF-8 文本文件（上限 256KB）。Read a remote UTF-8 text file.", json!({
                    "type": "object",
                    "properties": {
                        "host": {"type": "string", "description": "主机 id 或别名"},
                        "path": {"type": "string", "description": "远端文件路径"},
                        "max_bytes": {"type": "integer", "description": "最大读取字节数，默认 131072，最大 262144"}
                    },
                    "required": ["host", "path"]
                }), json!(["host", "path"])),
                tool_def("sftp_write", "另开一条 SSH，把 UTF-8 文本写入远端文件（覆盖，上限 512KB）。不占用交互终端。Write a UTF-8 file over a separate SSH login.", json!({
                    "type": "object",
                    "properties": {
                        "host": {"type": "string", "description": "主机 id 或别名"},
                        "path": {"type": "string", "description": "远端文件路径"},
                        "content": {"type": "string", "description": "完整文件内容（上限 512KB）"}
                    },
                    "required": ["host", "path", "content"]
                }), json!(["host", "path", "content"])),
                tool_def("sftp_download", "另开一条 SSH 下载小文件，返回 base64，上限 256KB。拒绝私钥。Download a small remote file without using the interactive tab.", json!({"type":"object","properties":{"host":{"type":"string"},"path":{"type":"string"},"max_bytes":{"type":"integer"}},"required":["host","path"]}), json!(["host","path"])),
                tool_def("sftp_upload", "另开一条 SSH 上传 base64 小文件，上限 512KB。拒绝私钥。Upload a small file without using the interactive tab.", json!({"type":"object","properties":{"host":{"type":"string"},"path":{"type":"string"},"content_base64":{"type":"string"}},"required":["host","path","content_base64"]}), json!(["host","path","content_base64"])),
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
            "list_hosts" => self.tool_list_hosts(),
            "list_groups" => self.tool_list_groups(),
            "list_terminal_sessions" => self.tool_terminal_sessions(),
            "open_terminal" => self.tool_open_terminal(args),
            "close_terminal" => self.tool_close_terminal(args),
            "read_terminal" => self.tool_read_terminal(args),
            "write_terminal" => self.tool_write_terminal(args),
            "interrupt_terminal" => self.tool_interrupt_terminal(args),
            "run_command" => self.tool_run_command(args),
            "start_command" => self.tool_start_command(args),
            "poll_command" => self.tool_poll_command(args),
            "cancel_command" => self.tool_cancel_command(args),
            "session_status" => self.tool_session_status(args.get("session_id").and_then(|v| v.as_str())),
            "search_history" => self.tool_search_history(args),
            "list_snippets" => self.tool_list_snippets(),
            "save_snippet" => self.tool_save_snippet(args),
            "run_snippet" => self.tool_run_snippet(args),
            "list_tunnels" => self.tool_list_tunnels(),
            "start_tunnel" => self.tool_start_tunnel(args),
            "stop_tunnel" => self.tool_stop_tunnel(args),
            "get_settings" => self.tool_get_settings(),
            "list_bindings" => self.tool_list_bindings(),
            "save_binding" => self.tool_save_binding(args),
            "delete_binding" => self.tool_delete_binding(args),
            "set_host_note" => self.tool_set_host_note(args),
            "sftp_list" => self.tool_sftp_list(args),
            "sftp_mkdir" => self.tool_sftp_mkdir(args),
            "sftp_read" => self.tool_sftp_read(args),
            "sftp_write" => self.tool_sftp_write(args),
            "sftp_download" => self.tool_sftp_download(args),
            "sftp_upload" => self.tool_sftp_upload(args),
            "system_metrics" => self.tool_system_metrics(args),
            _ => Err(format!("未知的工具: {name}")),
        }
    }

    pub(crate) fn tool_list_hosts(&self) -> Result<Value, String> {
        let hosts = lock(&self.hosts);
        let items: Vec<Value> = hosts
            .list_public()
            .into_iter()
            .map(|host| {
                let mut value = serde_json::to_value(&host).unwrap_or(json!({}));
                if let Some(notes) = value.get("notes").and_then(|item| item.as_str()) {
                    value["notes"] = json!(self.scrub(notes));
                }
                value
            })
            .collect();
        Ok(tool_text(serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".into()), false))
    }

    pub(crate) fn scrub(&self, text: &str) -> String {
        let token = self.token.trim();
        let extra: Vec<&str> = if token.len() >= 8 { vec![token] } else { Vec::new() };
        redact_secrets(text, &extra)
    }

    fn scrub_tool(&self, mut value: Value) -> Value {
        if let Some(text) = value.pointer("/content/0/text").and_then(|item| item.as_str()) {
            let clean = self.scrub(text);
            if let Some(slot) = value.pointer_mut("/content/0/text") {
                *slot = json!(clean);
            }
        }
        value
    }

    pub(crate) fn connect(&self, host_query: &str) -> Result<ssh2::Session, String> {
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

    fn tool_terminal_sessions(&self) -> Result<Value, String> {
        let Some(hub) = &self.terminals else {
            return Ok(tool_text(
                serde_json::to_string_pretty(&json!({
                    "attached": false,
                    "sessions": [],
                    "note": "这个 MCP 进程没有附着到工作台，看不到界面里的交互终端。请连接工作台的 Streamable HTTP，或用 tianshell-mcp --attach。"
                }))
                .unwrap_or_default(),
                false,
            ));
        };
        Ok(tool_text(
            serde_json::to_string_pretty(&json!({
                "attached": true,
                "sessions": hub.list_views(),
                "note": "这些是界面里的交互终端。run_command 会优先把命令打进匹配的标签。文件工具另开连接，避免堵住这个终端。"
            }))
            .unwrap_or_default(),
            false,
        ))
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

    fn tool_sftp_mkdir(&self, args: Value) -> Result<Value, String> {
        let host = args.get("host").and_then(|v| v.as_str()).ok_or("缺少 host 参数")?;
        let path = args.get("path").and_then(|v| v.as_str()).ok_or("缺少 path 参数")?;
        if path.is_empty() || path == "/" {
            return Err("不会创建空路径或根目录".into());
        }
        let session = self.connect(host)?;
        mkdir(&session, path).map_err(|err| format!("SFTP 建目录错误: {err}"))?;
        Ok(tool_text(format!("已创建目录 {path}"), false))
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

pub(crate) fn tool_text(text: String, is_error: bool) -> Value {
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error
    })
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
