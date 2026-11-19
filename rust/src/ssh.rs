use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;
use base64::Engine;
use sha2::{Digest, Sha256};
use ssh2::Session;
use thiserror::Error;
use uuid::Uuid;
use crate::keys::prepare_private_key;
use crate::store::HostStore;
use crate::types::{AuthType, HostRecord};

#[derive(Debug, Clone)]
pub struct SshTarget {
    pub id: String,
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    pub private_key: Option<String>,
    pub passphrase: Option<String>,
    pub known_fingerprint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Error)]
pub enum SshError {
    #[error("SSH 连接错误: {0}")]
    Ssh(#[from] ssh2::Error),
    #[error("网络 IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("认证失败: 请检查用户名、密码或私钥")]
    AuthFailed,
    #[error("跳板机链为空")]
    EmptyChain,
    #[error("{0}")]
    HostKey(String),
    #[error("{0}")]
    Message(String),
}

struct TempKey(PathBuf);

impl Drop for TempKey {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub fn resolve_targets(hosts: &HostStore, query: &str) -> Result<Vec<SshTarget>, String> {
    let needle = query.trim();
    if needle.is_empty() {
        return Err("请指定主机 id 或别名".into());
    }
    let all = hosts.list().to_vec();
    let target = all
        .iter()
        .find(|h| h.id == needle || h.alias.eq_ignore_ascii_case(needle))
        .cloned()
        .ok_or_else(|| format!("没有找到主机「{needle}」。可以先调用 list_hosts 查看已保存的别名。"))?;
    let mut hops = Vec::new();
    let mut current = Some(target);
    let mut guard = 0;
    while let Some(record) = current {
        guard += 1;
        if guard > 16 || hops.iter().any(|hop: &SshTarget| hop.id == record.id) {
            return Err("跳板机引用存在循环依赖，请检查跳板机设置".into());
        }
        hops.insert(0, target_from(hosts, &record)?);
        current = record
            .jump_host_id
            .as_ref()
            .and_then(|jid| all.iter().find(|h| &h.id == jid).cloned());
    }
    Ok(hops)
}

fn target_from(hosts: &HostStore, record: &HostRecord) -> Result<SshTarget, String> {
    let (password, private_key, passphrase) = match record.auth_type {
        AuthType::Password => {
            let password = if record.secret.is_empty() {
                None
            } else {
                Some(hosts.open_secret(&record.secret).map_err(|err| err.to_string())?)
            };
            (password, None, None)
        }
        AuthType::PrivateKey => {
            if record.secret.is_empty() {
                return Err("这台主机没有保存私钥".into());
            }
            let private_key = hosts.open_secret(&record.secret).map_err(|err| err.to_string())?;
            let passphrase = match &record.passphrase {
                Some(sealed) if !sealed.is_empty() => Some(hosts.open_secret(sealed).map_err(|err| err.to_string())?),
                _ => None,
            };
            (None, Some(private_key), passphrase)
        }
    };
    Ok(SshTarget {
        id: record.id.clone(),
        alias: record.alias.clone(),
        host: record.host.clone(),
        port: record.port,
        username: record.username.clone(),
        password,
        private_key,
        passphrase,
        known_fingerprint: record.fingerprint.clone(),
    })
}

fn fingerprint_of(key: &[u8]) -> String {
    let digest = Sha256::digest(key);
    let encoded = base64::engine::general_purpose::STANDARD.encode(digest);
    format!("SHA256:{}", encoded.trim_end_matches('='))
}

fn check_host_key(sess: &Session, known: Option<&str>) -> Result<String, SshError> {
    let Some((key, _)) = sess.host_key() else {
        if known.map(|item| !item.is_empty()).unwrap_or(false) {
            return Err(SshError::HostKey("无法读取主机密钥，已保存的指纹无法核对".into()));
        }
        return Ok(String::new());
    };
    if key.is_empty() {
        return Ok(String::new());
    }
    let fingerprint = fingerprint_of(key);
    if let Some(known) = known {
        if !known.is_empty() && known != fingerprint {
            return Err(SshError::HostKey(format!(
                "主机密钥指纹和上次保存的不一致（{fingerprint}）。如果这台机器换过密钥，请在编辑主机时清除指纹后再连接。"
            )));
        }
    }
    Ok(fingerprint)
}

fn authenticate_session(sess: &Session, target: &SshTarget) -> Result<(), SshError> {
    if let Some(password) = &target.password {
        sess.userauth_password(&target.username, password)?;
    } else if let Some(private_key) = &target.private_key {
        let prepared = prepare_private_key(private_key, target.passphrase.as_deref()).map_err(SshError::Message)?;
        let key_file = std::env::temp_dir().join(format!("ts_{}.key", Uuid::new_v4()));
        std::fs::write(&key_file, prepared.material.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600));
        }
        let _guard = TempKey(key_file.clone());
        sess.userauth_pubkey_file(&target.username, None, &key_file, prepared.passphrase.as_deref())?;
    }
    if !sess.authenticated() {
        return Err(SshError::AuthFailed);
    }
    Ok(())
}

