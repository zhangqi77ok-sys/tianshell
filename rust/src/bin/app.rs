use std::env;
use std::time::Duration;
use tianshell_core::http_server::{start_workbench, ServerOptions};
use tianshell_core::VERSION;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let serve_only = args.iter().any(|arg| arg == "serve" || arg == "--serve" || arg == "--no-window");
    let no_open = args.iter().any(|arg| arg == "--no-open");
    let mut port = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--port" {
            port = args.get(index + 1).and_then(|value| value.parse().ok());
            index += 2;
            continue;
        }
        index += 1;
    }
    let running = start_workbench(ServerOptions {
        data_dir: None,
        port,
        open_browser: false,
    })?;
    eprintln!("{}", banner(&running));
    let use_window = cfg!(feature = "desktop") && !serve_only;
    if use_window {
        #[cfg(feature = "desktop")]
        {
            if let Err(err) = run_window(&running.url) {
                eprintln!("原生窗口没有打开：{err}");
                eprintln!("可以改用浏览器打开上面的地址。");
                if !no_open {
                    open_browser(&running.url);
                }
                wait_for_ctrl_c();
            }
        }
    } else {
        if !no_open {
            open_browser(&running.url);
        }
        wait_for_ctrl_c();
    }
    Ok(())
}

fn banner(running: &tianshell_core::http_server::RunningServer) -> String {
    format!(
        "TianShell {VERSION} 工作台已启动（Rust，不需要 Node.js）\n界面  {}\nMCP   {}   Streamable HTTP，随工作台自动启动\nSSE   {}   旧版 HTTP+SSE\n数据  {}\n说明  docs/mcp.md",
        running.url, running.mcp_url, running.sse_url, running.data_dir.display()
    )
}

fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(["/c", "start", "", url]).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

fn wait_for_ctrl_c() {
    let (tx, rx) = std::sync::mpsc::channel();
    if ctrlc::set_handler(move || {
        let _ = tx.send(());
    })
    .is_err()
    {
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
    let _ = rx.recv();
}

#[cfg(feature = "desktop")]
fn run_window(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    use tao::dpi::LogicalSize;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoop};
    use tao::window::WindowBuilder;
    use wry::WebViewBuilder;

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("TianShell · 全功能终端工作台")
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(LogicalSize::new(960.0, 600.0))
        .build(&event_loop)?;
    let _webview = WebViewBuilder::new().with_url(url).build(&window)?;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
}
