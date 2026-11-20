use std::ffi::{c_char, c_int, CString};
use std::io::{Read, Write};
use std::path::Path;
use serde::{Deserialize, Serialize};
use ssh2::Session;
use thiserror::Error;

extern "C" {
    fn libssh2_sftp_posix_rename_ex(
        sftp: *mut ssh2::libssh2_sys::LIBSSH2_SFTP,
        source_filename: *const c_char,
        source_filename_len: usize,
        dest_filename: *const c_char,
        dest_filename_len: usize,
    ) -> c_int;
}

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
    remote_text_preview(&buf, limit)
}

/// 编辑器和 MCP 读文本只接受能原样写回的 UTF-8。
/// 有 NUL，或整段不是 UTF-8 时直接拒绝。不能先换成替换字符再写回去，那样会改掉原字节。
/// 超过上限时只丢掉末尾没写完的字符，并标成截断，调用方不能把这段写回。
fn remote_text_preview(buf: &[u8], limit: usize) -> Result<RemoteText, SftpError> {
    let truncated = buf.len() > limit;
    let window = if truncated { trim_partial_utf8(&buf[..limit]) } else { buf };
    if window.contains(&0) {
        return Err(SftpError::Rejected("检测到二进制文件，暂不支持在线文本编辑".into()));
    }
    let text = std::str::from_utf8(window).map_err(|_| {
        SftpError::Rejected("这个文件不是 UTF-8 文本，没有打开，原文件没有改动".into())
    })?;
    if looks_like_private_key(window) {
        return Err(SftpError::Rejected("拒绝返回私钥内容".into()));
    }
    Ok(RemoteText {
        content: text.to_string(),
        truncated,
    })
}

fn trim_partial_utf8(buf: &[u8]) -> &[u8] {
    if std::str::from_utf8(buf).is_ok() {
        return buf;
    }
    let floor = buf.len().saturating_sub(3);
    let mut end = buf.len();
    while end > floor {
        end -= 1;
        if std::str::from_utf8(&buf[..end]).is_ok() {
            return &buf[..end];
        }
    }
    buf
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
/// 先写到同目录的临时文件，全部写完再换上。失败时只删临时文件。
///
/// 换上已有文件：
/// posix-rename 成功 -> 旧目录项被替换
/// 扩展不存在，目标本来就没有 -> 普通改名
/// 扩展不存在或改名失败，目标还在 -> 删临时文件，原文件不动
/// 不会先删掉旧文件再改名。
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
    if let Err(err) = commit_staged_upload(&sftp, sess, &temp, &target) {
        let _ = sftp.unlink(Path::new(&temp));
        return Err(err);
    }
    Ok(())
}

/// 第 3 版 `SSH_FXP_RENAME` 的报文里没有覆盖标志。
/// OpenSSH 对普通文件是 `link` 再 `unlink`：目标已存在时 `EEXIST` 变成 SFTP(4)，新名字则成功。
/// 覆盖要走扩展 `posix-rename@openssh.com`，也就是 `rename(2)`，不先删旧文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PosixAttempt {
    Done,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StagedCommit {
    Done,
    PlainCreate,
    KeepOriginal,
}

fn staged_commit(existing: bool, attempt: PosixAttempt) -> StagedCommit {
    match attempt {
        PosixAttempt::Done => StagedCommit::Done,
        PosixAttempt::Unsupported if !existing => StagedCommit::PlainCreate,
        PosixAttempt::Unsupported | PosixAttempt::Failed => StagedCommit::KeepOriginal,
    }
}

struct PosixOutcome {
    attempt: PosixAttempt,
    error: Option<ssh2::Error>,
}

fn commit_staged_upload(sftp: &ssh2::Sftp, sess: &Session, temp: &str, target: &WriteTarget) -> Result<(), SftpError> {
    let outcome = posix_rename_outcome(sess, temp, &target.path)?;
    match staged_commit(target.mode.is_some(), outcome.attempt) {
        StagedCommit::Done => Ok(()),
        StagedCommit::PlainCreate => {
            sftp.rename(Path::new(temp), Path::new(&target.path), Some(ssh2::RenameFlags::empty()))?;
            Ok(())
        }
        StagedCommit::KeepOriginal => Err(match outcome.error {
            Some(err) => err.into(),
            None => SftpError::Rejected("远端没有换上文件，原文件没有改动".into()),
        }),
    }
}