pub fn connect_direct(target: &SshTarget) -> Result<(Session, String), SshError> {
    let addr = format!("{}:{}", target.host, target.port);
    let tcp = TcpStream::connect_timeout(
        &addr.parse().map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?,
        Duration::from_secs(15),
    )?;
    let mut sess = Session::new()?;
    sess.set_tcp_stream(tcp);
    sess.handshake()?;
    let fingerprint = check_host_key(&sess, target.known_fingerprint.as_deref())?;
    authenticate_session(&sess, target)?;
    Ok((sess, fingerprint))
}

pub fn connect_chain(chain: &[SshTarget]) -> Result<(Session, Vec<(String, String)>), SshError> {
    let first = chain.first().ok_or(SshError::EmptyChain)?;
    let (mut current, fingerprint) = connect_direct(first).map_err(|err| {
        if chain.len() > 1 {
            SshError::Message(describe_hop_error(chain, 0, &err))
        } else {
            err
        }
    })?;
    let mut learned = vec![(first.id.clone(), fingerprint)];
    if chain.len() == 1 {
        return Ok((current, learned));
    }
    for (index, next) in chain.iter().enumerate().skip(1) {
        let (next_sess, next_fp) = connect_through(current, next)
            .map_err(|err| SshError::Message(describe_hop_error(chain, index, &err)))?;
        learned.push((next.id.clone(), next_fp));
        current = next_sess;
    }
    Ok((current, learned))
}

/// One libssh2 session is not safe to read and write from two threads.
/// The bastion session moves onto a single pump thread and stays there until
/// the next hop's socket closes. Dropping it early used to kill the tunnel
/// under the handshake, which is why a live jump sat until the client gave up.
fn connect_through(bastion: Session, next: &SshTarget) -> Result<(Session, String), SshError> {
    bastion.set_timeout(12_000);
    let channel = bastion.channel_direct_tcpip(&next.host, next.port, Some(("127.0.0.1", 0)))?;
    bastion.set_timeout(0);
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let app_stream = TcpStream::connect(("127.0.0.1", port))?;
    let (jump_stream, _) = listener.accept()?;
    drop(listener);
    app_stream.set_nodelay(true).ok();
    std::thread::Builder::new()
        .name("ssh-jump".into())
        .spawn(move || bridge_loop(bastion, channel, jump_stream))
        .map_err(SshError::Io)?;
    let mut next_sess = Session::new()?;
    next_sess.set_timeout(12_000);
    next_sess.set_tcp_stream(app_stream);
    next_sess.handshake()?;
    let fingerprint = check_host_key(&next_sess, next.known_fingerprint.as_deref())?;
    authenticate_session(&next_sess, next)?;
    next_sess.set_timeout(0);
    Ok((next_sess, fingerprint))
}

