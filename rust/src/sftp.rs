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

/// `sftp.create` 带截断标志。文件一打开，原来的内容就没了，取消或中途失败也找不回来。
/// 先写到同目录的临时文件，全部写完再改名换上。失败时删掉临时文件。
pub fn write_bytes(sess: &Session, file_path: &str, bytes: &[u8], mut on_progress: impl FnMut(u64, u64) -> bool) -> Result<(), SftpError> {
    let sftp = sess.sftp()?;
    let target = follow_write_target(file_path, |path| classify_node(&sftp, path))?;
    let temp = staged_upload_path(&target.path, &uuid::Uuid::new_v4().simple().to_string())?;
    let mode = staging_mode(target.mode);
    if let Err(err) = write_exclusive(&sftp, &temp, bytes, mode, &mut on_progress) {
        let _ = sftp.unlink(Path::new(&temp));
        return Err(err);
    }
    if let Some(mode) = target.mode {
        match sftp.lstat(Path::new(&temp)) {
            Ok(mut stat) => {
                stat.perm = Some(mode);
                if let Err(err) = sftp.setstat(Path::new(&temp), stat) {
                    let _ = sftp.unlink(Path::new(&temp));
                    return Err(err.into());
                }
            }
            Err(err) => {
                let _ = sftp.unlink(Path::new(&temp));
                return Err(err.into());
            }
        }
    }
    if let Err(err) = sftp.rename(Path::new(&temp), Path::new(&target.path), None) {
        let _ = sftp.unlink(Path::new(&temp));
        return Err(err.into());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NodeKind {
    Missing,
    Symlink(String),
    Directory,
    File { mode: Option<u32> },
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WriteTarget {
    path: String,
    mode: Option<u32>,
}

fn is_missing(err: &ssh2::Error) -> bool {
    // libssh2: LIBSSH2_FX_NO_SUCH_FILE = 2，LIBSSH2_FX_NO_SUCH_PATH = 10。
    matches!(err.code(), ssh2::ErrorCode::SFTP(2) | ssh2::ErrorCode::SFTP(10))
}

fn classify_node(sftp: &ssh2::Sftp, path: &str) -> Result<NodeKind, SftpError> {
    match sftp.lstat(Path::new(path)) {
        Err(err) if is_missing(&err) => Ok(NodeKind::Missing),
        Err(err) => Err(err.into()),
        Ok(stat) => {
            if stat.file_type().is_symlink() {
                let link = sftp.readlink(Path::new(path))?;
                let text = link.to_str().ok_or_else(|| SftpError::Rejected("符号链接目标不是文本，没有写入".into()))?;
                Ok(NodeKind::Symlink(text.to_string()))
            } else if stat.is_dir() {
                Ok(NodeKind::Directory)
            } else if stat.file_type().is_file() {
                Ok(NodeKind::File { mode: stat.perm.map(|perm| perm & 0o777) })
            } else {
                Ok(NodeKind::Other)
            }
        }
    }
}

fn follow_write_target(start: &str, mut inspect: impl FnMut(&str) -> Result<NodeKind, SftpError>) -> Result<WriteTarget, SftpError> {
    let mut current = start.to_string();
    for _ in 0..8 {
        match inspect(&current)? {
            NodeKind::Missing => return Ok(WriteTarget { path: current, mode: None }),
            NodeKind::Directory => return Err(SftpError::Rejected("这是目录，不能当作文件写入".into())),
            NodeKind::Other => return Err(SftpError::Rejected("这个路径不是普通文件，没有写入".into())),
            NodeKind::File { mode } => {
                // 服务端没给出权限时按仅所有者处理，避免临时文件先变成 0644。
                return Ok(WriteTarget { path: current, mode: Some(mode.unwrap_or(0o600)) });
            }
            NodeKind::Symlink(link) => current = join_link_target(&current, &link),
        }
    }
    Err(SftpError::Rejected("符号链接套得太深，没有写入".into()))
}

/// 已有文件用它自己的权限位创建临时文件。新文件仍是 0644。
/// 不能先用 0644 再改回 0600，那段时间里替换内容可被其他用户读取。
fn staging_mode(existing: Option<u32>) -> u32 {
    match existing {
        Some(mode) => mode & 0o777,
        None => 0o644,
    }
}

fn join_link_target(current: &str, link: &str) -> String {
    if link.starts_with('/') {
        return tidy_path(link);
    }
    let parent = parent_posix(current);
    let joined = if parent == "." {
        link.to_string()
    } else if parent == "/" {
        format!("/{link}")
    } else {
        format!("{parent}/{link}")
    };
    tidy_path(&joined)
}

fn parent_posix(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(index) => {
            let parent = &path[..index];
            if parent.is_empty() { "/".to_string() } else { parent.to_string() }
        }
        None => ".".to_string(),
    }
}

/// 只去掉 `.` 和重复斜杠。`..` 留给文件系统解析。
/// 父目录本身是符号链接时，按字符串消掉 `..` 会写到另一个文件。
fn tidy_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        parts.push(part);
    }
    if absolute {
        if parts.is_empty() { "/".to_string() } else { format!("/{}", parts.join("/")) }
    } else if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn staged_upload_path(dest: &str, nonce: &str) -> Result<String, SftpError> {
    if nonce.is_empty() || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SftpError::Rejected("临时文件名无效".into()));
    }
    let (parent, name) = split_file_name(dest)?;
    let staged = format!(".{name}.tianshell-uploading-{nonce}");
    if parent == "/" {
        Ok(format!("/{staged}"))
    } else if parent == "." {
        Ok(staged)
    } else {
        Ok(format!("{parent}/{staged}"))
    }
}

