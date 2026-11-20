//! 一次性下载票。浏览器下载不能带自定义头，所以不能把长期访问令牌放进 URL。
//! 票在签发后几十秒内只能用一次，地址里也没有长期令牌。

use std::collections::HashMap;
use std::time::{Duration, Instant};
use rand::RngCore;
use crate::util::hex_encode;

pub const GRANT_TTL: Duration = Duration::from_secs(60);
const MAX_GRANTS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadKind {
    File,
    Archive,
}

#[derive(Debug)]
pub struct ArchiveRequest {
    pub command: String,
    pub filename: String,
}

/// 把远端路径收成一条 `tar -C parent -- name`。
/// 路径 `/` 不能收成名字 `root`，否则打的是 `/root`，不是文件系统根。
/// 整个根目录也不打包：里面有 `/proc` 这类特殊文件，而且会占住这条 SFTP 连接。
/// `--` 后面才是目录名。否则 `-safe`、`--exclude` 这类合法名字会被 tar 当成选项。
pub fn archive_request(remote: &str) -> Result<ArchiveRequest, &'static str> {
    if remote.is_empty() {
        return Err("缺少 path 参数");
    }
    if remote_is_filesystem_root(remote) {
        return Err("不会打包整个根目录。进入要下载的目录后再打包。");
    }
    if archive_path_rejected(remote) {
        return Err("路径包含非法字符");
    }
    let norm = remote.trim_end_matches('/');
    if norm.is_empty() || remote_is_filesystem_root(norm) {
        return Err("不会打包整个根目录。进入要下载的目录后再打包。");
    }
    let (parent, name) = match norm.rfind('/') {
        Some(0) => ("/", &norm[1..]),
        Some(idx) => (&norm[..idx], &norm[idx + 1..]),
        None => (".", norm),
    };
    if name.is_empty() || name == "." || name == ".." {
        return Err("不会打包这个路径");
    }
    Ok(ArchiveRequest {
        command: format!("tar -czf - -C {} -- {}", shell_quote(parent), shell_quote(name)),
        filename: format!("{name}.tar.gz"),
    })
}

pub fn archive_path_rejected(remote: &str) -> bool {
    remote.chars().any(|c| matches!(c, ';' | '|' | '&' | '$' | '>' | '<' | '`' | '\n' | '\r'))
}

/// `tar` 退出码不是 0 时，这次打包是失败，不能记成完成。
pub fn archive_failure(exit_code: i32, stderr: &str) -> Option<String> {
    if exit_code == 0 {
        return None;
    }
    let line: String = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .filter(|ch| !ch.is_control())
        .take(160)
        .collect();
    if line.is_empty() {
        Some(format!("远端打包失败（退出码 {exit_code}）"))
    } else {
        Some(format!("远端打包失败（退出码 {exit_code}）：{line}"))
    }
}

fn remote_is_filesystem_root(remote: &str) -> bool {
    let trimmed = remote.trim();
    !trimmed.is_empty() && trimmed.chars().all(|ch| ch == '/')
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// 普通下载只接受普通文件。目录仍提示改走打包。
/// 管道、设备和套接字不能打开来读：管道会堵住这条连接，设备则没有结束。
/// 打包下载不在这里拒绝。
pub fn file_download_rejected(kind: DownloadKind, node: crate::sftp::ReadNode) -> Option<&'static str> {
    if kind != DownloadKind::File {
        return None;
    }
    match node {
        crate::sftp::ReadNode::Regular => None,
        crate::sftp::ReadNode::Directory => Some("这是目录，请用打包下载"),
        crate::sftp::ReadNode::Special => Some("这不是普通文件，没有开始下载"),
    }
}

#[derive(Debug)]
pub struct IssuedGrant {
    pub session_id: String,
    pub path: String,
    pub kind: DownloadKind,
}

struct StoredGrant {
    session_id: String,
    path: String,
    kind: DownloadKind,
    expires_at: Instant,
}

pub struct DownloadGrants {
    inner: HashMap<String, StoredGrant>,
}

impl DownloadGrants {
    pub fn new() -> Self {
        Self { inner: HashMap::new() }
    }

    pub fn issue(
        &mut self,
        session_id: impl Into<String>,
        path: impl Into<String>,
        kind: DownloadKind,
        now: Instant,
    ) -> Result<String, &'static str> {
        self.purge(now);
        if self.inner.len() >= MAX_GRANTS {
            return Err("一次性下载票太多，请等一会儿再下");
        }
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let id = hex_encode(&bytes);
        self.inner.insert(
            id.clone(),
            StoredGrant {
                session_id: session_id.into(),
                path: path.into(),
                kind,
                expires_at: now + GRANT_TTL,
            },
        );
        Ok(id)
    }

    /// 取走这张票。过期、不存在或已经用过都失败，错误文案不包含票号。
    pub fn redeem(&mut self, id: &str, now: Instant) -> Result<IssuedGrant, &'static str> {
        self.purge(now);
        let Some(grant) = self.inner.remove(id) else {
            return Err("下载票无效、已过期或已经用过");
        };
        Ok(IssuedGrant {
            session_id: grant.session_id,
            path: grant.path,
            kind: grant.kind,
        })
    }

    fn purge(&mut self, now: Instant) {
        self.inner.retain(|_, grant| now < grant.expires_at);
    }
}

