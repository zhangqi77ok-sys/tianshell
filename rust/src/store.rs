use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;
use uuid::Uuid;
use crate::types::{AppSettings, AuthType, HostGroup, HostRecord, PublicHost, Snippet, TunnelRule};
use crate::util::iso_now;
use crate::vault::{open, seal, VaultError};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("找不到对应的主机: {0}")]
    HostNotFound(String),
    #[error("跳板机引用存在循环依赖，请检查跳板机设置")]
    CycleDetected(String),
    #[error("已经有别名叫「{0}」的主机了")]
    DuplicateAlias(String),
    #[error("{0}")]
    Invalid(String),
    #[error("Vault 加解密错误: {0}")]
    Vault(#[from] VaultError),
    #[error("序列化/反序列化错误: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO 存储错误: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HostsFilePayload {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub hosts: Vec<HostRecord>,
    #[serde(default)]
    pub groups: Vec<HostGroup>,
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(value)?;
    let tmp_path = path.with_extension(format!("tmp.{}", Uuid::new_v4()));
    fs::write(&tmp_path, data.as_bytes())?;
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..6 {
        match fs::rename(&tmp_path, path) {
            Ok(()) => {
                last_err = None;
                break;
            }
            Err(err) => {
                last_err = Some(err);
                if attempt < 5 {
                    thread::sleep(Duration::from_millis(20 * (attempt as u64 + 1)));
                }
            }
        }
    }
    if let Some(err) = last_err {
        let _ = fs::remove_file(&tmp_path);
        return Err(err.into());
    }
    crate::util::restrict_user_file(path);
    Ok(())
}

