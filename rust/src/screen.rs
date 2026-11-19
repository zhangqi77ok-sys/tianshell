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

    /// First extra line after the user's command.
    ///
    /// POSIX captures `$?`, turns echo off, and emits a short OSC so the
    /// follow-up can be typed while the terminal is not echoing. PowerShell
    /// reports the exit code on this same line; there is no second write.
    pub fn trailer(self, nonce: &str) -> String {
        match self {
            Self::Posix => format!("ec=$?;stty -echo;printf '\\033]9;Q{nonce}\\a'"),
            Self::PowerShell => format!(
                "$global:__ts_ec = if ($null -ne $LASTEXITCODE) {{ $LASTEXITCODE }} elseif ($?) {{ 0 }} else {{ 1 }}; Write-Output \"__TS_END_{nonce}__ ec=$global:__ts_ec cwd=$((Get-Location).Path)\""
            ),
        }
    }

    /// Typed only after the quiet OSC, while echo is off, so it never appears.
    /// The printf clears the spare prompt, emits the exit marker, then restores echo.
    pub fn probe_followup(self, nonce: &str) -> Option<String> {
        match self {
            Self::Posix => Some(format!(
                "printf '\\r\\033[K\\033]1337;__TS_END_{nonce}__ ec=%s cwd=%s\\007' \"$ec\" \"$PWD\";stty echo"
            )),
            Self::PowerShell => None,
        }
    }

    pub fn line_needles(self) -> Vec<String> {
        match self {
            Self::Posix => Vec::new(),
            Self::PowerShell => vec!["$global:__ts_ec".to_string()],
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
    OscSt,
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
                if byte == 0x07 {
                    self.finish_osc();
                    self.mode = EscMode::Normal;
                } else if byte == 0x1b {
                    self.mode = EscMode::OscSt;
                } else {
                    self.esc.push(byte);
                    if self.esc.len() > 4096 {
                        self.esc.clear();
                        self.mode = EscMode::Normal;
                    }
                }
            }
            EscMode::OscSt => {
                self.finish_osc();
                self.mode = EscMode::Normal;
                if byte != b'\\' {
                    self.push_byte(byte);
                }
            }
        }
    }

    fn finish_osc(&mut self) {
        let payload = String::from_utf8_lossy(&self.esc).into_owned();
        self.esc.clear();
        if payload.contains("__TS_END_") {
            self.push_log_char('\n');
            for ch in payload.chars() {
                self.push_log_char(ch);
            }
            self.push_log_char('\n');
        }
    }

    /// Drop a probe command that was echoed onto a visible line.
    pub fn scrub_probe(&mut self, quiet: &str) {
        if quiet.is_empty() {
            return;
        }
        for line in &mut self.lines {
            if line.contains(quiet) {
                *line = line.replace(quiet, "");
            }
            if is_trailer_line(line) {
                line.clear();
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
        || trimmed.contains("__ts_ec")
        || trimmed.contains("$global:__ts_ec")
        || trimmed.contains("stty -echo;printf")
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
    let quiet = ShellKind::Posix.trailer(nonce);
    let head = text[..cut].replace(&quiet, "");
    head.lines()
        .map(|line| line.trim_end())
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

/// Hides the exit-code probe from bytes that are about to be drawn.
///
/// Armed while a command is running:
/// - drop the exact quiet-line echo and the line break the tty adds after it
/// - drop OSC payloads that carry our nonce (the quiet signal and the marker)
/// - drop a visible `__TS_END_` line (PowerShell writes the marker as text)
/// - hold only a suffix that could still grow into one of those, so ordinary
///   output is forwarded immediately
///
/// When the quiet OSC arrives, `push` returns the follow-up command. That
/// command is typed with echo off, so it never joins the prompt.
#[derive(Debug)]
pub struct ProbeConceal {
    armed: bool,
    nonce: String,
    quiet: Vec<u8>,
    marker: Vec<u8>,
    line_needles: Vec<Vec<u8>>,
    follow: Option<String>,
    follow_sent: bool,
    buf: Vec<u8>,
}

impl ProbeConceal {
    pub fn new() -> Self {
        Self {
            armed: false,
            nonce: String::new(),
            quiet: Vec::new(),
            marker: Vec::new(),
            line_needles: Vec::new(),
            follow: None,
            follow_sent: false,
            buf: Vec::new(),
        }
    }

    pub fn arm(
        &mut self,
        nonce: &str,
        quiet: &str,
        follow: Option<String>,
        line_needles: &[String],
    ) {
        *self = Self::new();
        self.armed = true;
        self.nonce = nonce.to_string();
        self.quiet = quiet.as_bytes().to_vec();
        self.marker = format!("__TS_END_{nonce}__").into_bytes();
        self.line_needles = line_needles
            .iter()
            .map(|item| item.as_bytes().to_vec())
            .collect();
        self.follow = follow;
    }

    pub fn quiet_text(&self) -> String {
        String::from_utf8_lossy(&self.quiet).into_owned()
    }

    pub fn armed(&self) -> bool {
        self.armed
    }

    /// The quiet line has already turned echo off. A cancel has to turn it back on.
    pub fn follow_was_sent(&self) -> bool {
        self.follow_sent
    }

    /// Stop filtering. A probe line still in the buffer is dropped, not drawn.
    pub fn finish(&mut self) -> Vec<u8> {
        if !self.armed {
            return Vec::new();
        }
        self.strip_complete();
        if let Some(pos) = self.earliest_probe() {
            self.buf.truncate(pos);
        }
        self.armed = false;
        self.follow_sent = false;
        std::mem::take(&mut self.buf)
    }

    fn earliest_probe(&self) -> Option<usize> {
        let mut found = find_subslice(&self.buf, &self.marker);
        if !self.quiet.is_empty() {
            found = earlier(found, find_subslice(&self.buf, &self.quiet));
        }
        for needle in &self.line_needles {
            found = earlier(found, find_subslice(&self.buf, needle));
        }
        found
    }

    pub fn push(&mut self, data: &[u8]) -> (Vec<u8>, Option<String>) {
        if !self.armed {
            return (data.to_vec(), None);
        }
        self.buf.extend_from_slice(data);
        let follow = self.take_followup();
        self.strip_complete();
        let hold = self.hold_len();
        let emit_end = self.buf.len().saturating_sub(hold);
        let visible = self.buf.drain(..emit_end).collect();
        (visible, follow)
    }

    fn take_followup(&mut self) -> Option<String> {
        if self.follow_sent {
            return None;
        }
        let follow = self.follow.clone()?;
        let key = format!("Q{}", self.nonce);
        if osc_payloads(&self.buf).any(|payload| payload.contains(&key)) {
            self.follow_sent = true;
            return Some(follow);
        }
        None
    }

    fn strip_complete(&mut self) {
        if !self.quiet.is_empty() {
            while let Some(pos) = find_subslice(&self.buf, &self.quiet) {
                let mut end = pos + self.quiet.len();
                if self.buf.get(end) == Some(&b'\r') {
                    end += 1;
                }
                if self.buf.get(end) == Some(&b'\n') {
                    end += 1;
                }
                self.buf.drain(pos..end);
            }
        }
        strip_nonce_oscs(&mut self.buf, &self.nonce);
        for needle in self.line_needles.clone() {
            while let Some(pos) = find_subslice(&self.buf, &needle) {
                let Some(rel) = self.buf[pos..].iter().position(|byte| *byte == b'\n') else {
                    break;
                };
                self.buf.drain(pos..pos + rel + 1);
            }
        }
        while let Some(pos) = find_subslice(&self.buf, &self.marker) {
            let Some(rel) = self.buf[pos..].iter().position(|byte| *byte == b'\n') else {
                break;
            };
            self.buf.drain(pos..pos + rel + 1);
        }
    }

    fn hold_len(&self) -> usize {
        if let Some(pos) = find_subslice(&self.buf, &self.marker) {
            if !self.buf[pos..].contains(&b'\n') {
                return self.buf.len() - pos;
            }
        }
        for needle in &self.line_needles {
            if let Some(pos) = find_subslice(&self.buf, needle) {
                if !self.buf[pos..].contains(&b'\n') {
                    return self.buf.len() - pos;
                }
            }
        }
        if let Some(esc) = incomplete_escape_start(&self.buf) {
            return self.buf.len() - esc;
        }
        let mut hold = suffix_prefix_len(&self.buf, &self.quiet);
        hold = hold.max(suffix_prefix_len(&self.buf, &self.marker));
        for needle in &self.line_needles {
            hold = hold.max(suffix_prefix_len(&self.buf, needle));
        }
        hold
    }
}

impl Default for ProbeConceal {
    fn default() -> Self {
        Self::new()
    }
}

fn earlier(left: Option<usize>, right: Option<usize>) -> Option<usize> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len())
        .position(|window| window == needle)
}

fn suffix_prefix_len(hay: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || hay.is_empty() {
        return 0;
    }
    let max = needle.len().saturating_sub(1).min(hay.len());
    for len in (1..=max).rev() {
        if hay[hay.len() - len..] == needle[..len] {
            return len;
        }
    }
    0
}

fn incomplete_escape_start(buf: &[u8]) -> Option<usize> {
    let start = buf.iter().rposition(|byte| *byte == 0x1b)?;
    let tail = &buf[start..];
    if tail.len() == 1 {
        return Some(start);
    }
    match tail[1] {
        b'[' => {
            let done = tail[2..].iter().any(|byte| (0x40..=0x7e).contains(byte));
            if done {
                None
            } else {
                Some(start)
            }
        }
        b']' => {
            let done = tail.contains(&0x07) || tail.windows(2).any(|pair| pair == [0x1b, b'\\']);
            if done {
                None
            } else {
                Some(start)
            }
        }
        _ => None,
    }
}

fn osc_payloads(buf: &[u8]) -> impl Iterator<Item = String> + '_ {
    OscIter { buf, index: 0 }
}

