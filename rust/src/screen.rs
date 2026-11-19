//! A small terminal transcript.
//!
//! This is not a full emulator. It keeps the recent visible text a coding tool
//! can read, plus an append-only log used to notice when a command finished.
//! Cursor-addressing sequences are ignored. Carriage return, newline, and
//! backspace are applied so a redrawn prompt does not bury the line.

use std::collections::VecDeque;

const MAX_LINES: usize = 400;
const MAX_LINE: usize = 512;
const MAX_LOG: usize = 180_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Posix,
    PowerShell,
}

impl ShellKind {
    pub fn local_default() -> Self {
        if cfg!(windows) {
            Self::PowerShell
        } else {
            Self::Posix
        }
    }

    pub fn trailer(self, nonce: &str) -> String {
        match self {
            Self::Posix => format!(
                "__ts_ec=$?; printf '\\n__TS_END_{nonce}__ ec=%s cwd=%s\\n' \"$__ts_ec\" \"$PWD\""
            ),
            Self::PowerShell => format!(
                "$global:__ts_ec = if ($null -ne $LASTEXITCODE) {{ $LASTEXITCODE }} elseif ($?) {{ 0 }} else {{ 1 }}; Write-Output \"__TS_END_{nonce}__ ec=$global:__ts_ec cwd=$((Get-Location).Path)\""
            ),
        }
    }
}

#[derive(Debug)]
pub struct Screen {
    lines: VecDeque<String>,
    row: usize,
    col: usize,
    log: String,
    log_base: u64,
    written: u64,
    mode: EscMode,
    esc: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscMode {
    Normal,
    Esc,
    Csi,
    Osc,
}

impl Screen {
    pub fn new() -> Self {
        let mut lines = VecDeque::new();
        lines.push_back(String::new());
        Self {
            lines,
            row: 0,
            col: 0,
            log: String::new(),
            log_base: 0,
            written: 0,
            mode: EscMode::Normal,
            esc: Vec::new(),
        }
    }

    pub fn mark(&self) -> u64 {
        self.written
    }

    pub fn since(&self, mark: u64) -> (String, bool) {
        if mark < self.log_base {
            return (self.log.clone(), true);
        }
        let start = (mark - self.log_base) as usize;
        if start >= self.log.len() {
            return (String::new(), false);
        }
        (self.log[start..].to_string(), false)
    }

    pub fn text(&self, max_chars: usize) -> (String, bool) {
        let joined = self
            .lines
            .iter()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        let count = joined.chars().count();
        if max_chars == 0 || count <= max_chars {
            return (joined, false);
        }
        let tail: String = joined.chars().skip(count - max_chars).collect();
        (tail, true)
    }

    pub fn ingest(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.push_byte(byte);
        }
    }

    fn push_byte(&mut self, byte: u8) {
        match self.mode {
            EscMode::Normal => {
                if byte == 0x1b {
                    self.mode = EscMode::Esc;
                    self.esc.clear();
                } else {
                    self.visible(byte);
                }
            }
            EscMode::Esc => match byte {
                b'[' => self.mode = EscMode::Csi,
                b']' => self.mode = EscMode::Osc,
                _ => self.mode = EscMode::Normal,
            },
            EscMode::Csi => {
                self.esc.push(byte);
                if (0x40..=0x7e).contains(&byte) {
                    let seq = self.esc.clone();
                    self.esc.clear();
                    self.mode = EscMode::Normal;
                    self.apply_csi(&seq);
                } else if self.esc.len() > 32 {
                    self.esc.clear();
                    self.mode = EscMode::Normal;
                }
            }
            EscMode::Osc => {
                if byte == 0x07 || byte == 0x1b {
                    self.mode = EscMode::Normal;
                    self.esc.clear();
                }
            }
        }
    }

