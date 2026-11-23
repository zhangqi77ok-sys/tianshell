//! 把工作台已经展示的 Cursor MCP 配置写进用户级 `~/.cursor/mcp.json`。
//! 只替换 `mcpServers.tianshell`。文件不是合法 JSON，或其他服务器那一段不是对象时，整份文件不动。

use std::fs;
use std::path::{Path, PathBuf};
use serde_json::{json, Value};
use crate::util::write_private_atomic;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMcpAction {
    Created,
    Updated,
    Unchanged,
}

impl CursorMcpAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Unchanged => "unchanged",
        }
    }
}

pub fn cursor_user_mcp_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "找不到用户主目录，没有写入 Cursor 配置".to_string())?;
    Ok(home.join(".cursor").join("mcp.json"))
}

pub fn tiancode_user_mcp_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "找不到用户主目录，没有写入 TianCode 配置".to_string())?;
    Ok(home.join(".tiancode").join("mcp.json"))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct McpClientSyncResult {
    pub client: String,
    pub path: String,
    pub action: String,
}

pub fn sync_detected_mcp_clients(url: &str, token: &str) -> Vec<McpClientSyncResult> {
    let mut results = Vec::new();
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return results,
    };

    // 1. TianCode: 检测 ~/.tiancode 目录
    let tiancode_dir = home.join(".tiancode");
    if tiancode_dir.exists() {
        let path = tiancode_dir.join("mcp.json");
        if let Ok(action) = write_cursor_mcp_file(&path, url, token) {
            results.push(McpClientSyncResult {
                client: "tiancode".into(),
                path: path.display().to_string(),
                action: action.as_str().into(),
            });
        }
    }

    // 2. Cursor: 检测 ~/.cursor 目录
    let cursor_dir = home.join(".cursor");
    if cursor_dir.exists() {
        let path = cursor_dir.join("mcp.json");
        if let Ok(action) = write_cursor_mcp_file(&path, url, token) {
            results.push(McpClientSyncResult {
                client: "cursor".into(),
                path: path.display().to_string(),
                action: action.as_str().into(),
            });
        }
    }

    results
}

pub fn write_cursor_mcp_file(path: &Path, url: &str, token: &str) -> Result<CursorMcpAction, String> {
    let existed = path.exists();
    let original = if existed {
        Some(fs::read_to_string(path).map_err(|err| format!("读不到 Cursor 配置: {err}"))?)
    } else {
        None
    };
    let (next, changed) = merge_cursor_mcp(original.as_deref(), url, token)?;
    if !changed {
        return Ok(CursorMcpAction::Unchanged);
    }
    let mut text = serde_json::to_string_pretty(&next).map_err(|err| format!("整理 Cursor 配置失败: {err}"))?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    write_private_atomic(path, text.as_bytes()).map_err(|err| format!("写不了 Cursor 配置: {err}"))?;
    Ok(if existed { CursorMcpAction::Updated } else { CursorMcpAction::Created })
}

/// 返回合并后的文档，以及 tianshell 这一条是否和原来不同。
/// 失败时调用方不应写文件。错误文案不包含令牌或文件正文。
pub fn merge_cursor_mcp(existing: Option<&str>, url: &str, token: &str) -> Result<(Value, bool), String> {
    let entry = cursor_entry(url, token);
    let trimmed = existing.map(str::trim).filter(|text| !text.is_empty());
    let mut root = match trimmed {
        None => json!({}),
        Some(text) => serde_json::from_str::<Value>(text).map_err(|_| "Cursor 配置不是合法 JSON，没有改动".to_string())?,
    };
    if !root.is_object() {
        return Err("Cursor 配置的顶层不是对象，没有改动".into());
    }
    match root.get("mcpServers") {
        None => {
            root["mcpServers"] = json!({ "tianshell": entry });
            Ok((root, true))
        }
        Some(servers) if !servers.is_object() => Err("mcpServers 不是对象，没有改动".into()),
        Some(servers) => {
            if servers.get("tianshell") == Some(&entry) {
                return Ok((root, false));
            }
            root["mcpServers"]["tianshell"] = entry;
            Ok((root, true))
        }
    }
}

fn cursor_entry(url: &str, token: &str) -> Value {
    json!({
        "url": url,
        "headers": {
            "Authorization": format!("Bearer {token}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TOKEN: &str = "unit-test-token";
    const URL: &str = "http://127.0.0.1:53921/mcp";

    #[test]
    fn keeps_other_servers_and_replaces_only_tianshell() {
        let existing = r#"{
          "mcpServers": {
            "other": { "command": "echo", "args": ["hi"] },
            "tianshell": { "url": "http://127.0.0.1:1/mcp", "headers": { "Authorization": "Bearer old" } }
          }
        }"#;
        let (doc, changed) = merge_cursor_mcp(Some(existing), URL, TOKEN).unwrap();
        assert!(changed);
        assert_eq!(doc["mcpServers"]["other"]["command"], "echo");
        assert_eq!(doc["mcpServers"]["other"]["args"][0], "hi");
        assert_eq!(doc["mcpServers"]["tianshell"]["url"], URL);
        assert!(doc["mcpServers"]["tianshell"].get("type").is_none());
        let err = merge_cursor_mcp(Some("["), URL, TOKEN).unwrap_err();
        assert!(!err.contains(TOKEN));
        assert!(!err.contains("echo"));
    }

    #[test]
    fn identical_entry_does_not_count_as_a_change() {
        let existing = json!({
            "mcpServers": {
                "tianshell": {
                    "url": URL,
                    "headers": { "Authorization": format!("Bearer {TOKEN}") }
                }
            }
        });
        let (_, changed) = merge_cursor_mcp(Some(&existing.to_string()), URL, TOKEN).unwrap();
        assert!(!changed);
    }

    #[test]
    fn refuses_a_non_object_server_map_without_rewriting() {
        let err = merge_cursor_mcp(Some(r#"{"mcpServers":[]}"#), URL, TOKEN).unwrap_err();
        assert!(err.contains("没有改动"));
        assert!(!err.contains(TOKEN));
    }

    #[test]
    fn write_creates_then_leaves_an_unchanged_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        assert_eq!(write_cursor_mcp_file(&path, URL, TOKEN).unwrap(), CursorMcpAction::Created);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["mcpServers"]["tianshell"]["url"], URL);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let before = std::fs::read_to_string(&path).unwrap();
        assert_eq!(write_cursor_mcp_file(&path, URL, TOKEN).unwrap(), CursorMcpAction::Unchanged);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        let nested = dir.path().join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let path = nested.join("mcp.json");
        std::fs::write(&path, r#"{"mcpServers":{"notes":{"command":"true"}},"extra":1}"#).unwrap();
        assert_eq!(write_cursor_mcp_file(&path, URL, TOKEN).unwrap(), CursorMcpAction::Updated);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["mcpServers"]["notes"]["command"], "true");
        assert_eq!(saved["extra"], 1);
        assert_eq!(saved["mcpServers"]["tianshell"]["url"], URL);

        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{not json").unwrap();
        let err = write_cursor_mcp_file(&broken, URL, TOKEN).unwrap_err();
        assert!(!err.contains(TOKEN));
        assert_eq!(std::fs::read_to_string(&broken).unwrap(), "{not json");

        let tc_path = tiancode_user_mcp_path().unwrap();
        let s = tc_path.to_string_lossy();
        assert!(s.ends_with(".tiancode/mcp.json") || s.ends_with(".tiancode\\mcp.json"));
    }
}
