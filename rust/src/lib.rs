//! TianShell 核心库：凭据、主机库、SSH/SFTP，以及不依赖 Node.js 的工作台和 MCP。

pub mod auth;
pub mod http_server;
pub mod hub;
pub mod mcp;
pub mod mcp_http;
pub mod monitor;
pub mod paths;
pub mod pty;
pub mod runtime;
pub mod sftp;
pub mod ssh;
pub mod state;
pub mod store;
pub mod tunnel;
pub mod types;
pub mod util;
pub mod vault;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
