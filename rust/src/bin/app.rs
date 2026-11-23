#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::time::Duration;
use tianshell_core::http_server::{start_workbench, ServerOptions};
use tianshell_core::VERSION;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    unsafe {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        extern "system" {
            fn AttachConsole(dwProcessId: u32) -> i32;
            fn SetCurrentProcessExplicitAppUserModelID(AppID: *const u16) -> i32;
        }
        AttachConsole(0xFFFFFFFF);
        let app_id: Vec<u16> = OsStr::new("TianShell.Workbench.Desktop.0.0.1")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let _ = SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr());
    }

    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h" || arg == "help") {
        print!("{}", help_text());
        let _ = std::io::Write::flush(&mut std::io::stdout());
        return Ok(());
    }
    let serve_only = args.iter().any(|arg| arg == "serve" || arg == "--serve" || arg == "--no-window");
    let no_open = args.iter().any(|arg| arg == "--no-open");

    #[cfg(target_os = "windows")]
    if !serve_only {
        if activate_existing_window() {
            return Ok(());
        }
    }
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
    ShowAndFocus,
    Minimize,
    Maximize,
    Close,
    Quit,
    Drag,
}

#[cfg(feature = "desktop")]
#[cfg(target_os = "windows")]
use std::sync::Mutex;

#[cfg(feature = "desktop")]
#[cfg(target_os = "windows")]
static GLOBAL_PROXY: Mutex<Option<tao::event_loop::EventLoopProxy<AppWindowEvent>>> = Mutex::new(None);
#[cfg(feature = "desktop")]
#[cfg(target_os = "windows")]
static GLOBAL_URL: Mutex<String> = Mutex::new(String::new());

#[cfg(feature = "desktop")]
fn run_window(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    use tao::dpi::LogicalSize;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::WindowBuilder;
    use wry::WebViewBuilder;

    let event_loop = EventLoopBuilder::<AppWindowEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    #[cfg(target_os = "windows")]
    {
        *GLOBAL_PROXY.lock().unwrap() = Some(proxy.clone());
        *GLOBAL_URL.lock().unwrap() = url.to_string();
    }

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

    #[cfg(target_os = "windows")]
    unsafe {
        use tao::platform::windows::WindowExtWindows;
        let hwnd = window.hwnd() as isize;
        extern "system" {
            fn GetWindowLongW(hWnd: isize, nIndex: i32) -> i32;
            fn SetWindowLongW(hWnd: isize, nIndex: i32, dwNewLong: i32) -> i32;
            fn SetWindowPos(hWnd: isize, hWndInsertAfter: isize, X: i32, Y: i32, cx: i32, cy: i32, uFlags: u32) -> i32;
            fn ShowWindow(hWnd: isize, nCmdShow: i32) -> i32;
            fn SetForegroundWindow(hWnd: isize) -> i32;
        }
        let ex_style = GetWindowLongW(hwnd, -20);
        let new_ex_style = (ex_style | 0x00040000) & !0x00000080;
        SetWindowLongW(hwnd, -20, new_ex_style);
        SetWindowPos(hwnd, 0, 0, 0, 0, 0, 0x0027);
        ShowWindow(hwnd, 5);
        SetForegroundWindow(hwnd);

        // 挂载 Windows 系统托盘（右下角后台常驻图标）
        setup_tray_icon(hwnd);
    }

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
                #[cfg(target_os = "windows")]
                unsafe {
                    remove_tray_icon();
                }
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(AppWindowEvent::ShowAndFocus) => {
                window.set_visible(true);
                window.set_minimized(false);
                window.set_focus();
                #[cfg(target_os = "windows")]
                unsafe {
                    use tao::platform::windows::WindowExtWindows;
                    let hwnd = window.hwnd() as isize;
                    extern "system" {
                        fn ShowWindow(hWnd: isize, nCmdShow: i32) -> i32;
                        fn SetForegroundWindow(hWnd: isize) -> i32;
                    }
                    ShowWindow(hwnd, 9); // SW_RESTORE
                    SetForegroundWindow(hwnd);
                }
            }
            Event::UserEvent(AppWindowEvent::Minimize) => {
                window.set_minimized(true);
            }
            Event::UserEvent(AppWindowEvent::Maximize) => {
                window.set_maximized(!window.is_maximized());
            }
            Event::UserEvent(AppWindowEvent::Close) => {
                // 点击右上角 ✕ 隐藏到系统托盘（后台常驻），通过托盘菜单「彻底退出」
                window.set_visible(false);
                #[cfg(target_os = "windows")]
                unsafe {
                    use tao::platform::windows::WindowExtWindows;
                    let hwnd = window.hwnd() as isize;
                    extern "system" {
                        fn ShowWindow(hWnd: isize, nCmdShow: i32) -> i32;
                    }
                    ShowWindow(hwnd, 0); // SW_HIDE
                }
            }
            Event::UserEvent(AppWindowEvent::Quit) => {
                #[cfg(target_os = "windows")]
                unsafe {
                    remove_tray_icon();
                }
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(AppWindowEvent::Drag) => {
                let _ = window.drag_window();
            }
            Event::LoopDestroyed => {
                #[cfg(target_os = "windows")]
                unsafe {
                    remove_tray_icon();
                }
            }
            _ => {}
        }
    });
}

