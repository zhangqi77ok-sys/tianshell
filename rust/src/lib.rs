//! TianShell 核心库：凭据、主机库、SSH/SFTP，以及不依赖 Node.js 的工作台和 MCP。

pub mod auth;
pub mod bindings;
pub mod download_grant;
pub mod hands;
pub mod history;
pub mod http_server;
pub mod hub;
pub mod keys;
pub mod mcp;
pub mod mcp_config;
pub mod mcp_http;
pub mod monitor;
pub mod paths;
pub mod pty;
pub mod redact;
pub mod runtime;
pub mod screen;
pub mod sftp;
pub mod shared_ssh;
pub mod ssh;
pub mod state;
pub mod store;
pub mod terminal;
pub mod tunnel;
pub mod types;
pub mod util;
pub mod vault;
pub mod icon_data;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
