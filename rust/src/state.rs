use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use crate::hub::SessionHub;
use crate::mcp::McpHandler;
use crate::store::{HostStore, SettingsStore, SnippetStore, TunnelStore};
use crate::tunnel::TunnelManager;

#[derive(Clone)]
pub struct AppState {
    pub token: String,
    pub data_dir: PathBuf,
    pub port: Arc<AtomicU16>,
    pub hosts: Arc<Mutex<HostStore>>,
    pub snippets: Arc<Mutex<SnippetStore>>,
    pub tunnel_rules: Arc<Mutex<TunnelStore>>,
    pub settings: Arc<Mutex<SettingsStore>>,
    pub hub: Arc<SessionHub>,
    pub tunnels: Arc<TunnelManager>,
    pub mcp: Arc<McpHandler>,
    pub transfers: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    pub sse: Arc<SseHub>,
    pub mcp_sessions: Arc<Mutex<std::collections::HashSet<String>>>,
}

#[derive(Clone, Default)]
pub struct SseHub {
    inner: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<String>>>>,
}

impl SseHub {
    pub fn insert(&self, id: String, sender: mpsc::UnboundedSender<String>) {
        self.inner.lock().unwrap_or_else(|err| err.into_inner()).insert(id, sender);
    }

    pub fn remove(&self, id: &str) {
        self.inner.lock().unwrap_or_else(|err| err.into_inner()).remove(id);
    }

    pub fn send(&self, id: &str, message: String) -> bool {
        let guard = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        guard.get(id).map(|sender| sender.send(message).is_ok()).unwrap_or(false)
    }
}

pub fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}