#[cfg(target_os = "windows")]
fn activate_existing_window() -> bool {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    extern "system" {
        fn FindWindowW(lpClassName: *const u16, lpWindowName: *const u16) -> isize;
        fn ShowWindow(hWnd: isize, nCmdShow: i32) -> i32;
        fn SetForegroundWindow(hWnd: isize) -> i32;
        fn IsIconic(hWnd: isize) -> i32;
        fn BringWindowToTop(hWnd: isize) -> i32;
        fn SwitchToThisWindow(hWnd: isize, fAltTab: i32);
    }

    let titles = [
        "TianShell · 全功能终端工作台",
        "TianShell",
    ];

    for t in titles {
        let title: Vec<u16> = OsStr::new(t)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        unsafe {
            let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            if hwnd != 0 {
                if IsIconic(hwnd) != 0 {
                    ShowWindow(hwnd, 9); // SW_RESTORE
                } else {
                    ShowWindow(hwnd, 5); // SW_SHOW
                }
                BringWindowToTop(hwnd);
                SetForegroundWindow(hwnd);
                SwitchToThisWindow(hwnd, 1);
                return true;
            }
        }
    }
    false
}

#[cfg(target_os = "windows")]
const NIM_ADD: u32 = 0x00000000;
#[cfg(target_os = "windows")]
const NIM_DELETE: u32 = 0x00000002;

#[cfg(target_os = "windows")]
const NIF_MESSAGE: u32 = 0x00000001;
#[cfg(target_os = "windows")]
const NIF_ICON: u32 = 0x00000002;
#[cfg(target_os = "windows")]
const NIF_TIP: u32 = 0x00000004;

#[cfg(target_os = "windows")]
const WM_APP: u32 = 0x8000;
#[cfg(target_os = "windows")]
const WM_TRAY_CALLBACK: u32 = WM_APP + 42;

#[cfg(target_os = "windows")]
#[repr(C)]
struct NOTIFYICONDATAW {
    cb_size: u32,
    hwnd: isize,
    u_id: u32,
    u_flags: u32,
    u_callback_message: u32,
    h_icon: isize,
    sz_tip: [u16; 128],
    dw_state: u32,
    dw_state_mask: u32,
    sz_info: [u16; 256],
    u_timeout_or_version: u32,
    sz_info_title: [u16; 64],
    dw_info_flags: u32,
    guid_item: [u8; 16],
    h_balloon_icon: isize,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct POINT {
    x: i32,
    y: i32,
}

#[cfg(target_os = "windows")]
static OLD_WNDPROC: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
#[cfg(target_os = "windows")]
static GLOBAL_NID: Mutex<Option<NOTIFYICONDATAW>> = Mutex::new(None);

#[cfg(target_os = "windows")]
unsafe extern "system" fn tray_wnd_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    extern "system" {
        fn CallWindowProcW(lpPrevWndFunc: isize, hWnd: isize, Msg: u32, wParam: usize, lParam: isize) -> isize;
    }

    if msg == WM_TRAY_CALLBACK {
        let mouse_msg = (lparam & 0xFFFF) as u32;
        if mouse_msg == 0x0202 || mouse_msg == 0x0203 {
            // WM_LBUTTONUP (0x0202) or WM_LBUTTONDBLCLK (0x0203) -> 唤醒置顶窗口
            if let Some(proxy) = GLOBAL_PROXY.lock().unwrap().as_ref() {
                let _ = proxy.send_event(AppWindowEvent::ShowAndFocus);
            }
            return 0;
        } else if mouse_msg == 0x0205 {
            // WM_RBUTTONUP (0x0205) -> 弹出右键托盘菜单
            let url = GLOBAL_URL.lock().unwrap().clone();
            show_tray_popup_menu(hwnd, &url);
            return 0;
        }
    } else if msg == 0x0002 {
        // WM_DESTROY
        remove_tray_icon();
    }
    let prev = OLD_WNDPROC.load(std::sync::atomic::Ordering::SeqCst);
    CallWindowProcW(prev, hwnd, msg, wparam, lparam)
}