fn split_file_name(path: &str) -> Result<(&str, &str), SftpError> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(SftpError::Rejected("不能写入这个路径".into()));
    }
    let (parent, name) = match trimmed.rfind('/') {
        Some(0) => ("/", &trimmed[1..]),
        Some(index) => (&trimmed[..index], &trimmed[index + 1..]),
        None => (".", trimmed),
    };
    if name.is_empty() || name == "." || name == ".." {
        return Err(SftpError::Rejected("不能写入这个路径".into()));
    }
    Ok((parent, name))
}

fn write_exclusive(sftp: &ssh2::Sftp, path: &str, bytes: &[u8], mode: u32, on_progress: &mut impl FnMut(u64, u64) -> bool) -> Result<(), SftpError> {
    let mut file = sftp.open_mode(
        Path::new(path),
        ssh2::OpenFlags::WRITE | ssh2::OpenFlags::EXCLUSIVE,
        staging_mode(Some(mode)) as i32,
        ssh2::OpenType::File,
    )?;
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
    fn staged_upload_keeps_a_sibling_temp_name() {
        let staged = staged_upload_path("/var/app.log", "abc123").unwrap();
        assert_eq!(staged, "/var/.app.log.tianshell-uploading-abc123");
        assert!(staged_upload_path("/var/app.log", "ab/cd").is_err());
        assert!(staged_upload_path("/", "abc").is_err());
        assert!(staged_upload_path("/var/..", "abc").is_err());
        assert_eq!(staged_upload_path("notes.txt", "abc123").unwrap(), ".notes.txt.tianshell-uploading-abc123");
        assert_eq!(staging_mode(Some(0o600)), 0o600);
        assert_eq!(staging_mode(Some(0o1644)), 0o644);
        assert_eq!(staging_mode(None), 0o644);
    }

    #[test]
    fn write_target_follows_a_file_symlink_and_rejects_a_directory() {
        assert_eq!(join_link_target("/qa/sub/link", "../real/data.txt"), "/qa/sub/../real/data.txt");
        assert_eq!(join_link_target("/qa/link", "/abs/data.txt"), "/abs/data.txt");
        assert_eq!(join_link_target("notes.txt", "other.txt"), "other.txt");
        assert_eq!(join_link_target("dir/link", "../other.txt"), "dir/../other.txt");
        let file = follow_write_target("/qa/sub/link", |path| {
            if path == "/qa/sub/link" {
                Ok(NodeKind::Symlink("../real/data.txt".into()))
            } else if path == "/qa/sub/../real/data.txt" {
                Ok(NodeKind::File { mode: Some(0o700) })
            } else {
                Ok(NodeKind::Missing)
            }
        })
        .unwrap();
        assert_eq!(file.path, "/qa/sub/../real/data.txt");
        assert_eq!(file.mode, Some(0o700));
        let missing = follow_write_target("/qa/new.txt", |_| Ok(NodeKind::Missing)).unwrap();
        assert_eq!(missing.mode, None);
        let dir = follow_write_target("/qa/logs", |_| Ok(NodeKind::Directory)).unwrap_err();
        assert!(dir.to_string().contains("目录"));
        let deep = follow_write_target("/qa/loop", |_| Ok(NodeKind::Symlink("/qa/loop".into()))).unwrap_err();
        assert!(deep.to_string().contains("太深"));
    }

    #[test]
    fn truncating_open_discards_original_bytes_before_the_write_finishes() {
        let root = std::env::temp_dir().join(format!("ts-trunc-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let dest = root.join("keep.txt");
        std::fs::write(&dest, b"original-secret").unwrap();
        let file = std::fs::File::create(&dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"");
        drop(file);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn staged_replace_keeps_original_until_rename() {
        let root = std::env::temp_dir().join(format!("ts-stage-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let dest = root.join("keep.txt");
        std::fs::write(&dest, b"original-secret").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let dest_text = dest.to_str().unwrap();
        let target = follow_write_target(dest_text, local_node).unwrap();
        let temp = staged_upload_path(&target.path, "abc123").unwrap();
        std::fs::write(&temp, b"partial").unwrap();
        std::fs::remove_file(&temp).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"original-secret");
        assert!(!std::path::Path::new(&temp).exists());

        let temp = staged_upload_path(&target.path, "def456").unwrap();
        std::fs::write(&temp, b"new-bytes").unwrap();
        #[cfg(unix)]
        if let Some(mode) = target.mode {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        std::fs::rename(&temp, &target.path).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new-bytes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn staged_replace_updates_a_symlink_target_without_replacing_the_link() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("ts-stage-link-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let real = root.join("real.txt");
        std::fs::write(&real, b"original-secret").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let followed = std::fs::File::create(&link).unwrap();
        assert_eq!(std::fs::read(&real).unwrap(), b"");
        drop(followed);
        std::fs::write(&real, b"original-secret").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();

        let target = follow_write_target(link.to_str().unwrap(), local_node).unwrap();
        assert_eq!(target.path, real.to_str().unwrap());
        assert_eq!(target.mode, Some(0o600));
        let temp = staged_upload_path(&target.path, "abc123").unwrap();
        std::fs::write(&temp, b"partial").unwrap();
        std::fs::remove_file(&temp).unwrap();
        assert_eq!(std::fs::read(&real).unwrap(), b"original-secret");
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());

        let temp = staged_upload_path(&target.path, "def456").unwrap();
        std::fs::write(&temp, b"new-bytes").unwrap();
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::rename(&temp, &target.path).unwrap();
        assert_eq!(std::fs::read(&real).unwrap(), b"new-bytes");
        assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_link(&link).unwrap(), real);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn private_replacement_is_unreadable_before_the_first_byte() {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let root = std::env::temp_dir().join(format!("ts-mode-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mode = staging_mode(Some(0o600));
        assert_eq!(mode, 0o600);
        let temp = root.join(".keep.txt.tianshell-uploading-abc123");
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&temp).unwrap();
        let before = std::fs::symlink_metadata(&temp).unwrap().permissions().mode() & 0o777;
        assert_eq!(before & 0o077, 0);
        file.write_all(b"secret-bytes").unwrap();
        let during = std::fs::symlink_metadata(&temp).unwrap().permissions().mode() & 0o777;
        assert_eq!(during & 0o077, 0);
        let wide = root.join("wide.txt");
        std::fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(&wide).unwrap();
        let wide_mode = std::fs::symlink_metadata(&wide).unwrap().permissions().mode() & 0o777;
        assert_ne!(wide_mode & 0o044, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_parent_dotdot_follows_the_filesystem_not_the_lexical_path() {
        let root = std::env::temp_dir().join(format!("ts-lex-{}", std::process::id()));
        let var_dir = root.join("var");
        let data_log = root.join("data").join("log");
        std::fs::create_dir_all(&var_dir).unwrap();
        std::fs::create_dir_all(&data_log).unwrap();
        let victim = var_dir.join("only-here.txt");
        let physical = root.join("data").join("only-here.txt");
        std::fs::write(&victim, b"do-not-touch").unwrap();
        std::fs::write(&physical, b"physical-original").unwrap();
        std::os::unix::fs::symlink(&data_log, var_dir.join("log")).unwrap();
        std::os::unix::fs::symlink("../only-here.txt", data_log.join("pointer")).unwrap();
        let start = var_dir.join("log").join("pointer");
        let target = follow_write_target(start.to_str().unwrap(), local_node).unwrap();
        let resolved = std::fs::canonicalize(&target.path).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(&physical).unwrap());
        assert_ne!(resolved, std::fs::canonicalize(&victim).unwrap());
        std::fs::write(&target.path, b"updated-physical").unwrap();
        assert_eq!(std::fs::read(&physical).unwrap(), b"updated-physical");
        assert_eq!(std::fs::read(&victim).unwrap(), b"do-not-touch");
        let temp = staged_upload_path(&target.path, "abc123").unwrap();
        std::fs::write(&temp, b"staged").unwrap();
        assert_eq!(std::fs::canonicalize(&temp).unwrap().parent().unwrap(), std::fs::canonicalize(root.join("data")).unwrap());
        assert!(!var_dir.join(".only-here.txt.tianshell-uploading-abc123").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    fn local_node(path: &str) -> Result<NodeKind, SftpError> {
        match std::fs::symlink_metadata(path) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(NodeKind::Missing),
            Err(err) => Err(SftpError::Io(err)),
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    let link = std::fs::read_link(path)?;
                    Ok(NodeKind::Symlink(link.to_string_lossy().into_owned()))
                } else if meta.is_dir() {
                    Ok(NodeKind::Directory)
                } else if meta.is_file() {
                    #[cfg(unix)]
                    let mode = {
                        use std::os::unix::fs::PermissionsExt;
                        Some(meta.permissions().mode() & 0o777)
                    };
                    #[cfg(not(unix))]
                    let mode = None;
                    Ok(NodeKind::File { mode })
                } else {
                    Ok(NodeKind::Other)
                }
            }
        }
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
