use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use crate::util::{iso_now, restrict_user_file};

pub const MCP_PROTOCOL: &str = "2025-03-26";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInfo {
    pub pid: u32,
    pub port: u16,
    #[serde(rename = "workbenchUrl")]
    pub workbench_url: String,
    #[serde(rename = "mcpUrl")]
    pub mcp_url: String,
    #[serde(rename = "sseUrl")]
    pub sse_url: String,
    pub token: String,
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    #[serde(rename = "startedAt")]
    pub started_at: String,
}

pub fn runtime_path(data_dir: &Path) -> PathBuf {
    data_dir.join("runtime.json")
}

pub fn write_runtime(data_dir: &Path, info: &RuntimeInfo) -> std::io::Result<()> {
    if let Some(parent) = runtime_path(data_dir).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let path = runtime_path(data_dir);
    let body = serde_json::to_string_pretty(info).map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    std::fs::write(&path, body)?;
    restrict_user_file(&path);
    Ok(())
}

pub fn read_runtime(data_dir: &Path) -> Option<RuntimeInfo> {
    let raw = std::fs::read_to_string(runtime_path(data_dir)).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn remove_runtime(data_dir: &Path, pid: u32) {
    let path = runtime_path(data_dir);
    if let Some(info) = read_runtime(data_dir) {
        if info.pid == pid {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn describe(port: u16, token: &str) -> RuntimeInfo {
    RuntimeInfo {
        pid: std::process::id(),
        port,
        workbench_url: format!("http://127.0.0.1:{port}/?token={token}"),
        mcp_url: format!("http://127.0.0.1:{port}/mcp"),
        sse_url: format!("http://127.0.0.1:{port}/sse"),
        token: token.to_string(),
        protocol_version: MCP_PROTOCOL.to_string(),
        started_at: iso_now(),
    }
}