fn read_json_or_default<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> Result<T, StoreError> {
    if !path.exists() {
        return Ok(T::default());
    }
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

pub struct HostStore {
    path: PathBuf,
    key: [u8; 32],
    hosts: Vec<HostRecord>,
    groups: Vec<HostGroup>,
}

impl HostStore {
    pub fn load(path: impl Into<PathBuf>, key: [u8; 32]) -> Result<Self, StoreError> {
        let p: PathBuf = path.into();
        let payload: HostsFilePayload = read_json_or_default(&p)?;
        Ok(Self {
            path: p,
            key,
            hosts: payload.hosts,
            groups: payload.groups,
        })
    }

    pub fn list(&self) -> &[HostRecord] {
        &self.hosts
    }

    pub fn list_public(&self) -> Vec<PublicHost> {
        let mut items: Vec<_> = self.hosts.iter().map(HostRecord::to_public).collect();
        items.sort_by(|a, b| a.alias.to_lowercase().cmp(&b.alias.to_lowercase()));
        items
    }

    pub fn list_groups(&self) -> &[HostGroup] {
        &self.groups
    }

    pub fn list_groups_sorted(&self) -> Vec<HostGroup> {
        let mut groups = self.groups.clone();
        groups.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.name.cmp(&b.name)));
        groups
    }

    pub fn save_group(&mut self, name: &str, parent_id: Option<String>) -> Result<HostGroup, StoreError> {
        self.save_group_draft(&json!({ "name": name, "parentId": parent_id }))
    }

    pub fn save_group_draft(&mut self, draft: &Value) -> Result<HostGroup, StoreError> {
        let name = require_text(draft.get("name"), "分组名称", 48)?;
        let id = text_field(draft, "id");
        let parent_present = draft.get("parentId").is_some();
        let parent_id = if parent_present && !draft["parentId"].is_null() {
            draft["parentId"].as_str().map(|s| s.to_string())
        } else if parent_present {
            None
        } else {
            None
        };
        let order = draft.get("order").and_then(|v| v.as_i64()).map(|n| n as i32);
        if let Some(id) = id.clone() {
            if let Some(existing) = self.groups.iter_mut().find(|g| g.id == id) {
                existing.name = name;
                if parent_present {
                    existing.parent_id = parent_id;
                }
                if let Some(order) = order {
                    existing.order = order;
                }
                let group = existing.clone();
                self.persist()?;
                return Ok(group);
            }
        }
        let group = HostGroup {
            id: id.unwrap_or_else(|| Uuid::new_v4().to_string()),
            name,
            parent_id,
            order: order.unwrap_or(self.groups.len() as i32),
        };
        self.groups.push(group.clone());
        self.persist()?;
        Ok(group)
    }

    pub fn delete_group(&mut self, id: &str) -> Result<(), StoreError> {
        let before = self.groups.len();
        self.groups.retain(|g| g.id != id);
        if self.groups.len() == before {
            return Err(StoreError::Invalid("找不到这个分组".into()));
        }
        for host in &mut self.hosts {
            if host.group_id.as_deref() == Some(id) {
                host.group_id = None;
            }
        }
        self.persist()?;
        Ok(())
    }

    pub fn save(&mut self, mut record: HostRecord) -> Result<HostRecord, StoreError> {
        if record.id.is_empty() {
            record.id = Uuid::new_v4().to_string();
        }
        if let Some(jump_id) = &record.jump_host_id {
            self.check_cycle(&record.id, jump_id)?;
        }
        let now = iso_now();
        if record.created_at.is_empty() {
            record.created_at = now.clone();
        }
        record.updated_at = now;
        if let Some(existing) = self.hosts.iter_mut().find(|h| h.id == record.id) {
            *existing = record.clone();
        } else {
            self.hosts.push(record.clone());
        }
        self.persist()?;
        Ok(record)
    }

    pub fn save_draft(&mut self, draft: &Value) -> Result<PublicHost, StoreError> {
        let id_opt = text_field(draft, "id");
        let existing = if let Some(id) = &id_opt {
            Some(
                self.hosts
                    .iter()
                    .find(|h| &h.id == id)
                    .cloned()
                    .ok_or_else(|| StoreError::HostNotFound(id.clone()))?,
            )
        } else {
            None
        };
        let alias = require_text(draft.get("alias"), "别名", 64)?;
        let host = require_text(draft.get("host"), "主机地址", 253)?;
        if host.chars().any(char::is_whitespace) {
            return Err(StoreError::Invalid("主机地址里不能有空格".into()));
        }
        let username = require_text(draft.get("username"), "用户名", 128)?;
        if username.chars().any(char::is_whitespace) {
            return Err(StoreError::Invalid("用户名里不能有空格".into()));
        }
        let fallback_port = existing.as_ref().map(|h| h.port).unwrap_or(22);
        let port = if draft.get("port").is_none() {
            fallback_port
        } else {
            parse_port(draft.get("port"), fallback_port)?
        };
        let auth = match draft.get("authType").and_then(|v| v.as_str()) {
            Some("password") => AuthType::Password,
            Some("privateKey") | Some("privatekey") => AuthType::PrivateKey,
            _ => return Err(StoreError::Invalid("认证方式只能是密码或私钥".into())),
        };
        let raw_secret = draft.get("secret").and_then(|v| v.as_str()).unwrap_or("");
        let secret = if !raw_secret.trim().is_empty() {
            if auth == AuthType::PrivateKey && !raw_secret.contains("PRIVATE KEY") {
                return Err(StoreError::Invalid(
                    "私钥需要是 OpenSSH 或 PEM 格式（内容里要有 PRIVATE KEY）".into(),
                ));
            }
            self.seal_secret(raw_secret)?
        } else if existing.as_ref().map(|h| h.auth_type == auth).unwrap_or(false) {
            existing.as_ref().map(|h| h.secret.clone()).unwrap_or_default()
        } else if auth == AuthType::Password {
            return Err(StoreError::Invalid("请填写密码".into()));
        } else {
            return Err(StoreError::Invalid("请填写私钥".into()));
        };
        let passphrase = if auth != AuthType::PrivateKey {
            None
        } else if draft.get("clearPassphrase").and_then(|v| v.as_bool()).unwrap_or(false) {
            None
        } else if let Some(phrase) = draft.get("passphrase").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
            Some(self.seal_secret(phrase)?)
        } else {
            existing.as_ref().and_then(|h| h.passphrase.clone())
        };
        let fingerprint = if draft.get("clearFingerprint").and_then(|v| v.as_bool()).unwrap_or(false) {
            None
        } else {
            existing.as_ref().and_then(|h| h.fingerprint.clone())
        };
        let group_id = optional_id(draft, "groupId", existing.as_ref().and_then(|h| h.group_id.clone()));
        let jump_host_id = optional_id(draft, "jumpHostId", existing.as_ref().and_then(|h| h.jump_host_id.clone()));
        let tags = if let Some(arr) = draft.get("tags").and_then(|v| v.as_array()) {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        } else {
            existing.as_ref().map(|h| h.tags.clone()).unwrap_or_default()
        };
        let notes = if draft.get("notes").is_none() {
            existing.as_ref().and_then(|h| h.notes.clone())
        } else if draft["notes"].is_null() {
            None
        } else {
            draft["notes"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
        };
        let now = iso_now();
        let record = HostRecord {
            id: existing.as_ref().map(|h| h.id.clone()).unwrap_or_else(|| Uuid::new_v4().to_string()),
            alias,
            host,
            port,
            username,
            auth_type: auth,
            secret,
            passphrase,
            fingerprint,
            group_id,
            jump_host_id,
            tags,
            notes,
            created_at: existing.as_ref().map(|h| h.created_at.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| now.clone()),
            updated_at: now,
        };
        if self.hosts.iter().any(|h| h.id != record.id && h.alias.eq_ignore_ascii_case(&record.alias)) {
            return Err(StoreError::DuplicateAlias(record.alias));
        }
        if let Some(jump_id) = &record.jump_host_id {
            self.check_cycle(&record.id, jump_id)?;
        }
        if let Some(existing_pos) = self.hosts.iter().position(|h| h.id == record.id) {
            self.hosts[existing_pos] = record.clone();
        } else {
            self.hosts.push(record.clone());
        }
        self.persist()?;
        Ok(record.to_public())
    }

    pub fn delete(&mut self, id: &str) -> Result<bool, StoreError> {
        let before = self.hosts.len();
        self.hosts.retain(|h| h.id != id);
        if self.hosts.len() == before {
            return Ok(false);
        }
        for host in &mut self.hosts {
            if host.jump_host_id.as_deref() == Some(id) {
                host.jump_host_id = None;
            }
        }
        self.persist()?;
        Ok(true)
    }

    pub fn remember_fingerprint(&mut self, id: &str, fingerprint: &str) -> Result<bool, StoreError> {
        if fingerprint.is_empty() {
            return Ok(false);
        }
        let should_write = {
            let Some(host) = self.hosts.iter_mut().find(|h| h.id == id) else {
                return Ok(false);
            };
            match &host.fingerprint {
                Some(existing) if !existing.is_empty() => return Ok(existing == fingerprint),
                _ => {
                    host.fingerprint = Some(fingerprint.to_string());
                    host.updated_at = iso_now();
                    true
                }
            }
        };
        if should_write {
            self.persist()?;
        }
        Ok(true)
    }

    pub fn seal_secret(&self, plaintext: &str) -> Result<String, StoreError> {
        Ok(seal(&self.key, plaintext)?)
    }

    pub fn open_secret(&self, sealed: &str) -> Result<String, StoreError> {
        Ok(open(&self.key, sealed)?)
    }

    fn check_cycle(&self, target_id: &str, jump_id: &str) -> Result<(), StoreError> {
        let mut curr = Some(jump_id);
        let mut guard = 0;
        while let Some(cid) = curr {
            if cid == target_id || guard > 32 {
                return Err(StoreError::CycleDetected(target_id.to_string()));
            }
            guard += 1;
            curr = self.hosts.iter().find(|h| h.id == cid).and_then(|h| h.jump_host_id.as_deref());
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), StoreError> {
        let payload = HostsFilePayload {
            version: 1,
            hosts: self.hosts.clone(),
            groups: self.groups.clone(),
        };
        atomic_write_json(&self.path, &payload)
    }
}

