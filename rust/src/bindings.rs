//! A local project path pinned to a remote host path.
//!
//! The binding is a reminder for a coding tool. It does not sync files.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::redact::contains_secret;
use crate::state::lock;
use crate::util::{iso_now, write_private_atomic};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectBinding {
    pub id: String,
    pub name: String,
    #[serde(rename = "localPath")]
    pub local_path: String,
    #[serde(rename = "hostId")]
    pub host_id: String,
    pub alias: String,
    #[serde(rename = "remotePath")]
    pub remote_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

pub struct BindingStore {
    path: PathBuf,
    items: Mutex<Vec<ProjectBinding>>,
}

impl BindingStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let items = if path.exists() {
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
            path,
            items: Mutex::new(items),
        })
    }

    pub fn list(&self) -> Vec<ProjectBinding> {
        lock(&self.items).clone()
    }

    pub fn save(
        &self,
        id: Option<String>,
        name: &str,
        local_path: &str,
        host_id: &str,
        alias: &str,
        remote_path: &str,
        note: Option<String>,
    ) -> Result<ProjectBinding, String> {
        let name = clean(name, "名称", 80)?;
        let local_path = clean(local_path, "本地路径", 1024)?;
        let remote_path = clean(remote_path, "远端路径", 1024)?;
        if let Some(note) = &note {
            if contains_secret(note) {
                return Err("绑定备注里不能放私钥、密码或令牌".into());
            }
        }
        let mut items = lock(&self.items);
        if let Some(id) = id.as_ref() {
            if let Some(existing) = items.iter_mut().find(|item| &item.id == id) {
                existing.name = name;
                existing.local_path = local_path;
                existing.host_id = host_id.to_string();
                existing.alias = alias.to_string();
                existing.remote_path = remote_path;
                existing.note = note.filter(|text| !text.trim().is_empty());
                existing.updated_at = iso_now();
                let saved = existing.clone();
                drop(items);
                self.persist()?;
                return Ok(saved);
            }
        }
        let saved = ProjectBinding {
            id: id.unwrap_or_else(|| Uuid::new_v4().to_string()),
            name,
            local_path,
            host_id: host_id.to_string(),
            alias: alias.to_string(),
            remote_path,
            note: note.filter(|text| !text.trim().is_empty()),
            updated_at: iso_now(),
        };
        items.push(saved.clone());
        drop(items);
        self.persist()?;
        Ok(saved)
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let mut items = lock(&self.items);
        let before = items.len();
        items.retain(|item| item.id != id);
        if items.len() == before {
            return Err("找不到这个路径绑定".into());
        }
        drop(items);
        self.persist()
    }

    fn persist(&self) -> Result<(), String> {
        let items = lock(&self.items);
        let body = serde_json::to_string_pretty(&*items).map_err(|err| err.to_string())?;
        write_private_atomic(&self.path, body.as_bytes()).map_err(|err| err.to_string())
    }
}

fn clean(value: &str, label: &str, max: usize) -> Result<String, String> {
    let text = value.trim();
    if text.is_empty() || text.contains('\0') {
        return Err(format!("请填写{label}"));
    }
    if text.chars().count() > max {
        return Err(format!("{label}太长了"));
    }
    Ok(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn bindings_persist_and_reject_secret_notes() {
        let dir = tempdir().unwrap();
        let store = BindingStore::open(dir.path().join("bindings.json")).unwrap();
        let saved = store
            .save(None, "api", "/src/api", "h1", "lab", "/srv/api", Some("port 8080".into()))
            .unwrap();
        assert!(store
            .save(None, "bad", "/src", "h1", "lab", "/srv", Some("export TOKEN=abcdef".into()))
            .is_err());
        drop(store);
        let reloaded = BindingStore::open(dir.path().join("bindings.json")).unwrap();
        let items = reloaded.list();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, saved.id);
        assert_eq!(items[0].remote_path, "/srv/api");
        let raw = std::fs::read_to_string(dir.path().join("bindings.json")).unwrap();
        assert!(!raw.contains("TOKEN=abcdef"));
    }
}
