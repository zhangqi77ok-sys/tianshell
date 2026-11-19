use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use wry::WebViewBuilder;

fn spawn_backend_if_needed() -> Option<Child> {
    // 检查本地 53901 端口是否已有运行的服务
    if std::net::TcpStream::connect("127.0.0.1:53901").is_ok() {
        return None;
    }
    let mut cmd = Command::new("node");
    cmd.args(["dist/server.js", "--no-open"]);
    cmd.env("TIANSHELL_PORT", "53901");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    cmd.spawn().ok()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut backend_child = spawn_backend_if_needed();
    if backend_child.is_some() {
        // 等待服务端口就绪
        for _ in 0..30 {
            if std::net::TcpStream::connect("127.0.0.1:53901").is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("TianShell · 全功能终端工作台")
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(LogicalSize::new(960.0, 600.0))
        .build(&event_loop)?;

    let app_url = "http://127.0.0.1:53901/";
    let _webview = WebViewBuilder::new()
        .with_url(app_url)
        .build(&window)?;

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
