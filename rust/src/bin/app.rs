use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use wry::WebViewBuilder;

const BACKEND_ADDR: &str = "127.0.0.1:53901";
const BACKEND_URL: &str = "http://127.0.0.1:53901/";

fn backend_ready() -> bool {
    let addr: SocketAddr = BACKEND_ADDR.parse().expect("static listen address");
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

fn find_server_js() -> Option<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        let mut hops = 0;
        while let Some(current) = dir {
            roots.push(current.clone());
            dir = current.parent().map(|p| p.to_path_buf());
            hops += 1;
            if hops >= 8 {
                break;
            }
        }
    }
    for root in roots {
        let candidate = root.join("dist").join("server.js");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn spawn_backend_if_needed() -> Result<Option<Child>, String> {
    if backend_ready() {
        return Ok(None);
    }
    let script = find_server_js().ok_or_else(|| {
        "没有找到 dist/server.js。请先在仓库根目录执行 npm install 和 npm run build。".to_string()
    })?;
    let cwd = script
        .parent()
        .and_then(|p| p.parent())
        .ok_or_else(|| "无法确定仓库根目录".to_string())?;
    let mut child = Command::new("node")
        .arg(&script)
        .arg("--no-open")
        .env("TIANSHELL_PORT", "53901")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("无法启动 Node 后端（需要本机已安装 Node.js 22+）：{err}"))?;

    for _ in 0..50 {
        if backend_ready() {
            return Ok(Some(child));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    Err("工作台后端启动超时。请确认 Node.js 可用，并且已经执行过 npm run build。".to_string())
}

fn error_page(message: &str) -> String {
    let escaped = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
  <head>
    <meta charset="utf-8" />
    <title>TianShell</title>
    <style>
      body {{ font-family: sans-serif; background: #1b2228; color: #e8eef2; margin: 0; padding: 48px; }}
      main {{ max-width: 640px; line-height: 1.6; }}
      h1 {{ font-size: 20px; font-weight: 600; }}
      p {{ color: #c5d0d6; }}
    </style>
  </head>
  <body>
    <main>
      <h1>TianShell 工作台没有启动起来</h1>
      <p>{escaped}</p>
    </main>
  </body>
</html>"#
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let backend = spawn_backend_if_needed();
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("TianShell · 全功能终端工作台")
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(LogicalSize::new(960.0, 600.0))
        .build(&event_loop)?;

    let mut backend_child = None;
    let mut builder = WebViewBuilder::new();
    match backend {
        Ok(child) => {
            backend_child = child;
            builder = builder.with_url(BACKEND_URL);
        }
        Err(message) => {
            eprintln!("{message}");
            builder = builder.with_html(error_page(&message));
        }
    }
    let _webview = builder.build(&window)?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            if let Some(ref mut child) = backend_child {
                let _ = child.kill();
            }
            *control_flow = ControlFlow::Exit;
        }
    });
}