    fn apply_csi(&mut self, seq: &[u8]) {
        let Some((&final_byte, params)) = seq.split_last() else {
            return;
        };
        let numbers = parse_params(params);
        match final_byte {
            b'A' => self.move_row(-(numbers.first().copied().unwrap_or(1).max(1) as isize)),
            b'B' => self.move_row(numbers.first().copied().unwrap_or(1).max(1) as isize),
            b'C' => self.col = self.col.saturating_add(numbers.first().copied().unwrap_or(1).max(1) as usize),
            b'D' => self.col = self.col.saturating_sub(numbers.first().copied().unwrap_or(1).max(1) as usize),
            b'G' => self.col = numbers.first().copied().unwrap_or(1).saturating_sub(1) as usize,
            b'H' | b'f' => {
                let row = numbers.first().copied().unwrap_or(1).saturating_sub(1) as usize;
                let col = numbers.get(1).copied().unwrap_or(1).saturating_sub(1) as usize;
                self.row = row.min(self.lines.len().saturating_sub(1));
                self.col = col;
            }
            b'K' => self.erase_line(numbers.first().copied().unwrap_or(0)),
            b'J' => {
                if numbers.first().copied().unwrap_or(0) == 2 {
                    self.lines.clear();
                    self.lines.push_back(String::new());
                    self.row = 0;
                    self.col = 0;
                }
            }
            _ => {}
        }
    }

    fn move_row(&mut self, delta: isize) {
        let next = self.row as isize + delta;
        if next < 0 {
            self.row = 0;
        } else {
            self.row = next as usize;
            while self.row >= self.lines.len() {
                self.lines.push_back(String::new());
            }
            self.trim_lines();
        }
    }

    fn erase_line(&mut self, mode: i64) {
        let col = self.col;
        let chars: Vec<char> = self.current().chars().collect();
        let col = col.min(chars.len());
        let next = match mode {
            1 => chars.into_iter().skip(col).collect(),
            2 => String::new(),
            _ => chars.into_iter().take(col).collect(),
        };
        *self.current() = next;
    }

    fn visible(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.push_log_char('\n');
                self.row += 1;
                while self.row >= self.lines.len() {
                    self.lines.push_back(String::new());
                }
                self.col = 0;
                self.trim_lines();
            }
            b'\r' => self.col = 0,
            0x7f | 0x08 => {
                if self.col > 0 {
                    self.col -= 1;
                    let col = self.col;
                    let mut chars: Vec<char> = self.current().chars().collect();
                    if col < chars.len() {
                        chars.remove(col);
                        *self.current() = chars.into_iter().collect();
                    }
                    self.pop_log_char();
                }
            }
            b'\t' => {
                let spaces = 8 - (self.col % 8);
                for _ in 0..spaces {
                    self.put_char(' ');
                }
            }
            byte if byte < 0x20 => {}
            byte => self.put_char(byte as char),
        }
    }

    fn put_char(&mut self, ch: char) {
        if self.col >= MAX_LINE {
            return;
        }
        let col = self.col;
        let mut chars: Vec<char> = self.current().chars().collect();
        while chars.len() < col {
            chars.push(' ');
        }
        if col < chars.len() {
            chars[col] = ch;
        } else {
            chars.push(ch);
        }
        *self.current() = chars.into_iter().collect();
        self.col += 1;
        self.push_log_char(ch);
    }

    fn current(&mut self) -> &mut String {
        let row = self.row.min(self.lines.len().saturating_sub(1));
        self.row = row;
        &mut self.lines[row]
    }

    fn trim_lines(&mut self) {
        while self.lines.len() > MAX_LINES {
            self.lines.pop_front();
            self.row = self.row.saturating_sub(1);
        }
    }

    fn push_log_char(&mut self, ch: char) {
        self.log.push(ch);
        self.written += ch.len_utf8() as u64;
        if self.log.len() > MAX_LOG {
            let extra = self.log.len() - MAX_LOG / 2;
            let mut cut = extra;
            while cut < self.log.len() && !self.log.is_char_boundary(cut) {
                cut += 1;
            }
            self.log_base += cut as u64;
            self.log.drain(..cut);
        }
    }

    fn pop_log_char(&mut self) {
        if self.log.ends_with('\n') || self.log.is_empty() {
            return;
        }
        if let Some(ch) = self.log.pop() {
            self.written = self.written.saturating_sub(ch.len_utf8() as u64);
        }
    }
}

