use std::io::{Read, Write};
use std::path::Path;
use serde::{Deserialize, Serialize};
use ssh2::Session;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SftpEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: u64,
    #[serde(rename = "mtimeMs")]
    pub mtime_ms: u64,
    pub mode: u32,
    pub permissions: String,
}

#[derive(Debug, Error)]
pub enum SftpError {
    #[error("SFTP 引擎错误: {0}")]
    Ssh(#[from] ssh2::Error),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("文本解码错误: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

fn format_unix_mode(mode: u32) -> String {
    let ftype = if (mode & 0o170000) == 0o040000 {
        'd'
    } else if (mode & 0o170000) == 0o120000 {
        'l'
    } else {
        '-'
    };
    let bits = [
        0o400, 0o200, 0o100, 0o040, 0o020, 0o010, 0o004, 0o002, 0o001,
    ];
    let mut perms = String::with_capacity(10);
    perms.push(ftype);
    for (i, bit) in bits.iter().enumerate() {
        if mode & bit != 0 {
            let rwx = ['r', 'w', 'x'];
            perms.push(rwx[i % 3]);
        } else {
            perms.push('-');
        }
    }
    perms
}

pub fn list_dir(sess: &Session, dir_path: &str) -> Result<Vec<SftpEntry>, SftpError> {
    let sftp = sess.sftp()?;
    let path_obj = Path::new(dir_path);
    let mut entries = Vec::new();

    for item in sftp.readdir(path_obj)? {
        let name = item.0.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if name == "." || name == ".." || name.is_empty() {
            continue;
        }
        let stat = item.1;
        let mode = stat.perm.unwrap_or(0);
        let kind = if (mode & 0o170000) == 0o040000 {
            "dir"
        } else if (mode & 0o170000) == 0o120000 {
            "symlink"
        } else {
            "file"
        };
        entries.push(SftpEntry {
            name: name.clone(),
            path: format!("{}/{}", dir_path.trim_end_matches('/'), name),
            kind: kind.to_string(),
            size: stat.size.unwrap_or(0),
            mtime_ms: stat.mtime.unwrap_or(0) * 1000,
            mode: mode & 0o777,
            permissions: format_unix_mode(mode),
        });
    }
    Ok(entries)
}

pub fn read_text(sess: &Session, file_path: &str, max_bytes: usize) -> Result<String, SftpError> {
    let sftp = sess.sftp()?;
    let mut file = sftp.open(Path::new(file_path))?;
    let mut buf = Vec::new();
    let mut take = (&mut file).take(max_bytes as u64);
    take.read_to_end(&mut buf)?;
    Ok(String::from_utf8(buf)?)
}

pub fn write_text(sess: &Session, file_path: &str, content: &str) -> Result<(), SftpError> {
    let sftp = sess.sftp()?;
    let mut file = sftp.create(Path::new(file_path))?;
    file.write_all(content.as_bytes())?;
    file.flush()?;
    Ok(())
}
