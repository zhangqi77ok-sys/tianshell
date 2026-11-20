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
    #[error("{0}")]
    Rejected(String),
}

#[derive(Debug, Clone)]
pub struct RemoteText {
    pub content: String,
    pub truncated: bool,
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

pub const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
pub const UPLOAD_TOO_BIG_MESSAGE: &str = "上传内容超过 64MB";

pub fn upload_too_big(len: usize) -> bool {
    len > MAX_UPLOAD_BYTES
}

/// `Content-Length` 已经声明超限时，不必把正文读进内存。
/// 头缺失或不是数字时返回 false，交给后面的读取上限处理。
pub fn declared_length_too_big(header: Option<&str>) -> bool {
    let Some(text) = header.map(str::trim).filter(|text| !text.is_empty()) else {
        return false;
    };
    let Ok(len) = text.parse::<u64>() else {
        return false;
    };
    usize::try_from(len).map(upload_too_big).unwrap_or(true)
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
    write_bytes(sess, file_path, content.as_bytes(), |_, _| true)
}

pub fn read_bytes_limited(sess: &Session, file_path: &str, max_bytes: usize) -> Result<(Vec<u8>, bool), SftpError> {
    let limit = max_bytes.min(262_144);
    let sftp = sess.sftp()?;
    let mut file = sftp.open(Path::new(file_path))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    while buf.len() <= limit {
        let n = file.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > limit {
            break;
        }
    }
    let truncated = buf.len() > limit;
    if truncated {
        buf.truncate(limit);
    }
    if looks_like_private_key(&buf) {
        return Err(SftpError::Rejected("拒绝返回私钥内容".into()));
    }
    Ok((buf, truncated))
}

fn looks_like_private_key(buf: &[u8]) -> bool {
    let head = &buf[..buf.len().min(4096)];
    let text = String::from_utf8_lossy(head);
    text.contains("PRIVATE KEY") || text.contains("-----BEGIN")
}

pub fn read_text_limited(sess: &Session, file_path: &str, max_bytes: usize) -> Result<RemoteText, SftpError> {
    let limit = max_bytes.min(262_144);
    let sftp = sess.sftp()?;
    let mut file = sftp.open(Path::new(file_path))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    while buf.len() <= limit {
        let n = file.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > limit {
            break;
        }
    }
    let truncated = buf.len() > limit;
    if truncated {
        buf.truncate(limit);
    }
    if buf.iter().take(1024).any(|byte| *byte == 0) {
        return Err(SftpError::Rejected("检测到二进制文件，暂不支持在线文本编辑".into()));
    }
    if looks_like_private_key(&buf) {
        return Err(SftpError::Rejected("拒绝返回私钥内容".into()));
    }
    Ok(RemoteText {
        content: String::from_utf8_lossy(&buf).into_owned(),
        truncated,
    })
}

pub fn mkdir(sess: &Session, dir_path: &str) -> Result<(), SftpError> {
    let sftp = sess.sftp()?;
    sftp.mkdir(Path::new(dir_path), 0o755)?;
    Ok(())
}

pub fn rename(sess: &Session, old_path: &str, new_path: &str) -> Result<(), SftpError> {
    let sftp = sess.sftp()?;
    sftp.rename(Path::new(old_path), Path::new(new_path), None)?;
    Ok(())
}

pub fn chmod(sess: &Session, file_path: &str, mode: u32) -> Result<(), SftpError> {
    let sftp = sess.sftp()?;
    let mut stat = sftp.stat(Path::new(file_path))?;
    stat.perm = Some(mode);
    sftp.setstat(Path::new(file_path), stat)?;
    Ok(())
}

pub fn remove_path(sess: &Session, file_path: &str) -> Result<(), SftpError> {
    if file_path.is_empty() || file_path == "/" {
        return Err(SftpError::Rejected("安全拦截：禁止删除根目录 /".into()));
    }
    let sftp = sess.sftp()?;
    remove_rec(&sftp, file_path)
}

/// `stat` 会穿过符号链接。链接指向目录时，跟着走就会删掉目标里的内容。
/// 链接本身只做 unlink。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveAction {
    Unlink,
    Recurse,
}

