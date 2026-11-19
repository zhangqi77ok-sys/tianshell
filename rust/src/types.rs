use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthType {
    Password,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(rename = "jumpHostId", skip_serializing_if = "Option::is_none")]
    pub jump_host_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(rename = "knownFingerprint", skip_serializing_if = "Option::is_none")]
    pub known_fingerprint: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostGroup {
    pub id: String,
    pub name: String,
    #[serde(rename = "parentId", skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    pub category: String,
    pub command: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelKind {
    Local,
    Remote,
    Socks5,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelConfig {
    pub id: String,
    pub name: String,
    #[serde(rename = "hostId")]
    pub host_id: String,
    pub kind: TunnelKind,
    #[serde(rename = "localPort")]
    pub local_port: u16,
    #[serde(rename = "remoteHost", skip_serializing_if = "Option::is_none")]
    pub remote_host: Option<String>,
    #[serde(rename = "remotePort", skip_serializing_if = "Option::is_none")]
    pub remote_port: Option<u16>,
    #[serde(rename = "autoStart", default)]
    pub auto_start: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(rename = "monitorIntervalMs")]
    pub monitor_interval_ms: u64,
    pub theme: String,
    #[serde(rename = "fontSize")]
    pub font_size: u8,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            monitor_interval_ms: 2000,
            theme: "dark".to_string(),
            font_size: 14,
        }
    }
}
