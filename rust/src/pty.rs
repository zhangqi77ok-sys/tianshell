use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("本地终端 IO 错误: {0}")]
    Io(#[from] std::io::Error),
}

pub struct CrLfNormalizer {
    prev_was_cr: bool,
}

impl CrLfNormalizer {
    pub fn new() -> Self {
        Self { prev_was_cr: false }
    }

    pub fn normalize(&mut self, chunk: &[u8]) -> Vec<u8> {
        if chunk.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(chunk.len() + 16);
        for &b in chunk {
            if b == b'\n' && !self.prev_was_cr {
                out.push(b'\r');
            }
            out.push(b);
            self.prev_was_cr = b == b'\r';
        }
        out
    }
}

pub struct NormalizedReader<R: Read> {
    inner: R,
    normalizer: CrLfNormalizer,
}

impl<R: Read> NormalizedReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            normalizer: CrLfNormalizer::new(),
        }
    }
}

impl<R: Read> Read for NormalizedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut tmp = vec![0u8; buf.len().max(512)];
        let n = self.inner.read(&mut tmp)?;
        if n == 0 {
            return Ok(0);
        }
        let norm = self.normalizer.normalize(&tmp[..n]);
        let copy_len = norm.len().min(buf.len());
        buf[..copy_len].copy_from_slice(&norm[..copy_len]);
        Ok(copy_len)
    }
}

pub struct LocalShellSession {
    child: Child,
    stdin: ChildStdin,
}

impl LocalShellSession {
    pub fn spawn(cwd: Option<&str>) -> Result<(Self, NormalizedReader<ChildStdout>), PtyError> {
        let is_win = cfg!(target_os = "windows");
        let shell = if is_win { "powershell.exe" } else { "sh" };
        let mut cmd = Command::new(shell);
        if is_win {
            cmd.args([
                "-NoLogo",
                "-NoExit",
                "-Command",
                "[Console]::InputEncoding=[System.Text.Encoding]::UTF8; [Console]::OutputEncoding=[System.Text.Encoding]::UTF8",
            ]);
        } else {
            cmd.arg("-i");
        }
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        } else if let Some(home) = dirs::home_dir() {
            cmd.current_dir(home);
        }

        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "无法连接终端 stdin")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "无法连接终端 stdout")
        })?;

        let reader = NormalizedReader::new(stdout);
        let session = Self { child, stdin };
        Ok((session, reader))
    }

    pub fn write(&mut self, data: &[u8]) -> Result<(), PtyError> {
        self.stdin.write_all(data)?;
        self.stdin.flush()?;
        Ok(())
    }

    pub fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            _ => false,
        }
    }

    pub fn kill(&mut self) -> Result<(), PtyError> {
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }
}