impl Default for DownloadGrants {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_is_single_use_and_expires() {
        let mut grants = DownloadGrants::new();
        let now = Instant::now();
        let id = grants.issue("sess", "/var/log/app.log", DownloadKind::File, now).unwrap();
        assert_eq!(id.len(), 64);
        let first = grants.redeem(&id, now).unwrap();
        assert_eq!(first.session_id, "sess");
        assert_eq!(first.path, "/var/log/app.log");
        assert_eq!(first.kind, DownloadKind::File);
        assert!(grants.redeem(&id, now).is_err());
    }

    #[test]
    fn expired_grant_cannot_be_redeemed() {
        let mut grants = DownloadGrants::new();
        let now = Instant::now();
        let id = grants.issue("sess", "/tmp/a", DownloadKind::Archive, now).unwrap();
        let later = now + GRANT_TTL;
        let err = grants.redeem(&id, later).unwrap_err();
        assert!(!err.contains(&id));
    }

    #[test]
    fn filesystem_root_is_not_packed_as_the_root_home() {
        let err = archive_request("/").unwrap_err();
        assert!(err.contains("根目录"));
        assert!(archive_request("//").is_err());
        assert!(archive_request(" / ").is_err());
        let home = archive_request("/root").unwrap();
        assert_eq!(home.command, "tar -czf - -C '/' -- 'root'");
        assert_eq!(home.filename, "root.tar.gz");
        assert!(!home.command.contains("-C '/' '.'"));
    }

    #[test]
    fn archive_command_uses_the_parent_and_the_leaf() {
        let log = archive_request("/var/log/").unwrap();
        assert_eq!(log.command, "tar -czf - -C '/var' -- 'log'");
        assert_eq!(log.filename, "log.tar.gz");
        let quoted = archive_request("/tmp/o'brien").unwrap();
        assert_eq!(quoted.command, "tar -czf - -C '/tmp' -- 'o'\\''brien'");
        let dashed = archive_request("/tmp/qa/-safe").unwrap();
        assert_eq!(dashed.command, "tar -czf - -C '/tmp/qa' -- '-safe'");
        let long = archive_request("/tmp/qa/--exclude").unwrap();
        assert_eq!(long.command, "tar -czf - -C '/tmp/qa' -- '--exclude'");
        assert_eq!(archive_request("/tmp/a;rm").unwrap_err(), "路径包含非法字符");
    }

    #[test]
    fn failed_tar_is_not_a_completed_download() {
        assert_eq!(archive_failure(0, "anything"), None);
        assert_eq!(archive_failure(2, "").as_deref(), Some("远端打包失败（退出码 2）"));
        let message = archive_failure(2, "tar: /var/missing: Cannot stat\n").unwrap();
        assert!(message.contains("退出码 2"));
        assert!(message.contains("Cannot stat"));
        assert!(!message.contains('\n'));
    }

    #[test]
    fn file_download_rejects_a_directory() {
        use crate::sftp::ReadNode;
        assert_eq!(file_download_rejected(DownloadKind::File, ReadNode::Directory), Some("这是目录，请用打包下载"));
        assert_eq!(file_download_rejected(DownloadKind::File, ReadNode::Regular), None);
        assert_eq!(file_download_rejected(DownloadKind::File, ReadNode::Special), Some("这不是普通文件，没有开始下载"));
        assert_eq!(file_download_rejected(DownloadKind::Archive, ReadNode::Directory), None);
        assert_eq!(file_download_rejected(DownloadKind::Archive, ReadNode::Special), None);
    }

    /// GNU tar 1.35：`tar -C parent '-safe'` 退出码 64，把名字当成 `--safe`。
    /// `--` 之后同一目录能打出来，成员里有 `note.txt`。
    #[cfg(unix)]
    #[test]
    fn tar_archives_dash_prefixed_names_as_directories() {
        let root = std::env::temp_dir().join(format!("ts-archive-{}", std::process::id()));
        let parent = root.join("qa");
        for name in ["-safe", "--exclude"] {
            let leaf = parent.join(name);
            std::fs::create_dir_all(&leaf).unwrap();
            std::fs::write(leaf.join("note.txt"), b"marker").unwrap();
            let remote = leaf.to_string_lossy().into_owned();
            let req = archive_request(&remote).unwrap();
            assert!(req.command.contains(&format!(" -- '{}'", name)), "{}", req.command);
            let packed = std::process::Command::new("sh").arg("-c").arg(&req.command).output().expect("sh");
            let stderr = String::from_utf8_lossy(&packed.stderr);
            assert!(packed.status.success(), "{name}: {stderr}");
            let mut listed = std::process::Command::new("tar")
                .args(["-tzf", "-"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .expect("tar");
            std::io::Write::write_all(&mut listed.stdin.take().unwrap(), &packed.stdout).unwrap();
            let listed = listed.wait_with_output().unwrap();
            let members = String::from_utf8_lossy(&listed.stdout);
            assert!(members.contains(&format!("{name}/note.txt")), "{name}: {members}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_grant_does_not_echo_the_id() {
        let mut grants = DownloadGrants::new();
        let err = grants.redeem("not-a-real-grant", Instant::now()).unwrap_err();
        assert!(!err.contains("not-a-real-grant"));
    }
}