pub fn remove_action(is_symlink: bool, is_dir: bool) -> RemoveAction {
    if is_symlink || !is_dir {
        RemoveAction::Unlink
    } else {
        RemoveAction::Recurse
    }
}

fn remove_rec(sftp: &ssh2::Sftp, file_path: &str) -> Result<(), SftpError> {
    let stat = sftp.lstat(Path::new(file_path))?;
    if remove_action(stat.file_type().is_symlink(), stat.is_dir()) == RemoveAction::Recurse {
        for item in sftp.readdir(Path::new(file_path))? {
            let name = item.0.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "." || name == ".." || name.is_empty() {
                continue;
            }
            let child = format!("{}/{}", file_path.trim_end_matches('/'), name);
            remove_rec(sftp, &child)?;
        }
        sftp.rmdir(Path::new(file_path))?;
    } else {
        sftp.unlink(Path::new(file_path))?;
    }
    Ok(())
}

pub fn read_chunks(sess: &Session, file_path: &str, mut on_chunk: impl FnMut(&[u8]) -> bool) -> Result<(), SftpError> {
    let sftp = sess.sftp()?;
    let mut file = sftp.open(Path::new(file_path))?;
    let mut buf = [0u8; 32_768];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if !on_chunk(&buf[..n]) {
            break;
        }
    }
    Ok(())
}

pub fn write_bytes(sess: &Session, file_path: &str, bytes: &[u8], mut on_progress: impl FnMut(u64, u64) -> bool) -> Result<(), SftpError> {
    let sftp = sess.sftp()?;
    let mut file = sftp.create(Path::new(file_path))?;
    let total = bytes.len() as u64;
    let mut sent = 0u64;
    for chunk in bytes.chunks(32_768) {
        file.write_all(chunk)?;
        sent += chunk.len() as u64;
        if !on_progress(sent, total) {
            return Err(SftpError::Rejected("传输已取消".into()));
        }
    }
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_symlink_is_removed_without_walking_into_its_target() {
        assert_eq!(remove_action(true, true), RemoveAction::Unlink);
        assert_eq!(remove_action(true, false), RemoveAction::Unlink);
        assert_eq!(remove_action(false, true), RemoveAction::Recurse);
        assert_eq!(remove_action(false, false), RemoveAction::Unlink);
    }

    #[cfg(unix)]
    #[test]
    fn local_symlink_metadata_selects_unlink_and_leaves_the_target() {
        let root = std::env::temp_dir().join(format!("ts-unlink-{}", std::process::id()));
        let real = root.join("real");
        let keep = real.join("keep");
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(keep.join("data.txt"), b"keep-me").unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let followed = std::fs::metadata(&link).unwrap();
        assert!(followed.is_dir());
        assert_eq!(remove_action(false, followed.is_dir()), RemoveAction::Recurse);
        let own = std::fs::symlink_metadata(&link).unwrap();
        let action = remove_action(own.file_type().is_symlink(), own.file_type().is_dir());
        assert_eq!(action, RemoveAction::Unlink);
        std::fs::remove_file(&link).unwrap();
        assert!(keep.join("data.txt").is_file());
        assert_eq!(std::fs::read(keep.join("data.txt")).unwrap(), b"keep-me");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn upload_limit_is_64_mebibytes() {
        assert!(!upload_too_big(0));
        assert!(!upload_too_big(MAX_UPLOAD_BYTES));
        assert!(upload_too_big(MAX_UPLOAD_BYTES + 1));
        assert!(!declared_length_too_big(None));
        assert!(!declared_length_too_big(Some("")));
        assert!(!declared_length_too_big(Some("nope")));
        assert!(!declared_length_too_big(Some(&MAX_UPLOAD_BYTES.to_string())));
        assert!(declared_length_too_big(Some(&(MAX_UPLOAD_BYTES as u64 + 1).to_string())));
        assert!(declared_length_too_big(Some(&u64::MAX.to_string())));
    }
}
