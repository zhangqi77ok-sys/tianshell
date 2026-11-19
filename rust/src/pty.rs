use std::io::{Read, Write};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("本地终端 IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("本地终端错误: {0}")]
    Message(String),
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
    pending: Vec<u8>,
}

impl<R: Read> NormalizedReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            normalizer: CrLfNormalizer::new(),
            pending: Vec::new(),
        }
    }
}

impl<R: Read> Read for NormalizedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if !self.pending.is_empty() {
            let n = buf.len().min(self.pending.len());
            buf[..n].copy_from_slice(&self.pending[..n]);
            self.pending.drain(..n);
            return Ok(n);
        }
        let mut tmp = vec![0u8; buf.len().max(512)];
        let n = self.inner.read(&mut tmp)?;
        if n == 0 {
            return Ok(0);
        }
        let norm = self.normalizer.normalize(&tmp[..n]);
        if norm.len() <= buf.len() {
            buf[..norm.len()].copy_from_slice(&norm);
            Ok(norm.len())
        } else {
            buf.copy_from_slice(&norm[..buf.len()]);
            self.pending.extend_from_slice(&norm[buf.len()..]);
            Ok(buf.len())
        }
    }
}

pub struct LocalShellSession {
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
}

impl LocalShellSession {
    pub fn spawn(cwd: Option<&str>) -> Result<(Self, Box<dyn Read + Send>), PtyError> {
        Self::open(cwd, 80, 24)
    }

    pub fn open(cwd: Option<&str>, cols: u32, rows: u32) -> Result<(Self, Box<dyn Read + Send>), PtyError> {
        let system = native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows: dim(rows),
                cols: dim(cols),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| PtyError::Message(err.to_string()))?;
        let mut cmd = shell_command();
        if let Some(dir) = cwd {
            cmd.cwd(dir);
        } else if let Some(home) = dirs::home_dir() {
            cmd.cwd(home);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|err| PtyError::Message(err.to_string()))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| PtyError::Message(err.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|err| PtyError::Message(err.to_string()))?;
        Ok((
            Self {
                child,
                writer,
                master: pair.master,
            },
            reader,
        ))
    }

    pub fn resize(&mut self, cols: u32, rows: u32) -> Result<(), PtyError> {
        self.master
            .resize(PtySize {
                rows: dim(rows),
                cols: dim(cols),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| PtyError::Message(err.to_string()))
    }

    pub fn write(&mut self, data: &[u8]) -> Result<(), PtyError> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            _ => false,
        }
    }

    pub fn kill(&mut self) -> Result<(), PtyError> {
        let _ = self.writer.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }
}

fn dim(value: u32) -> u16 {
    value.clamp(1, 512) as u16
}

fn shell_command() -> CommandBuilder {
    if cfg!(target_os = "windows") {
        let mut cmd = CommandBuilder::new("powershell.exe");
        cmd.args([
            "-NoLogo",
            "-NoExit",
            "-Command",
            "[Console]::InputEncoding=[System.Text.Encoding]::UTF8; [Console]::OutputEncoding=[System.Text.Encoding]::UTF8",
        ]);
        cmd
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut cmd = CommandBuilder::new(shell);
        cmd.arg("-i");
        if std::env::var_os("TERM").is_none() {
            cmd.env("TERM", "xterm-256color");
        }
        cmd
    }
}