pub struct SnippetStore {
    path: PathBuf,
    snippets: Vec<Snippet>,
}

impl SnippetStore {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let p: PathBuf = path.into();
        let snippets: Vec<Snippet> = read_json_or_default(&p)?;
        Ok(Self { path: p, snippets })
    }

    pub fn list(&self) -> &[Snippet] {
        &self.snippets
    }

    pub fn save(&mut self, mut snippet: Snippet) -> Result<Snippet, StoreError> {
        if snippet.id.is_empty() {
            snippet.id = Uuid::new_v4().to_string();
        }
        if snippet.created_at.is_empty() {
            snippet.created_at = iso_now();
        }
        if snippet.category.trim().is_empty() {
            snippet.category = "常用".into();
        }
        if let Some(pos) = self.snippets.iter().position(|it| it.id == snippet.id) {
            self.snippets[pos] = snippet.clone();
        } else {
            self.snippets.push(snippet.clone());
        }
        atomic_write_json(&self.path, &self.snippets)?;
        Ok(snippet)
    }

    pub fn save_draft(&mut self, draft: &Value) -> Result<Snippet, StoreError> {
        let name = require_text(draft.get("name"), "快捷命令名称", 80)?;
        let command = require_text(draft.get("command"), "快捷命令内容", 8000)?;
        let category = draft
            .get("category")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("常用")
            .to_string();
        let description = draft
            .get("description")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let id = text_field(draft, "id").unwrap_or_default();
        self.save(Snippet {
            id,
            name,
            category,
            command,
            description,
            created_at: String::new(),
        })
    }

    pub fn delete(&mut self, id: &str) -> Result<(), StoreError> {
        let before = self.snippets.len();
        self.snippets.retain(|s| s.id != id);
        if self.snippets.len() == before {
            return Err(StoreError::Invalid("找不到该快捷命令".into()));
        }
        atomic_write_json(&self.path, &self.snippets)?;
        Ok(())
    }
}