struct OscIter<'a> {
    buf: &'a [u8],
    index: usize,
}

impl Iterator for OscIter<'_> {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        while self.index + 1 < self.buf.len() {
            if self.buf[self.index] == 0x1b && self.buf[self.index + 1] == b']' {
                let start = self.index + 2;
                let mut end = start;
                while end < self.buf.len() {
                    if self.buf[end] == 0x07 {
                        let payload = String::from_utf8_lossy(&self.buf[start..end]).into_owned();
                        self.index = end + 1;
                        return Some(payload);
                    }
                    if self.buf[end] == 0x1b
                        && end + 1 < self.buf.len()
                        && self.buf[end + 1] == b'\\'
                    {
                        let payload = String::from_utf8_lossy(&self.buf[start..end]).into_owned();
                        self.index = end + 2;
                        return Some(payload);
                    }
                    end += 1;
                }
                return None;
            }
            self.index += 1;
        }
        None
    }
}

fn strip_nonce_oscs(buf: &mut Vec<u8>, nonce: &str) {
    if nonce.is_empty() {
        return;
    }
    let mut index = 0;
    while index + 1 < buf.len() {
        if buf[index] == 0x1b && buf[index + 1] == b']' {
            let start = index + 2;
            let mut end = start;
            let mut term = None;
            while end < buf.len() {
                if buf[end] == 0x07 {
                    term = Some(end + 1);
                    break;
                }
                if buf[end] == 0x1b && end + 1 < buf.len() && buf[end + 1] == b'\\' {
                    term = Some(end + 2);
                    break;
                }
                end += 1;
            }
            let Some(term) = term else {
                return;
            };
            let payload = String::from_utf8_lossy(&buf[start..end]);
            if payload.contains(nonce) {
                buf.drain(index..term);
                continue;
            }
            index = term;
            continue;
        }
        index += 1;
    }
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
        let mut screen = Screen::new();
        let osc = format!("\u{1b}]1337;__TS_END_{nonce}__ ec=0 cwd=/tmp/work\u{7}");
        screen.ingest(format!("{echoed}\n").as_bytes());
        screen.ingest(osc.as_bytes());
        let (raw, _) = screen.since(0);
        assert!(parse_end_marker(&echoed, nonce).is_none());
        let (code, cwd) = parse_end_marker(&raw, nonce).unwrap();
        assert_eq!(code, 0);
        assert_eq!(cwd, "/tmp/work");
        let transcript = transcript_before_marker(&raw, nonce);
        assert!(!transcript.contains("__TS_END_"), "{transcript}");
        assert!(!transcript.contains("stty"), "{transcript}");
        assert!(is_trailer_line(&echoed));
        screen.scrub_probe(&echoed);
        let (visible, _) = screen.text(4000);
        assert!(!visible.contains("stty"), "{visible}");
        assert!(!visible.contains("__TS_END_"), "{visible}");
    }

    #[test]
    fn conceal_strips_the_probe_and_keeps_the_command() {
        let nonce = "abcd1234";
        let shell = ShellKind::Posix;
        let mut conceal = ProbeConceal::new();
        conceal.arm(
            nonce,
            &shell.trailer(nonce),
            shell.probe_followup(nonce),
            &shell.line_needles(),
        );
        let echoed = format!("$ echo hi\r\nhi\r\n$ {}\r\n", shell.trailer(nonce));
        let (visible, follow) = conceal.push(echoed.as_bytes());
        assert!(follow.is_none(), "quiet OSC has not arrived");
        let quiet = format!("\u{1b}]9;Q{nonce}\u{7}");
        let (more, follow) = conceal.push(quiet.as_bytes());
        let follow = follow.expect("follow-up");
        assert!(follow.contains(&format!("__TS_END_{nonce}__")), "{follow}");
        assert!(follow.contains("stty echo"), "{follow}");
        let marker = format!("\u{1b}]1337;__TS_END_{nonce}__ ec=0 cwd=/tmp/work\u{7}$ ");
        let (tail, again) = conceal.push(marker.as_bytes());
        assert!(again.is_none());
        let mut shown = visible;
        shown.extend(more);
        shown.extend(tail);
        shown.extend(conceal.finish());
        let text = String::from_utf8_lossy(&shown);
        assert!(text.contains("echo hi"), "{text}");
        assert!(text.contains("hi"), "{text}");
        assert!(!text.contains("stty"), "{text}");
        assert!(!text.contains("__TS_END_"), "{text}");
        assert!(!text.contains(&nonce), "{text}");
        assert!(
            text.trim_end().ends_with('$') || text.contains("$ "),
            "{text}"
        );
    }

    #[test]
    fn powershell_redraw_after_the_marker_stays_hidden() {
        let nonce = "59e148db";
        let shell = ShellKind::PowerShell;
        let mut conceal = ProbeConceal::new();
        conceal.arm(nonce, &shell.trailer(nonce), None, &shell.line_needles());
        let first = format!("tianshell-probe-ok\r\n__TS_END_{nonce}__ ec=0 cwd=C:\\Users\\runneradmin\r\nPS C:\\Users\\runneradmin> ");
        let (visible, _) = conceal.push(first.as_bytes());
        assert!(conceal.armed(), "the redraw arrives after the command has finished");
        let redraw = format!(
            "\u{1b}[92m$global:__ts_ec \u{1b}[90m= if \u{1b}[36m\"__TS_END_{nonce}__ ec=\u{1b}[m__TS_END_{nonce}__ ec=0 cwd=C:\\Users\\runneradmin\r\nPS C:\\Users\\runneradmin> "
        );
        let (again, _) = conceal.push(redraw.as_bytes());
        let mut shown = visible;
        shown.extend(again);
        let text = String::from_utf8_lossy(&shown);
        assert!(text.contains("tianshell-probe-ok"), "{text:?}");
        assert!(!text.contains("__TS_END_"), "{text:?}");
        assert!(!text.contains("__ts_ec"), "{text:?}");
        assert!(text.contains("PS C:\\Users\\runneradmin>"), "{text:?}");
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