fn bridge_loop(session: Session, mut channel: ssh2::Channel, mut local: TcpStream) {
    session.set_blocking(true);
    session.set_timeout(40);
    let _ = local.set_nodelay(true);
    let _ = local.set_read_timeout(Some(Duration::from_millis(40)));
    let _ = local.set_write_timeout(Some(Duration::from_secs(20)));
    let mut buf = [0u8; 16384];
    let mut pending = Vec::new();
    loop {
        match channel.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if local.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
            Err(err) if io_wait(&err) => {}
            Err(_) => break,
        }
        if pending.is_empty() {
            match local.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => pending.extend_from_slice(&buf[..n]),
                Err(err) if io_wait(&err) => {}
                Err(_) => break,
            }
        }
        if !pending.is_empty() {
            session.set_timeout(15_000);
            match channel.write(&pending) {
                Ok(0) => break,
                Ok(n) => {
                    pending.drain(..n);
                }
                Err(err) if io_wait(&err) => {}
                Err(_) => break,
            }
            session.set_timeout(40);
        }
    }
}

fn io_wait(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
    ) || {
        let text = err.to_string().to_ascii_lowercase();
        text.contains("timeout") || text.contains("would block") || text.contains("eagain")
    }
}

pub fn describe_hop_error(chain: &[SshTarget], hop_index: usize, err: &SshError) -> String {
    if chain.len() > 1 && hop_index == 0 {
        let hop = &chain[0];
        return format!(
            "跳板机「{}」({}:{}) 连接失败: {err}",
            hop.alias, hop.host, hop.port
        );
    }
    if hop_index > 0 {
        if let (Some(via), Some(dest)) = (chain.get(hop_index - 1), chain.get(hop_index)) {
            return format!(
                "经跳板机「{}」({}:{}) 连接「{}」({}:{}) 失败: {err}",
                via.alias, via.host, via.port, dest.alias, dest.host, dest.port
            );
        }
    }
    err.to_string()
}

pub fn exec_command(sess: &Session, command: &str) -> Result<ExecResult, SshError> {
    let mut channel = sess.channel_session()?;
    channel.exec(command)?;
    let mut stdout = String::new();
    channel.read_to_string(&mut stdout)?;
    let mut stderr = String::new();
    channel.stderr().read_to_string(&mut stderr)?;
    channel.wait_close()?;
    let exit_code = channel.exit_status()?;
    Ok(ExecResult { exit_code, stdout, stderr })
}

pub fn truncate_output(text: String, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text, false);
    }
    let trimmed: String = text.chars().take(max_chars).collect();
    (format!("{trimmed}\n…（输出已截断）"), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hop(alias: &str, host: &str, port: u16) -> SshTarget {
        SshTarget {
            id: alias.into(),
            alias: alias.into(),
            host: host.into(),
            port,
            username: "ops".into(),
            password: None,
            private_key: None,
            passphrase: None,
            known_fingerprint: None,
        }
    }

    #[test]
    fn jump_errors_name_the_bastion() {
        let chain = vec![
            hop("lab", "127.0.0.1", 2222),
            hop("内网", "127.0.0.1", 2223),
        ];
        let refused = SshError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "Connection refused (os error 111)",
        ));
        let first = describe_hop_error(&chain, 0, &refused);
        assert!(first.contains("跳板机「lab」"), "{first}");
        assert!(first.contains("127.0.0.1:2222"), "{first}");
        assert!(first.contains("Connection refused"), "{first}");
        let second = describe_hop_error(&chain, 1, &refused);
        assert!(second.contains("经跳板机「lab」"), "{second}");
        assert!(second.contains("「内网」"), "{second}");
        assert!(second.contains("2223"), "{second}");
        let direct = vec![hop("lab", "127.0.0.1", 2222)];
        let plain = describe_hop_error(&direct, 0, &refused);
        assert!(!plain.contains("跳板机"), "{plain}");
    }
}