pub struct SettingsStore {
    path: PathBuf,
    settings: AppSettings,
}

impl SettingsStore {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let p: PathBuf = path.into();
        let settings = normalize_settings(read_json_or_default(&p)?);
        Ok(Self { path: p, settings })
    }

    pub fn get(&self) -> &AppSettings {
        &self.settings
    }

    pub fn update(&mut self, settings: AppSettings) -> Result<AppSettings, StoreError> {
        self.settings = normalize_settings(settings);
        atomic_write_json(&self.path, &self.settings)?;
        Ok(self.settings.clone())
    }

    pub fn apply_patch(&mut self, patch: &Value) -> Result<AppSettings, StoreError> {
        let mut next = self.settings.clone();
        if let Some(theme) = patch.get("theme").and_then(|v| v.as_str()) {
            next.theme = theme.to_string();
        }
        if let Some(size) = patch.get("fontSize").and_then(|v| v.as_u64()) {
            next.font_size = size.min(255) as u8;
        }
        if let Some(interval) = patch.get("monitorIntervalMs").and_then(|v| v.as_u64()) {
            next.monitor_interval_ms = interval;
        }
        if let Some(hidden) = patch.get("showHiddenFiles").and_then(|v| v.as_bool()) {
            next.show_hidden_files = hidden;
        }
        self.update(next)
    }
}

pub struct TunnelStore {
    path: PathBuf,
    rules: Vec<TunnelRule>,
}