#[cfg(target_os = "windows")]
unsafe fn show_tray_popup_menu(hwnd: isize, url: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    extern "system" {
        fn CreatePopupMenu() -> isize;
        fn DestroyMenu(hMenu: isize) -> i32;
        fn AppendMenuW(hMenu: isize, uFlags: u32, uIDNewItem: usize, lpNewItem: *const u16) -> i32;
        fn TrackPopupMenu(hMenu: isize, uFlags: u32, x: i32, y: i32, nReserved: i32, hWnd: isize, prcRect: *const std::ffi::c_void) -> i32;
        fn GetCursorPos(lpPoint: *mut POINT) -> i32;
        fn SetForegroundWindow(hWnd: isize) -> i32;
        fn PostMessageW(hWnd: isize, Msg: u32, wParam: usize, lParam: isize) -> i32;
        fn SetMenuDefaultItem(hMenu: isize, uItem: u32, fByPos: u32) -> i32;
    }

    let hmenu = CreatePopupMenu();
    if hmenu == 0 {
        return;
    }

    let to_wide = |s: &str| -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    };

    let title_show = to_wide("显示工作台");
    let title_browser = to_wide("在浏览器中打开");
    let title_exit = to_wide("彻底退出 TianShell");

    // 0 = MF_STRING, 0x0800 = MF_SEPARATOR
    AppendMenuW(hmenu, 0, 101, title_show.as_ptr());
    AppendMenuW(hmenu, 0, 102, title_browser.as_ptr());
    AppendMenuW(hmenu, 0x0800, 0, std::ptr::null());
    AppendMenuW(hmenu, 0, 103, title_exit.as_ptr());

    // 粗体默认项
    SetMenuDefaultItem(hmenu, 101, 0);

    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);

    // 必须在 TrackPopupMenu 之前调用 SetForegroundWindow，否则点击外部无法取消菜单
    SetForegroundWindow(hwnd);

    // 0x0100 = TPM_RETURNCMD, 0x0002 = TPM_RIGHTBUTTON
    let cmd = TrackPopupMenu(hmenu, 0x0102, pt.x, pt.y, 0, hwnd, std::ptr::null());
    PostMessageW(hwnd, 0, 0, 0);
    DestroyMenu(hmenu);

    match cmd {
        101 => {
            if let Some(proxy) = GLOBAL_PROXY.lock().unwrap().as_ref() {
                let _ = proxy.send_event(AppWindowEvent::ShowAndFocus);
            }
        }
        102 => {
            open_browser(url);
        }
        103 => {
            if let Some(proxy) = GLOBAL_PROXY.lock().unwrap().as_ref() {
                let _ = proxy.send_event(AppWindowEvent::Quit);
            }
        }
        _ => {}
    }
}

#[cfg(target_os = "windows")]
unsafe fn setup_tray_icon(hwnd: isize) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    extern "system" {
        fn Shell_NotifyIconW(dwMessage: u32, lpData: *const NOTIFYICONDATAW) -> i32;
        fn GetWindowLongPtrW(hWnd: isize, nIndex: i32) -> isize;
        fn SetWindowLongPtrW(hWnd: isize, nIndex: i32, dwNewLong: isize) -> isize;
    }

    let h_icon = get_or_create_app_icon(hwnd);

    let mut nid = NOTIFYICONDATAW {
        cb_size: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hwnd,
        u_id: 1001,
        u_flags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        u_callback_message: WM_TRAY_CALLBACK,
        h_icon,
        sz_tip: [0u16; 128],
        dw_state: 0,
        dw_state_mask: 0,
        sz_info: [0u16; 256],
        u_timeout_or_version: 0,
        sz_info_title: [0u16; 64],
        dw_info_flags: 0,
        guid_item: [0u8; 16],
        h_balloon_icon: 0,
    };

    let tip = "TianShell · 全功能终端工作台";
    let tip_wide: Vec<u16> = OsStr::new(tip).encode_wide().collect();
    for (i, &c) in tip_wide.iter().enumerate().take(127) {
        nid.sz_tip[i] = c;
    }

    let res = Shell_NotifyIconW(NIM_ADD, &nid);
    if res == 0 {
        nid.cb_size = 504;
        let res2 = Shell_NotifyIconW(NIM_ADD, &nid);
        if res2 == 0 {
            nid.cb_size = 952;
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        }
    }
    *GLOBAL_NID.lock().unwrap() = Some(nid);

    // Subclass 窗口过程以接收 WM_TRAY_CALLBACK
    let prev = GetWindowLongPtrW(hwnd, -4 /* GWLP_WNDPROC */);
    if prev != 0 {
        OLD_WNDPROC.store(prev, std::sync::atomic::Ordering::SeqCst);
        SetWindowLongPtrW(hwnd, -4, tray_wnd_proc as *const () as isize);
    }
}

