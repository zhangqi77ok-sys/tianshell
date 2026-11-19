//! Command memory stored in the TianShell data directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::redact::redact_secrets;
use crate::state::lock;
use crate::util::{iso_now, write_private_atomic};

const MAX_ENTRIES: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub at: String,
    #[serde(rename = "hostId", default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(rename = "sessionKind")]
    pub session_kind: String,
    #[serde(rename = "sessionId", default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(rename = "exitCode", default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    pub source: String,
}

#[derive(Debug)]
pub struct HistoryDraft {
    pub host_id: Option<String>,
    pub alias: Option<String>,
    pub session_kind: String,
    pub session_id: Option<String>,
    pub command: String,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub ok: Option<bool>,
    pub source: String,
}

pub struct CommandHistory {
    path: Option<PathBuf>,
    entries: Mutex<Vec<HistoryEntry>>,
    token: Mutex<String>,
}

impl CommandHistory {
    pub fn memory() -> Self {
        Self {
            path: None,
            entries: Mutex::new(Vec::new()),
            token: Mutex::new(String::new()),
        }
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let entries = if path.exists() {
            let raw = fs::read_to_string(&path).map_err(|err| err.to_string())?;
            if raw.trim().is_empty() {
                Vec::new()
            } else {
                serde_json::from_str(&raw).map_err(|err| err.to_string())?
            }
        } else {
            Vec::new()
        };
        Ok(Self {
            path: Some(path),
            entries: Mutex::new(entries),
            token: Mutex::new(String::new()),
        })
    }

    pub fn set_token(&self, token: &str) {
        *lock(&self.token) = token.trim().to_string();
    }

    pub fn record(&self, draft: HistoryDraft) -> Option<String> {
        let command = self.redact(&draft.command);
        let command = command.trim();
        if command.is_empty() {
            return None;
        }
        let entry = HistoryEntry {
            id: Uuid::new_v4().to_string(),
            at: iso_now(),
            host_id: draft.host_id,
            alias: draft.alias,
            session_kind: draft.session_kind,
            session_id: draft.session_id,
            command: truncate_chars(command, 8000),
            cwd: draft.cwd.map(|value| truncate_chars(&self.redact(&value), 1024)),
            exit_code: draft.exit_code,
            ok: draft.ok.or_else(|| draft.exit_code.map(|code| code == 0)),
            source: draft.source,
        };
        let id = entry.id.clone();
        {
            let mut entries = lock(&self.entries);
            entries.push(entry);
            if entries.len() > MAX_ENTRIES {
                let extra = entries.len() - MAX_ENTRIES;
                entries.drain(..extra);
            }
        }
        self.persist();
        Some(id)
    }

    pub fn redact_text(&self, text: &str) -> String {
        self.redact(text)
    }

    pub fn finish(&self, id: &str, exit_code: Option<i32>, cwd: Option<String>, ok: Option<bool>) {
        let cwd = cwd.map(|value| truncate_chars(&self.redact(&value), 1024));
        let mut changed = false;
        {
            let mut entries = lock(&self.entries);
            if let Some(entry) = entries.iter_mut().rev().find(|entry| entry.id == id) {
                entry.exit_code = exit_code;
                if cwd.is_some() {
                    entry.cwd = cwd;
                }
                entry.ok = ok.or_else(|| exit_code.map(|code| code == 0));
                changed = true;
            }
        }
        if changed {
            self.persist();
        }
    }

    pub fn search(&self, query: Option<&str>, host: Option<&str>, limit: usize) -> Vec<HistoryEntry> {
        let query = query.map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_ascii_lowercase());
        let host = host.map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_ascii_lowercase());
        let limit = limit.clamp(1, 200);
        let entries = lock(&self.entries);
        entries
            .iter()
            .rev()
            .filter(|entry| {
                if let Some(host) = &host {
                    let alias = entry.alias.as_deref().unwrap_or("").to_ascii_lowercase();
                    let id = entry.host_id.as_deref().unwrap_or("").to_ascii_lowercase();
                    if alias != *host && id != *host && !alias.contains(host) && !id.contains(host) {
                        return false;
                    }
                }
                if let Some(query) = &query {
                    if !entry.command.to_ascii_lowercase().contains(query) {
                        return false;
                    }
                }
                true
            })
            .take(limit)
            .cloned()
            .collect()
    }

    fn redact(&self, text: &str) -> String {
        let token = lock(&self.token).clone();
        let extra: Vec<&str> = if token.len() >= 8 { vec![token.as_str()] } else { Vec::new() };
        redact_secrets(text, &extra)
    }

    fn persist(&self) {
        let Some(path) = &self.path else { return };
        let entries = lock(&self.entries);
        if let Ok(body) = serde_json::to_string_pretty(&*entries) {
            let _ = write_private_atomic(path, body.as_bytes());
        }
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect()
    }
}

pub fn load_token_quietly(dir: &Path) -> String {
    fs::read_to_string(dir.join("mcp.token"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn history_survives_reload_and_omits_secrets() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("command-history.json");
        let history = CommandHistory::open(&path).unwrap();
        history.set_token("abcdef0123456789abcdef0123456789");
        history.record(HistoryDraft {
            host_id: Some("h1".into()),
            alias: Some("lab".into()),
            session_kind: "ssh".into(),
            session_id: Some("s1".into()),
            command: "export AWS_SECRET_ACCESS_KEY=supersecretvalue && echo ok".into(),
            cwd: Some("/srv".into()),
            exit_code: Some(0),
            ok: Some(true),
            source: "mcp".into(),
        });
        let id = history
            .record(HistoryDraft {
                host_id: None,
                alias: None,
                session_kind: "local".into(),
                session_id: None,
                command: "printf ready".into(),
                cwd: Some("/home/me".into()),
                exit_code: None,
                ok: None,
                source: "interactive".into(),
            })
            .unwrap();
        history.finish(&id, Some(0), Some("/home/me".into()), Some(true));
        drop(history);
        let reloaded = CommandHistory::open(&path).unwrap();
        let recent = reloaded.search(None, None, 10);
        assert_eq!(recent[0].command, "printf ready");
        assert_eq!(recent[0].exit_code, Some(0));
        assert_eq!(recent[0].ok, Some(true));
        let secret_row = reloaded.search(Some("AWS_SECRET"), Some("lab"), 5);
        assert_eq!(secret_row.len(), 1);
        assert!(!secret_row[0].command.contains("supersecretvalue"));
        assert!(secret_row[0].command.contains("[redacted]"));
        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("supersecretvalue"));
        assert!(!raw.contains("abcdef0123456789abcdef0123456789"));
    }
}
