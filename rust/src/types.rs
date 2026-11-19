use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthType {
    #[serde(rename = "password")]
    Password,
    #[serde(rename = "privateKey", alias = "privatekey")]
    PrivateKey,
}

impl Default for AuthType {
    fn default() -> Self {
        Self::Password
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRecord {
    pub id: String,
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(rename = "authType")]
    pub auth_type: AuthType,
    #[serde(default)]
    pub secret: String,
    #[serde(default)]
    pub passphrase: Option<String>,
    #[serde(default, rename = "fingerprint", alias = "knownFingerprint")]
    pub fingerprint: Option<String>,
    #[serde(default, rename = "groupId", alias = "group")]
    pub group_id: Option<String>,
    #[serde(default, rename = "jumpHostId")]
    pub jump_host_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, rename = "notes", alias = "note")]
    pub notes: Option<String>,
    #[serde(default, rename = "createdAt")]
    pub created_at: String,
    #[serde(default, rename = "updatedAt")]
    pub updated_at: String,
}

impl HostRecord {
    pub fn to_public(&self) -> PublicHost {
        PublicHost {
            id: self.id.clone(),
            alias: self.alias.clone(),
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            auth_type: self.auth_type.clone(),
            has_passphrase: self.passphrase.as_ref().map(|s| !s.is_empty()).unwrap_or(false),
            fingerprint: self.fingerprint.clone(),
            group_id: self.group_id.clone(),
            jump_host_id: self.jump_host_id.clone(),
            tags: self.tags.clone(),
            notes: self.notes.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicHost {
    pub id: String,
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(rename = "authType")]
    pub auth_type: AuthType,
    #[serde(rename = "hasPassphrase")]
    pub has_passphrase: bool,
    pub fingerprint: Option<String>,
    #[serde(rename = "groupId")]
    pub group_id: Option<String>,
    #[serde(rename = "jumpHostId")]
    pub jump_host_id: Option<String>,
    pub tags: Vec<String>,
    pub notes: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostGroup {
    pub id: String,
    pub name: String,
    #[serde(default, rename = "parentId")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub order: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub category: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, rename = "createdAt", skip_serializing_if = "String::is_empty")]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelRule {
    pub id: String,
    pub name: String,
    #[serde(rename = "hostId")]
    pub host_id: String,
    pub mode: String,
    #[serde(rename = "localHost")]
    pub local_host: String,
    #[serde(rename = "localPort")]
    pub local_port: u16,
    #[serde(rename = "remoteHost", skip_serializing_if = "Option::is_none")]
    pub remote_host: Option<String>,
    #[serde(rename = "remotePort", skip_serializing_if = "Option::is_none")]
    pub remote_port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(rename = "monitorIntervalMs")]
    pub monitor_interval_ms: u64,
    pub theme: String,
    #[serde(rename = "fontSize")]
    pub font_size: u8,
    #[serde(default, rename = "showHiddenFiles")]
    pub show_hidden_files: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            monitor_interval_ms: 2500,
            theme: "dark".to_string(),
            font_size: 14,
            show_hidden_files: false,
        }
    }
}