impl Default for Screen {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_params(bytes: &[u8]) -> Vec<i64> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(bytes);
    text.split(';').filter_map(|part| part.parse::<i64>().ok()).collect()
}

#[derive(Debug)]
pub struct LineAssembler {
    buf: String,
}

impl LineAssembler {
    pub fn new() -> Self {
        Self { buf: String::new() }
    }

    pub fn feed(&mut self, data: &str) -> Vec<String> {
        let mut submitted = Vec::new();
        let mut chars = data.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\u{1b}' {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                } else if chars.peek() == Some(&']') {
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\u{7}' {
                            break;
                        }
                    }
                }
                continue;
            }
            match ch {
                '\r' | '\n' => {
                    let line = std::mem::take(&mut self.buf);
                    let trimmed = line.trim();
                    if !trimmed.is_empty() && !is_trailer_line(trimmed) {
                        submitted.push(trimmed.to_string());
                    }
                }
                '\u{7f}' | '\u{8}' => {
                    self.buf.pop();
                }
                '\u{3}' | '\u{15}' => self.buf.clear(),
                c if c.is_control() => {}
                c => {
                    if self.buf.chars().count() < 8000 {
                        self.buf.push(c);
                    }
                }
            }
        }
        submitted
    }
}

impl Default for LineAssembler {
    fn default() -> Self {
        Self::new()
    }
}

pub fn is_trailer_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains("__TS_END_")
        || trimmed.contains("__TS_CWD__")
        || trimmed.starts_with("__ts_ec=")
        || trimmed.contains("$global:__ts_ec")
}

pub fn parse_end_marker(text: &str, nonce: &str) -> Option<(i32, String)> {
    let key = format!("__TS_END_{nonce}__ ec=");
    let idx = text.rfind(&key)?;
    let rest = &text[idx + key.len()..];
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let exit_code: i32 = rest[..digits].parse().ok()?;
    let after = rest[digits..].trim_start();
    let cwd = after.strip_prefix("cwd=")?.lines().next()?.trim();
    if cwd.is_empty() {
        return None;
    }
    Some((exit_code, cwd.to_string()))
}

pub fn parse_exec_cwd(stderr: &str) -> Option<String> {
    let key = "__TS_CWD__";
    let idx = stderr.rfind(key)?;
    let cwd = stderr[idx + key.len()..].lines().next()?.trim();
    if cwd.is_empty() {
        None
    } else {
        Some(cwd.to_string())
    }
}

pub fn transcript_before_marker(text: &str, nonce: &str) -> String {
    let key = format!("__TS_END_{nonce}__ ec=");
    let cut = text.rfind(&key).unwrap_or(text.len());
    let head = &text[..cut];
    head.lines()
        .filter(|line| !is_trailer_line(line))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn note_cd(cwd: &mut Option<String>, command: &str) {
    let trimmed = command.trim();
    let rest = if let Some(rest) = trimmed.strip_prefix("cd ") {
        rest.trim()
    } else if trimmed == "cd" || trimmed.starts_with("Set-Location") {
        *cwd = None;
        return;
    } else {
        return;
    };
    if rest.contains("&&") || rest.contains('|') || rest.contains(';') || rest.contains('\n') {
        return;
    }
    let arg = rest.split_whitespace().next().unwrap_or("");
    let arg = arg.trim_matches('"').trim_matches('\'');
    if arg.is_empty() || arg == "-" || arg == "~" || arg == "~/" {
        *cwd = None;
        return;
    }
    if arg.starts_with('/') || is_windows_abs(arg) {
        *cwd = Some(arg.to_string());
        return;
    }
    if let Some(base) = cwd.clone() {
        *cwd = Some(join_cwd(&base, arg));
    }
}

fn is_windows_abs(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn join_cwd(base: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = if base.starts_with('/') {
        base.split('/').filter(|p| !p.is_empty()).collect()
    } else {
        base.split(|c| c == '/' || c == '\\').filter(|p| !p.is_empty()).collect()
    };
    for part in rel.split(|c| c == '/' || c == '\\') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            parts.pop();
        } else {
            parts.push(part);
        }
    }
    if base.starts_with('/') {
        format!("/{}", parts.join("/"))
    } else {
        parts.join(if base.contains('\\') { "\\" } else { "/" })
    }
}