fn posix_rename_outcome(sess: &Session, source: &str, dest: &str) -> Result<PosixOutcome, SftpError> {
    let source = CString::new(source).map_err(|_| SftpError::Rejected("路径里有空字节，没有写入".into()))?;
    let dest = CString::new(dest).map_err(|_| SftpError::Rejected("路径里有空字节，没有写入".into()))?;
    let mut guard = sess.raw();
    let session = &mut *guard as *mut ssh2::libssh2_sys::LIBSSH2_SESSION;
    unsafe {
        let sftp = ssh2::libssh2_sys::libssh2_sftp_init(session);
        if sftp.is_null() {
            let rc = ssh2::libssh2_sys::libssh2_session_last_errno(session);
            return Err(ssh2::Error::from_session_error_raw(session, rc).into());
        }
        let rc = libssh2_sftp_posix_rename_ex(
            sftp,
            source.as_ptr(),
            source.as_bytes().len(),
            dest.as_ptr(),
            dest.as_bytes().len(),
        );
        let fx = ssh2::libssh2_sys::libssh2_sftp_last_error(sftp);
        let _ = ssh2::libssh2_sys::libssh2_sftp_shutdown(sftp);
        if rc == 0 {
            return Ok(PosixOutcome { attempt: PosixAttempt::Done, error: None });
        }
        let error = if fx != 0 {
            ssh2::Error::from_errno(ssh2::ErrorCode::SFTP(fx as c_int))
        } else {
            ssh2::Error::from_session_error_raw(session, rc)
        };
        let attempt = if fx as u64 == u64::from(ssh2::libssh2_sys::LIBSSH2_FX_OP_UNSUPPORTED as u32) {
            PosixAttempt::Unsupported
        } else {
            PosixAttempt::Failed
        };
        Ok(PosixOutcome { attempt, error: Some(error) })
    }
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
    fn existing_overwrite_keeps_the_old_file_when_posix_rename_cannot_replace_it() {
        assert_eq!(staged_commit(true, PosixAttempt::Done), StagedCommit::Done);
        assert_eq!(staged_commit(true, PosixAttempt::Unsupported), StagedCommit::KeepOriginal);
        assert_eq!(staged_commit(true, PosixAttempt::Failed), StagedCommit::KeepOriginal);
        assert_eq!(staged_commit(false, PosixAttempt::Done), StagedCommit::Done);
        assert_eq!(staged_commit(false, PosixAttempt::Unsupported), StagedCommit::PlainCreate);
        assert_eq!(staged_commit(false, PosixAttempt::Failed), StagedCommit::KeepOriginal);
    }

    #[cfg(unix)]
    #[test]
    fn openssh_plain_rename_leaves_an_existing_file_and_posix_rename_replaces_it() {
        if !std::path::Path::new("/usr/sbin/sshd").is_file() {
            return;
        }
        if !std::process::Command::new("sudo").args(["-n", "true"]).status().map(|status| status.success()).unwrap_or(false) {
            return;
        }
        if !std::path::Path::new("/usr/bin/ssh-keygen").is_file() {
            return;
        }
        let _server = match LocalSshd::start() {
            Ok(server) => server,
            Err(err) => panic!("{err}"),
        };
        let sess = connect_sftp(_server.port, &_server.key).unwrap_or_else(|err| panic!("{err}"));
        let dir = _server.work.clone();
        let keep = format!("{dir}/keep.txt");
        write_bytes(&sess, &keep, b"original-secret", |_, _| true).unwrap();
        chmod(&sess, &keep, 0o600).unwrap();
        assert_eq!(sftp_bytes(&sess, &keep).unwrap(), b"original-secret");
        assert_eq!(sftp_mode(&sess, &keep).unwrap() & 0o777, 0o600);

        let incoming = format!("{dir}/incoming.txt");
        write_bytes(&sess, &incoming, b"plain-rename-source", |_, _| true).unwrap();
        let plain = sess.sftp().unwrap().rename(Path::new(&incoming), Path::new(&keep), None).unwrap_err();
        assert!(plain.to_string().contains("SFTP(4)"), "{plain}");
        assert_eq!(sftp_bytes(&sess, &keep).unwrap(), b"original-secret");
        assert_eq!(sftp_mode(&sess, &keep).unwrap() & 0o777, 0o600);
        assert_eq!(sftp_bytes(&sess, &incoming).unwrap(), b"plain-rename-source");
        sess.sftp().unwrap().unlink(Path::new(&incoming)).unwrap();

        let mut replacement = vec![0x5a; 262_144];
        replacement[..4].copy_from_slice(b"NEW!");
        let mut saw_private_temp = false;
        write_bytes(&sess, &keep, &replacement, |sent, _| {
            if sent >= 32_768 && !saw_private_temp {
                saw_private_temp = true;
                let mode = uploading_mode(&sess, &dir).unwrap();
                assert_eq!(mode & 0o077, 0, "临时文件权限 {mode:o}");
            }
            true
        })
        .unwrap();
        assert!(saw_private_temp);
        assert_eq!(sftp_bytes(&sess, &keep).unwrap(), replacement);
        assert_eq!(sftp_mode(&sess, &keep).unwrap() & 0o777, 0o600);
        assert!(uploading_names(&sess, &dir).unwrap().is_empty());

        std::os::unix::fs::symlink("keep.txt", std::path::Path::new(&dir).join("rel-link")).unwrap();
        let link = format!("{dir}/rel-link");
        write_bytes(&sess, &link, b"through-link", |_, _| true).unwrap();
        assert_eq!(sftp_bytes(&sess, &keep).unwrap(), b"through-link");
        assert_eq!(std::fs::read_link(&link).unwrap(), std::path::Path::new("keep.txt"));
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(sftp_mode(&sess, &keep).unwrap() & 0o777, 0o600);

        let cancel = format!("{dir}/cancel.txt");
        write_bytes(&sess, &cancel, b"cancel-original", |_, _| true).unwrap();
        chmod(&sess, &cancel, 0o600).unwrap();
        let cancelled = write_bytes(&sess, &cancel, &vec![7u8; 65_536], |sent, _| sent < 32_768).unwrap_err();
        assert!(cancelled.to_string().contains("取消"), "{cancelled}");
        assert_eq!(sftp_bytes(&sess, &cancel).unwrap(), b"cancel-original");
        assert_eq!(sftp_mode(&sess, &cancel).unwrap() & 0o777, 0o600);
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(uploading_names(&sess, &dir).unwrap().is_empty());

        let sub = format!("{dir}/subdir");
        sess.sftp().unwrap().mkdir(Path::new(&sub), 0o755).unwrap();
        let rejected = write_bytes(&sess, &sub, b"nope", |_, _| true).unwrap_err();
        assert!(rejected.to_string().contains("目录"), "{rejected}");
        let pipe = format!("{dir}/pipe");
        assert!(std::process::Command::new("mkfifo").arg(&pipe).status().unwrap().success());
        let rejected = write_bytes(&sess, &pipe, b"nope", |_, _| true).unwrap_err();
        assert!(rejected.to_string().contains("不是普通文件"), "{rejected}");
        assert!(std::path::Path::new(&pipe).exists());
        let binary = format!("{dir}/not-utf8.bin");
        let raw = b"PDF\xff\xfebinary-tail";
        write_bytes(&sess, &binary, raw, |_, _| true).unwrap();
        let refused = read_text_limited(&sess, &binary, 4096).unwrap_err();
        assert!(refused.to_string().contains("UTF-8"), "{refused}");
        assert!(!refused.to_string().contains('\u{FFFD}'));
        assert_eq!(sftp_bytes(&sess, &binary).unwrap(), raw);
        let mut late_nul = vec![b'a'; 2048];
        late_nul.extend_from_slice(b"tail");
        late_nul[1500] = 0;
        let nul_path = format!("{dir}/late-nul.txt");
        write_bytes(&sess, &nul_path, &late_nul, |_, _| true).unwrap();
        let refused = read_text_limited(&sess, &nul_path, 4096).unwrap_err();
        assert!(refused.to_string().contains("二进制"), "{refused}");
        assert_eq!(sftp_bytes(&sess, &nul_path).unwrap(), late_nul);
        write_bytes(&sess, &format!("{dir}/after.txt"), b"still-up", |_, _| true).unwrap();
        assert_eq!(sftp_bytes(&sess, &format!("{dir}/after.txt")).unwrap(), b"still-up");
        assert!(uploading_names(&sess, &dir).unwrap().is_empty());
    }

    #[cfg(unix)]
    struct LocalSshd {
        child: std::process::Child,
        pid_file: std::path::PathBuf,
        dir: std::path::PathBuf,
        work: String,
        key: std::path::PathBuf,
        port: u16,
    }

    #[cfg(unix)]
    impl Drop for LocalSshd {
        fn drop(&mut self) {
            let _ = self.child.kill();
            if let Ok(pid) = std::fs::read_to_string(&self.pid_file) {
                let _ = std::process::Command::new("sudo").args(["-n", "kill", pid.trim()]).status();
            }
            let _ = self.child.wait();
            let text = self.dir.to_string_lossy();
            if text.starts_with("/tmp/tianshell-qa/pr23-") {
                let _ = std::process::Command::new("sudo").args(["-n", "rm", "-rf", "--", text.as_ref()]).status();
            }
        }
    }

    #[cfg(unix)]
    impl LocalSshd {
        fn start() -> Result<Self, String> {
            let _ = std::process::Command::new("sudo").args(["-n", "mkdir", "-p", "/run/sshd"]).status();
            let _ = std::process::Command::new("sudo").args(["-n", "chmod", "755", "/run/sshd"]).status();
            let id = std::process::Command::new("id")
                .arg("tianshellsftp")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            if !id {
                let created = std::process::Command::new("sudo")
                    .args(["-n", "useradd", "--system", "--create-home", "--home-dir", "/var/lib/tianshell-sftp-test", "--shell", "/usr/sbin/nologin", "tianshellsftp"])
                    .status()
                    .map_err(|err| err.to_string())?;
                if !created.success() {
                    return Err("创建测试用户失败".into());
                }
            }
            let unlocked = std::process::Command::new("sudo")
                .args(["-n", "usermod", "-p", "$6$rounds=1000$tianshell$notapasswordhash000000000000000000000000000000000000", "tianshellsftp"])
                .status()
                .map_err(|err| err.to_string())?;
            if !unlocked.success() {
                return Err("测试用户不能用于公钥登录".into());
            }
            let dir = std::path::PathBuf::from(format!("/tmp/tianshell-qa/pr23-overwrite-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
            let work = dir.join("work");
            std::fs::create_dir_all(&work).map_err(|err| err.to_string())?;
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o777)).map_err(|err| err.to_string())?;
            }
            let host = dir.join("host");
            let client = dir.join("client");
            for path in [&host, &client] {
                let generated = std::process::Command::new("ssh-keygen")
                    .args(["-q", "-t", "ed25519", "-f", path.to_str().unwrap(), "-N", ""])
                    .status()
                    .map_err(|err| err.to_string())?;
                if !generated.success() {
                    return Err("ssh-keygen 失败".into());
                }
            }
            std::fs::copy(client.with_extension("pub"), dir.join("authorized_keys")).map_err(|err| err.to_string())?;
            let port = (20_000 + (std::process::id() % 10_000)) as u16;
            let pid_file = dir.join("sshd.pid");
            let log_file = dir.join("sshd.log");
            let config = format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {host}\nPidFile {pid}\nAuthorizedKeysFile {keys}\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nChallengeResponseAuthentication no\nUsePAM no\nPubkeyAuthentication yes\nPermitRootLogin no\nStrictModes no\nAllowUsers tianshellsftp\nSubsystem sftp internal-sftp\n",
                host = host.display(),
                pid = pid_file.display(),
                keys = dir.join("authorized_keys").display(),
            );
            let config_path = dir.join("sshd_config");
            std::fs::write(&config_path, config).map_err(|err| err.to_string())?;
            let mut child = std::process::Command::new("sudo")
                .args(["-n", "/usr/sbin/sshd", "-f", config_path.to_str().unwrap(), "-D", "-E", log_file.to_str().unwrap()])
                .spawn()
                .map_err(|err| err.to_string())?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() && pid_file.exists() {
                    break;
                }
                if let Some(status) = child.try_wait().map_err(|err| err.to_string())? {
                    return Err(format!("sshd 退出 {status}: {}", read_sshd_log(&log_file)));
                }
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    return Err(format!("sshd 没有开始监听: {}", read_sshd_log(&log_file)));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Ok(Self {
                child,
                pid_file,
                work: work.to_string_lossy().into_owned(),
                key: client,
                port,
                dir,
            })
        }
    }

    #[cfg(unix)]
    fn read_sshd_log(path: &std::path::Path) -> String {
        let output = std::process::Command::new("sudo").args(["-n", "cat", "--", path.to_str().unwrap_or("")]).output();
        match output {
            Ok(output) => String::from_utf8_lossy(&output.stdout).into_owned(),
            Err(err) => err.to_string(),
        }
    }

    #[cfg(unix)]
    fn connect_sftp(port: u16, key: &std::path::Path) -> Result<ssh2::Session, String> {
        let tcp = std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|err| err.to_string())?;
        let mut sess = ssh2::Session::new().map_err(|err| err.to_string())?;
        sess.set_tcp_stream(tcp);
        sess.handshake().map_err(|err| err.to_string())?;
        sess.userauth_pubkey_file("tianshellsftp", None, key, None).map_err(|err| err.to_string())?;
        if !sess.authenticated() {
            return Err("公钥登录没有完成".into());
        }
        sess.set_blocking(true);
        sess.set_timeout(8_000);
        Ok(sess)
    }

    #[cfg(unix)]
    fn sftp_bytes(sess: &ssh2::Session, path: &str) -> Result<Vec<u8>, String> {
        let sftp = sess.sftp().map_err(|err| err.to_string())?;
        let mut file = sftp.open(Path::new(path)).map_err(|err| err.to_string())?;
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut buf).map_err(|err| err.to_string())?;
        Ok(buf)
    }

    #[cfg(unix)]
    fn sftp_mode(sess: &ssh2::Session, path: &str) -> Result<u32, ssh2::Error> {
        let stat = sess.sftp()?.lstat(Path::new(path))?;
        Ok(stat.perm.unwrap_or(0))
    }

    #[cfg(unix)]
    fn uploading_names(sess: &ssh2::Session, dir: &str) -> Result<Vec<String>, ssh2::Error> {
        let entries = sess.sftp()?.readdir(Path::new(dir))?;
        Ok(entries
            .into_iter()
            .filter_map(|(path, _)| {
                let name = path.file_name()?.to_string_lossy().into_owned();
                name.contains("tianshell-uploading").then_some(name)
            })
            .collect())
    }

    #[cfg(unix)]
    fn uploading_mode(sess: &ssh2::Session, dir: &str) -> Result<u32, String> {
        let entries = sess.sftp().map_err(|err| err.to_string())?.readdir(Path::new(dir)).map_err(|err| err.to_string())?;
        for (path, stat) in entries {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            if name.contains("tianshell-uploading") {
                return Ok(stat.perm.unwrap_or(0));
            }
        }
        Err("写入过程中没有临时文件".into())
    }

    #[test]
    fn remote_text_refuses_bytes_that_cannot_be_written_back_unchanged() {
        let text = remote_text_preview("日志：保持原样\n".as_bytes(), 1024).unwrap();
        assert_eq!(text.content, "日志：保持原样\n");
        assert!(!text.truncated);

        let mut late_nul = vec![b'a'; 2048];
        late_nul[1500] = 0;
        late_nul.extend_from_slice(b"tail");
        let err = remote_text_preview(&late_nul, 4096).unwrap_err();
        assert!(err.to_string().contains("二进制"), "{err}");

        let binary = b"PDF\xff\xfebinary";
        let lossy = String::from_utf8_lossy(binary);
        assert!(lossy.contains('\u{FFFD}'));
        assert_ne!(lossy.as_bytes(), binary);
        let err = remote_text_preview(binary, 4096).unwrap_err();
        assert!(err.to_string().contains("UTF-8"), "{err}");
        assert!(!err.to_string().contains('\u{FFFD}'));

        let mut cut = "甲乙丙".as_bytes().to_vec();
        cut.extend_from_slice("丁".as_bytes());
        let limit = "甲乙丙".as_bytes().len() + 1;
        let preview = remote_text_preview(&cut, limit).unwrap();
        assert!(preview.truncated);
        assert_eq!(preview.content, "甲乙丙");
        assert!(std::str::from_utf8(preview.content.as_bytes()).is_ok());

        let key = b"-----BEGIN OPENSSH PRIVATE KEY-----\n";
        let err = remote_text_preview(key, 4096).unwrap_err();
        assert!(err.to_string().contains("私钥"), "{err}");
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