impl TunnelStore {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let p: PathBuf = path.into();
        let rules: Vec<TunnelRule> = read_json_or_default(&p)?;
        Ok(Self { path: p, rules })
    }

    pub fn list(&self) -> &[TunnelRule] {
        &self.rules
    }

    pub fn save_draft(&mut self, draft: &Value) -> Result<TunnelRule, StoreError> {
        let name = require_text(draft.get("name"), "隧道名称", 80)?;
        let host_id = require_text(draft.get("hostId"), "关联 SSH 主机", 80)?;
        let mode = draft.get("mode").and_then(|v| v.as_str()).unwrap_or("");
        if mode != "local" && mode != "remote" && mode != "socks5" {
            return Err(StoreError::Invalid("不支持的隧道模式".into()));
        }
        let local_port = parse_port(draft.get("localPort"), 0)?;
        if local_port == 0 {
            return Err(StoreError::Invalid("本地端口必须是 1 到 65535 的整数".into()));
        }
        let local_host = draft
            .get("localHost")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("127.0.0.1")
            .to_string();
        let (remote_host, remote_port) = if mode == "socks5" {
            (None, None)
        } else {
            let host = draft
                .get("remoteHost")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("127.0.0.1")
                .to_string();
            let port = parse_port(draft.get("remotePort"), 0)?;
            if port == 0 {
                return Err(StoreError::Invalid("目标端口必须是 1 到 65535 的整数".into()));
            }
            (Some(host), Some(port))
        };
        let rule = TunnelRule {
            id: text_field(draft, "id").unwrap_or_else(|| Uuid::new_v4().to_string()),
            name,
            host_id,
            mode: mode.to_string(),
            local_host,
            local_port,
            remote_host,
            remote_port,
        };
        if let Some(pos) = self.rules.iter().position(|r| r.id == rule.id) {
            self.rules[pos] = rule.clone();
        } else {
            self.rules.push(rule.clone());
        }
        atomic_write_json(&self.path, &self.rules)?;
        Ok(rule)
    }

    pub fn delete(&mut self, id: &str) -> Result<(), StoreError> {
        let before = self.rules.len();
        self.rules.retain(|r| r.id != id);
        if self.rules.len() == before {
            return Err(StoreError::Invalid("找不到该隧道规则".into()));
        }
        atomic_write_json(&self.path, &self.rules)?;
        Ok(())
    }
}

fn normalize_settings(mut settings: AppSettings) -> AppSettings {
    if settings.theme != "light" {
        settings.theme = "dark".into();
    }
    if !(11..=22).contains(&settings.font_size) {
        settings.font_size = 14;
    }
    if !(1000..=10_000).contains(&settings.monitor_interval_ms) {
        settings.monitor_interval_ms = 2500;
    }
    settings
}

fn require_text(value: Option<&Value>, label: &str, max: usize) -> Result<String, StoreError> {
    let text = value.and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if text.is_empty() {
        return Err(StoreError::Invalid(format!("请填写{label}")));
    }
    if text.chars().count() > max {
        return Err(StoreError::Invalid(format!("{label}太长了")));
    }
    Ok(text)
}

fn text_field(draft: &Value, key: &str) -> Option<String> {
    draft.get(key).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn optional_id(draft: &Value, key: &str, previous: Option<String>) -> Option<String> {
    match draft.get(key) {
        None => previous,
        Some(value) if value.is_null() => None,
        Some(value) => value.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
    }
}

fn parse_port(value: Option<&Value>, fallback: u16) -> Result<u16, StoreError> {
    let Some(value) = value else {
        return Ok(fallback);
    };
    if value.is_null() {
        return Ok(fallback);
    }
    let port = value
        .as_u64()
        .ok_or_else(|| StoreError::Invalid("端口需要是 1 到 65535 的整数".into()))?;
    if (1..=65535).contains(&port) {
        Ok(port as u16)
    } else {
        Err(StoreError::Invalid("端口需要是 1 到 65535 的整数".into()))
    }
}