#[cfg(target_os = "windows")]
unsafe fn remove_tray_icon() {
    extern "system" {
        fn Shell_NotifyIconW(dwMessage: u32, lpData: *const NOTIFYICONDATAW) -> i32;
    }
    if let Some(nid) = GLOBAL_NID.lock().unwrap().take() {
        Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

#[cfg(target_os = "windows")]
unsafe fn get_or_create_app_icon(hwnd: isize) -> isize {
    extern "system" {
        fn SendMessageW(hWnd: isize, Msg: u32, wParam: usize, lParam: isize) -> isize;
        fn GetClassLongPtrW(hWnd: isize, nIndex: i32) -> isize;
        fn GetDC(hWnd: isize) -> isize;
        fn ReleaseDC(hWnd: isize, hDC: isize) -> i32;
        fn CreateDIBSection(hdc: isize, pbmi: *const BITMAPINFOHEADER, usage: u32, ppvBits: *mut *mut u8, hSection: isize, offset: u32) -> isize;
        fn CreateBitmap(nWidth: i32, nHeight: i32, nPlanes: u32, nBitCount: u32, lpBits: *const u8) -> isize;
        fn CreateIconIndirect(piconinfo: *const ICONINFO) -> isize;
        fn DeleteObject(ho: isize) -> i32;
        fn LoadIconW(hInstance: isize, lpIconName: *const u16) -> isize;
    }

    #[repr(C)]
    struct BITMAPINFOHEADER {
        bi_size: u32,
        bi_width: i32,
        bi_height: i32,
        bi_planes: u16,
        bi_bit_count: u16,
        bi_compression: u32,
        bi_size_image: u32,
        bi_xpels_per_meter: i32,
        bi_ypels_per_meter: i32,
        bi_clr_used: u32,
        bi_clr_important: u32,
    }

    #[repr(C)]
    struct ICONINFO {
        f_icon: i32,
        x_hotspot: u32,
        y_hotspot: u32,
        hbm_mask: isize,
        hbm_color: isize,
    }

    let h = SendMessageW(hwnd, 0x007F /* WM_GETICON */, 0 /* ICON_SMALL */, 0);
    if h != 0 {
        return h;
    }
    let h = SendMessageW(hwnd, 0x007F, 1 /* ICON_BIG */, 0);
    if h != 0 {
        return h;
    }
    let h = GetClassLongPtrW(hwnd, -14 /* GCLP_HICON */);
    if h != 0 {
        return h;
    }

    use tianshell_core::icon_data::{ICON_32_HEIGHT, ICON_32_RGBA, ICON_32_WIDTH};
    let width = ICON_32_WIDTH as i32;
    let height = ICON_32_HEIGHT as i32;

    let bmi = BITMAPINFOHEADER {
        bi_size: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        bi_width: width,
        bi_height: -height, // top-down DIB
        bi_planes: 1,
        bi_bit_count: 32,
        bi_compression: 0, // BI_RGB
        bi_size_image: (width * height * 4) as u32,
        bi_xpels_per_meter: 0,
        bi_ypels_per_meter: 0,
        bi_clr_used: 0,
        bi_clr_important: 0,
    };

    let mut bits: *mut u8 = std::ptr::null_mut();
    let hdc = GetDC(0);
    let hbm_color = CreateDIBSection(hdc, &bmi, 0, &mut bits, 0, 0);
    ReleaseDC(0, hdc);

    if hbm_color != 0 && !bits.is_null() {
        for (i, chunk) in ICON_32_RGBA.chunks_exact(4).enumerate() {
            let r = chunk[0];
            let g = chunk[1];
            let b = chunk[2];
            let a = chunk[3];
            let offset = i * 4;
            std::ptr::write(bits.add(offset), b);
            std::ptr::write(bits.add(offset + 1), g);
            std::ptr::write(bits.add(offset + 2), r);
            std::ptr::write(bits.add(offset + 3), a);
        }

        let hbm_mask = CreateBitmap(width, height, 1, 1, std::ptr::null());
        let info = ICONINFO {
            f_icon: 1,
            x_hotspot: 0,
            y_hotspot: 0,
            hbm_mask,
            hbm_color,
        };

        let icon = CreateIconIndirect(&info);
        DeleteObject(hbm_color);
        DeleteObject(hbm_mask);
        if icon != 0 {
            return icon;
        }
    }

    LoadIconW(0, 32512 as *const u16)
}
