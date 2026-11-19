use std::io::Read;
use std::time::{Duration, Instant};
use tianshell_core::monitor::parse_metrics_sample;
use tianshell_core::pty::LocalShellSession;

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

#[test]
fn test_local_shell_echo_and_crlf() {
    let (mut session, mut reader) = LocalShellSession::spawn(None).unwrap();
    assert!(session.is_alive());

    session.write(b"echo TIAN_RUST_OK\r\n").unwrap();

    let start = Instant::now();
    let mut out = Vec::new();
    let mut buf = [0u8; 1024];

    while start.elapsed() < Duration::from_secs(5) {
        if let Ok(n) = reader.read(&mut buf) {
            if n > 0 {
                out.extend_from_slice(&buf[..n]);
                let s = String::from_utf8_lossy(&out);
                if s.contains("TIAN_RUST_OK") {
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let result_str = String::from_utf8_lossy(&out);
    assert!(result_str.contains("TIAN_RUST_OK"), "Shell did not echo expected string: {}", result_str);

    session.kill().unwrap();
}

#[test]
fn local_shell_is_a_pty_and_accepts_resize() {
    let (mut session, mut reader) = LocalShellSession::spawn(None).unwrap();
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

    let start = Instant::now();
    let mut out = Vec::new();
    let mut buf = [0u8; 1024];
    while start.elapsed() < Duration::from_secs(12) {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                let text = strip_ansi(&String::from_utf8_lossy(&out));
                let ready = if cfg!(windows) {
                    text.lines().any(|line| line.trim() == "PTYOK")
                } else {
                    text.lines().any(|line| line.trim() == "HAS_TTY") && text.contains("12 40")
                };
                if ready {
                    break;
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(30)),
        }
    }
    let text = strip_ansi(&String::from_utf8_lossy(&out));
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
