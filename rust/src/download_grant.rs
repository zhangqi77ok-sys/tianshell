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
    fn unknown_grant_does_not_echo_the_id() {
        let mut grants = DownloadGrants::new();
        let err = grants.redeem("not-a-real-grant", Instant::now()).unwrap_err();
        assert!(!err.contains("not-a-real-grant"));
    }
}
