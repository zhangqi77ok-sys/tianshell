use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tianshell_core::monitor::parse_metrics_sample;
use tianshell_core::pty::LocalShellSession;
use tianshell_core::screen::quick_command_input;

/// 提示符已经出现，并且这一小段时间没有新输出，才说明 shell 开始读键盘。
/// 这不是启动前的固定等待：输出还在涨时计时会重新开始。
const STARTUP_QUIET: Duration = Duration::from_millis(200);

fn strip_ansi(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != 0x1b {
            out.push(bytes[index] as char);
            index += 1;
            continue;
        }
        index += 1;
        if index < bytes.len() && bytes[index] == b'[' {
            index += 1;
            while index < bytes.len() && !bytes[index].is_ascii_alphabetic() {
                index += 1;
            }
            if index < bytes.len() {
                index += 1;
            }
            continue;
        }
        if index < bytes.len() && bytes[index] == b']' {
            index += 1;
            while index < bytes.len() && bytes[index] != 0x07 && bytes[index] != 0x1b {
                index += 1;
            }
            if index < bytes.len() {
                index += 1;
            }
        }
    }
    out
}

/// 读线程单独阻塞在 PTY 上，测试线程才能一边看输出一边守住截止时间。
struct CapturedPty {
    text: Arc<Mutex<String>>,
}

