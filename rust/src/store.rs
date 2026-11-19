use std::fs;
use std::path::{Path, PathBuf};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use crate::types::{AppSettings, HostGroup, HostRecord, Snippet};
use crate::vault::{open, seal, VaultError};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("找不到对应的主机: {0}")]
    HostNotFound(String),
    #[error("跳板机引用发生循环: 主机 {0}")]
    CycleDetected(String),
    #[error("主机别名已存在: {0}")]
    DuplicateAlias(String),
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
    fs::rename(&tmp_path, path)?;
    Ok(())
}

fn read_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> Result<T, StoreError> {
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

    pub fn list_groups(&self) -> &[HostGroup] {
        &self.groups
    }

    pub fn save_group(&mut self, name: &str, parent_id: Option<String>) -> Result<HostGroup, StoreError> {
        let group = HostGroup {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            parent_id,
        };
        self.groups.push(group.clone());
        self.persist()?;
        Ok(group)
    }

    pub fn save(&mut self, mut record: HostRecord) -> Result<HostRecord, StoreError> {
        if let Some(jump_id) = &record.jump_host_id {
            self.check_cycle(&record.id, jump_id)?;
        }
        let now = chrono_now_iso();
        record.updated_at = now.clone();

        if let Some(existing) = self.hosts.iter_mut().find(|h| h.id == record.id) {
            *existing = record.clone();
        } else {
            if record.id.is_empty() {
                record.id = Uuid::new_v4().to_string();
            }
            if record.created_at.is_empty() {
                record.created_at = now;
            }
            self.hosts.push(record.clone());
        }
        self.persist()?;
        Ok(record)
    }

    pub fn delete(&mut self, id: &str) -> Result<bool, StoreError> {
        let init_len = self.hosts.len();
        self.hosts.retain(|h| h.id != id);
        if self.hosts.len() != init_len {
            self.persist()?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn seal_secret(&self, plaintext: &str) -> Result<String, StoreError> {
        Ok(seal(&self.key, plaintext)?)
    }

    pub fn open_secret(&self, sealed: &str) -> Result<String, StoreError> {
        Ok(open(&self.key, sealed)?)
    }

    fn check_cycle(&self, target_id: &str, jump_id: &str) -> Result<(), StoreError> {
        let mut curr = Some(jump_id);
        while let Some(cid) = curr {
            if cid == target_id {
                return Err(StoreError::CycleDetected(target_id.to_string()));
            }
            curr = self.hosts.iter().find(|h| h.id == cid).and_then(|h| h.jump_host_id.as_deref());
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), StoreError> {
        let payload = HostsFilePayload {
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

    pub fn save(&mut self, mut s: Snippet) -> Result<Snippet, StoreError> {
        if s.id.is_empty() {
            s.id = Uuid::new_v4().to_string();
        }
        if s.created_at.is_empty() {
            s.created_at = chrono_now_iso();
        }
        if let Some(pos) = self.snippets.iter().position(|it| it.id == s.id) {
            self.snippets[pos] = s.clone();
        } else {
            self.snippets.push(s.clone());
        }
        atomic_write_json(&self.path, &self.snippets)?;
        Ok(s)
    }
}

pub struct SettingsStore {
    path: PathBuf,
    settings: AppSettings,
}

impl SettingsStore {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let p: PathBuf = path.into();
        let settings: AppSettings = read_json_or_default(&p)?;
        Ok(Self { path: p, settings })
    }

    pub fn get(&self) -> &AppSettings {
        &self.settings
    }

    pub fn update(&mut self, s: AppSettings) -> Result<(), StoreError> {
        self.settings = s;
        atomic_write_json(&self.path, &self.settings)
    }
}

fn chrono_now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{}.{:03}Z", dur.as_secs(), dur.subsec_millis())
}
