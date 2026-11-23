#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::time::Duration;
use tianshell_core::http_server::{start_workbench, ServerOptions};
use tianshell_core::VERSION;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    unsafe {
        extern "system" {
            fn AttachConsole(dwProcessId: u32) -> i32;
        }
        AttachConsole(0xFFFFFFFF);
    }

    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h" || arg == "help") {
        print!("{}", help_text());
        let _ = std::io::Write::flush(&mut std::io::stdout());
        return Ok(());
    }
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

fn help_text() -> String {
    format!(
        "\
TianShell {version}

用法:
  tianshell                  打开桌面工作台（默认带原生窗口）
  tianshell serve            只在浏览器中打开，不创建原生窗口
  tianshell --no-window      同 serve
  tianshell --no-open        不自动打开浏览器
  tianshell --port <端口>    指定监听端口，默认 53921
  tianshell --help           显示这份说明并退出，不启动工作台

工作台只监听 127.0.0.1。MCP 随工作台一起启动。
说明见 docs/mcp.md。令牌不会写在启动日志里。
",
        version = VERSION
    )
}

fn banner(running: &tianshell_core::http_server::RunningServer) -> String {
    format!(
        "TianShell {version} 工作台已启动（Rust，不需要 Node.js）\n界面  http://127.0.0.1:{port}/\nMCP   {mcp}   Streamable HTTP，随工作台自动启动\nSSE   {sse}   旧版 HTTP+SSE\n数据  {data}\n连接  打开界面后点左下角「连接 MCP」。令牌不写在这行日志里。\n说明  docs/mcp.md",
        version = VERSION,
        port = running.port,
        mcp = running.mcp_url,
        sse = running.sse_url,
        data = running.data_dir.display()
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
#[derive(Debug, Clone, Copy)]
enum AppWindowEvent {
    Minimize,
    Maximize,
    Close,
    Drag,
}

#[cfg(feature = "desktop")]
fn run_window(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    use tao::dpi::LogicalSize;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::WindowBuilder;
    use wry::WebViewBuilder;

    let event_loop = EventLoopBuilder::<AppWindowEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let mut window_builder = WindowBuilder::new()
        .with_title("TianShell · 全功能终端工作台")
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(LogicalSize::new(960.0, 600.0))
        .with_decorations(false)
        .with_resizable(true);

    // 注入应用图标 (Windows 任务栏与 Alt+Tab 即时生效)
    {
        use tao::window::Icon;
        use tianshell_core::icon_data::{ICON_32_HEIGHT, ICON_32_RGBA, ICON_32_WIDTH};
        if let Ok(icon) = Icon::from_rgba(ICON_32_RGBA.to_vec(), ICON_32_WIDTH, ICON_32_HEIGHT) {
            window_builder = window_builder.with_window_icon(Some(icon));
        }
    }

    #[cfg(target_os = "windows")]
    {
        use tao::platform::windows::WindowBuilderExtWindows;
        window_builder = window_builder.with_undecorated_shadow(true);
    }

    let window = window_builder.build(&event_loop)?;

    let _webview = WebViewBuilder::new()
        .with_url(url)
        .with_ipc_handler(move |req| {
            let msg = req.body().trim();
            match msg {
                "min" => {
                    let _ = proxy.send_event(AppWindowEvent::Minimize);
                }
                "max" => {
                    let _ = proxy.send_event(AppWindowEvent::Maximize);
                }
                "close" => {
                    let _ = proxy.send_event(AppWindowEvent::Close);
                }
                "drag" => {
                    let _ = proxy.send_event(AppWindowEvent::Drag);
                }
                _ => {}
            }
        })
        .build(&window)?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(AppWindowEvent::Minimize) => {
                window.set_minimized(true);
            }
            Event::UserEvent(AppWindowEvent::Maximize) => {
                window.set_maximized(!window.is_maximized());
            }
            Event::UserEvent(AppWindowEvent::Close) => {
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(AppWindowEvent::Drag) => {
                let _ = window.drag_window();
            }
            _ => {}
        }
    });
}