impl CapturedPty {
    fn start(mut reader: Box<dyn Read + Send>) -> Self {
        let text = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&text);
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&buf[..n]);
                        sink.lock().expect("pty capture").push_str(&chunk);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });
        Self { text }
    }

    fn snapshot(&self) -> String {
        self.text.lock().expect("pty capture").clone()
    }

    /// 等到提示符出现且输出停稳。截止时间到了就返回当时的内容，由调用方断言。
    fn wait_until_ready(&self, budget: Duration) -> String {
        let start = Instant::now();
        let mut last_len = 0usize;
        let mut last_change = Instant::now();
        loop {
            let text = self.snapshot();
            if text.len() != last_len {
                last_len = text.len();
                last_change = Instant::now();
            }
            if shell_prompt_ready(&text) && last_change.elapsed() >= STARTUP_QUIET {
                return text;
            }
            if start.elapsed() >= budget {
                return text;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_until(&self, budget: Duration, done: impl Fn(&str) -> bool) -> String {
        let start = Instant::now();
        loop {
            let text = self.snapshot();
            if done(&text) || start.elapsed() >= budget {
                return text;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

fn shell_prompt_ready(text: &str) -> bool {
    strip_ansi(text).split(|c| c == '\n' || c == '\r').any(prompt_line)
}

fn prompt_line(line: &str) -> bool {
    let line = line.trim();
    if line.is_empty() || line.len() > 120 {
        return false;
    }
    if line.starts_with("PS ") && line.ends_with('>') {
        return true;
    }
    match line.chars().last() {
        Some('$' | '#' | '%' | '❯' | '›') => true,
        Some('>') => line == ">" || line.ends_with(" >"),
        _ => false,
    }
}

#[test]
fn powershell_startup_banner_is_a_prompt_and_command_text_is_not() {
    let flake = "\u{1b}[?9001h\u{1b}[?1004h\u{1b}[?25l\u{1b}[2J\u{1b}[m\u{1b}[H\u{1b}]0;Administrator: C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\u{7}\u{1b}[?25hPS C:\\Users\\runneradmin>";
    assert!(shell_prompt_ready(flake));
    assert!(shell_prompt_ready("PS C:\\>"));
    assert!(shell_prompt_ready("$ "));
    assert!(shell_prompt_ready("user@host:~$ "));
    assert!(shell_prompt_ready(">"));
    assert!(!shell_prompt_ready(""));
    assert!(!shell_prompt_ready("\u{1b}[2J\u{1b}[H"));
    assert!(!shell_prompt_ready("TIAN_RUST_OK\r\n"));
    assert!(!shell_prompt_ready("echo hello>\r\n"));
}

#[test]
fn test_local_shell_echo_and_crlf() {
    let (mut session, reader) = LocalShellSession::spawn(None).unwrap();
    assert!(session.is_alive());
    let captured = CapturedPty::start(reader);

    let ready = captured.wait_until_ready(Duration::from_secs(8));
    assert!(shell_prompt_ready(&ready), "shell never became ready: {ready}");

    session.write(b"echo TIAN_RUST_OK\r\n").unwrap();
    let echoed = captured.wait_until(Duration::from_secs(5), |text| text.contains("TIAN_RUST_OK"));
    assert!(
        echoed.contains("TIAN_RUST_OK"),
        "Shell did not echo expected string: {echoed}"
    );

    session.kill().unwrap();
}

#[test]
fn local_shell_is_a_pty_and_accepts_resize() {
    let (mut session, reader) = LocalShellSession::spawn(None).unwrap();
    let captured = CapturedPty::start(reader);
    let ready = captured.wait_until_ready(Duration::from_secs(8));
    assert!(shell_prompt_ready(&ready), "shell never became ready: {ready}");
    session.resize(40, 12).unwrap();
    // The success token must not appear as a literal in the command. Windows
    // PowerShell reprints the line with color codes, and that echo used to
    // satisfy a search for HAS_TTY before the command actually ran.
    let probe: &[u8] = if cfg!(windows) {
        b"$m='PTY'+$(if([Console]::IsInputRedirected){'PIPE'}else{'OK'});Write-Output $m\r\n"
    } else {
        b"if test -t 0; then echo HAS_TTY; else echo NO_TTY; fi\nstty size\n"
    };
    session.write(probe).unwrap();

    let out = captured.wait_until(Duration::from_secs(8), |raw| {
        let text = strip_ansi(raw);
        if cfg!(windows) {
            text.lines().any(|line| line.trim() == "PTYOK")
        } else {
            text.lines().any(|line| line.trim() == "HAS_TTY") && text.contains("12 40")
        }
    });
    let text = strip_ansi(&out);
    let lines: Vec<&str> = text.split(|c| c == '\n' || c == '\r').map(str::trim).filter(|line| !line.is_empty()).collect();
    if cfg!(windows) {
        assert!(lines.iter().any(|line| *line == "PTYOK"), "local shell is not a tty: {text}");
        assert!(!lines.iter().any(|line| *line == "PTYPIPE"), "{text}");
    } else {
        assert!(lines.iter().any(|line| *line == "HAS_TTY"), "local shell is not a tty: {text}");
        assert!(!lines.iter().any(|line| *line == "NO_TTY"), "{text}");
        assert!(lines.iter().any(|line| *line == "12 40"), "resize was not applied: {text}");
    }
    assert!(!text.to_ascii_lowercase().contains("can't access tty"), "{text}");
    assert!(!text.to_ascii_lowercase().contains("not a tty"), "{text}");
    session.kill().unwrap();
}

/// 把终端重绘折成行。成功断言看的是执行结果那一行，不能只看见送出去的命令文本。
fn folded_lines(raw: &str) -> Vec<String> {
    let text = strip_ansi(raw);
    let mut lines = vec![String::new()];
    let mut col = 0usize;
    for ch in text.chars() {
        match ch {
            '\n' => {
                lines.push(String::new());
                col = 0;
            }
            '\r' => col = 0,
            '\u{8}' | '\u{7f}' => {
                if col > 0 {
                    col -= 1;
                }
            }
            c if c.is_control() => {}
            c => {
                let line = lines.last_mut().expect("line");
                let mut chars: Vec<char> = line.chars().collect();
                if col < chars.len() {
                    chars[col] = c;
                } else {
                    while chars.len() < col {
                        chars.push(' ');
                    }
                    chars.push(c);
                }
                *line = chars.into_iter().collect();
                col += 1;
            }
        }
    }
    lines.into_iter().map(|line| line.trim().to_string()).filter(|line| !line.is_empty()).collect()
}

fn marker_command(powershell: bool, marker: &str) -> String {
    let (head, tail) = marker.split_once(':').expect("marker");
    if powershell {
        format!("Write-Output ('{head}:' + '{tail}')")
    } else {
        format!("printf '{head}:%s\\n' {tail}")
    }
}

fn replace_pending_line(session: &mut LocalShellSession, captured: &CapturedPty, at_end: bool, pending: &str, marker: &str) {
    let powershell = cfg!(windows);
    let command = marker_command(powershell, marker);
    assert!(!command.contains(marker), "the marker must be produced by running the command");
    let mut bytes = format!("echo {pending}").into_bytes();
    if !at_end {
        let steps = pending.len();
        if powershell {
            for _ in 0..steps {
                bytes.extend_from_slice(b"\x1b[D");
            }
        } else {
            bytes.extend(std::iter::repeat(0x02u8).take(steps));
        }
    }
    bytes.extend(quick_command_input(&command, powershell).into_bytes());
    session.write(&bytes).unwrap();
    let out = captured.wait_until(Duration::from_secs(8), |raw| folded_lines(raw).iter().any(|line| line == marker));
    let lines = folded_lines(&out);
    assert!(
        lines.iter().any(|line| line == marker),
        "quick command did not run by itself ({pending}, end={at_end}): {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line == pending),
        "pending text was executed ({pending}, end={at_end}): {lines:?}"
    );
}

#[test]
fn quick_command_replaces_pending_line_at_end_and_middle() {
    let (mut session, reader) = LocalShellSession::spawn(None).unwrap();
    let captured = CapturedPty::start(reader);
    let ready = captured.wait_until_ready(Duration::from_secs(8));
    assert!(shell_prompt_ready(&ready), "shell never became ready: {ready}");
    replace_pending_line(&mut session, &captured, false, "PENDINGMID", "RAN:MIDOK");
    replace_pending_line(&mut session, &captured, true, "PENDINGEND", "RAN:ENDOK");
    session.kill().unwrap();
}

#[test]
fn test_parse_metrics_sample() {
    let raw = r#"
__HOST__
ubuntu-server
__UPTIME__
3600.5 7200.0
__LOAD__
0.15 0.20 0.18 1/200 1234
__MEM__
MemTotal:        4096000 kB
MemAvailable:    2048000 kB
MemFree:         1024000 kB
SwapTotal:       1048576 kB
SwapFree:        1048576 kB
__DF__
Filesystem     1K-blocks     Used Available Use% Mounted on
/dev/sda1       20971520 10485760  10485760  50% /
__PS__
  PID USER     %CPU %MEM COMMAND
  101 root      2.5  1.0 systemd
"#;
    let metrics = parse_metrics_sample(raw);
    assert_eq!(metrics.hostname, "ubuntu-server");
    assert_eq!(metrics.uptime_seconds, 3600);
    assert_eq!(metrics.load_avg[0], 0.15);
    assert_eq!(metrics.mem_total_kb, 4096000);
    assert_eq!(metrics.mem_used_kb, 2048000);
    assert_eq!(metrics.swap_total_kb, 1048576);
    assert_eq!(metrics.swap_used_kb, 0);
    assert_eq!(metrics.disks.len(), 1);
    assert_eq!(metrics.disks[0].mount, "/");
    assert_eq!(metrics.disks[0].use_percent, 50.0);
    assert_eq!(metrics.processes.len(), 1);
    assert_eq!(metrics.processes[0].pid, 101);
}