pub fn wrap_exec(command: &str) -> String {
    format!("{command}\n__ts_ec=$?; printf '\\n__TS_CWD__%s\\n' \"$PWD\" >&2; exit $__ts_ec")
}

pub fn strip_exec_cwd(stderr: &str) -> String {
    stderr
        .lines()
        .filter(|line| !line.contains("__TS_CWD__"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_keeps_visible_text_and_ignores_color() {
        let mut screen = Screen::new();
        screen.ingest(b"hello\r\n\x1b[31mred\x1b[0m");
        let (text, _) = screen.text(1000);
        assert!(text.contains("hello"), "{text}");
        assert!(text.contains("red"), "{text}");
        assert!(!text.contains("[31"), "{text}");
        screen.ingest(b"\r\nab\x08c");
        let (text, _) = screen.text(1000);
        assert!(text.contains("ac"), "{text}");
    }

    #[test]
    fn marker_parse_ignores_the_echoed_format_string() {
        let nonce = "ab12";
        let echoed = ShellKind::Posix.trailer(nonce);
        let output = format!("{echoed}\n__TS_END_{nonce}__ ec=0 cwd=/tmp/work\n");
        assert!(parse_end_marker(&echoed, nonce).is_none());
        let (code, cwd) = parse_end_marker(&output, nonce).unwrap();
        assert_eq!(code, 0);
        assert_eq!(cwd, "/tmp/work");
        let transcript = transcript_before_marker(&output, nonce);
        assert!(!transcript.contains("__TS_END_"));
        assert!(is_trailer_line(&echoed));
    }

    #[test]
    fn assembler_submits_a_line_and_skips_the_trailer() {
        let mut asm = LineAssembler::new();
        assert!(asm.feed("ec").is_empty());
        assert!(asm.feed("ho hi").is_empty());
        let lines = asm.feed("\r");
        assert_eq!(lines, vec!["echo hi".to_string()]);
        let trailer = format!("{}\r", ShellKind::Posix.trailer("zz"));
        assert!(asm.feed(&trailer).is_empty());
        let mut wiped = LineAssembler::new();
        wiped.feed("rm");
        wiped.feed("\u{7f}");
        wiped.feed("\u{7f}");
        assert_eq!(wiped.feed("\r"), Vec::<String>::new());
    }

    #[test]
    fn cd_updates_a_guess_and_exec_wrapper_reports_cwd() {
        let mut cwd = Some("/tmp".to_string());
        note_cd(&mut cwd, "cd work");
        assert_eq!(cwd.as_deref(), Some("/tmp/work"));
        note_cd(&mut cwd, "echo hi");
        assert_eq!(cwd.as_deref(), Some("/tmp/work"));
        let wrapped = wrap_exec("uname -a");
        assert!(wrapped.contains("exit $__ts_ec"));
        let stderr = "warn\n__TS_CWD__/srv/app\n";
        assert_eq!(parse_exec_cwd(stderr).as_deref(), Some("/srv/app"));
        assert!(!strip_exec_cwd(stderr).contains("__TS_CWD__"));
        assert!(ShellKind::PowerShell.trailer("n").contains("LASTEXITCODE"));
    }
}
