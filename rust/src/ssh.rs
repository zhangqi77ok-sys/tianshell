use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;
use base64::Engine;
use sha2::{Digest, Sha256};
use ssh2::Session;
use thiserror::Error;
use uuid::Uuid;
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
        let key_file = std::env::temp_dir().join(format!("ts_{}.key", Uuid::new_v4()));
        std::fs::write(&key_file, private_key)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600));
        }
        let _guard = TempKey(key_file.clone());
        sess.userauth_pubkey_file(&target.username, None, &key_file, target.passphrase.as_deref())?;
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
    let (mut current, fingerprint) = connect_direct(first)?;
    let mut learned = vec![(first.id.clone(), fingerprint)];
    if chain.len() == 1 {
        return Ok((current, learned));
    }
    for next in &chain[1..] {
        let (next_sess, next_fp) = connect_through(&current, next)?;
        learned.push((next.id.clone(), next_fp));
        current = next_sess;
    }
    Ok((current, learned))
}

fn connect_through(current: &Session, next: &SshTarget) -> Result<(Session, String), SshError> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let local_port = listener.local_addr()?.port();
    let target_host = next.host.clone();
    let target_port = next.port;
    let channel = current.channel_direct_tcpip(&target_host, target_port, None)?;
    std::thread::spawn(move || {
        if let Ok((mut client_stream, _)) = listener.accept() {
            let client_clone = client_stream.try_clone().ok();
            let mut channel_stream = channel.stream(0);
            let mut channel_clone = channel.stream(0);
            let copy = std::thread::spawn(move || {
                let _ = std::io::copy(&mut channel_stream, &mut client_stream);
            });
            if let Some(mut cloned) = client_clone {
                let _ = std::io::copy(&mut cloned, &mut channel_clone);
            }
            let _ = copy.join();
        }
    });
    let tcp = TcpStream::connect(("127.0.0.1", local_port))?;
    let mut next_sess = Session::new()?;
    next_sess.set_tcp_stream(tcp);
    next_sess.handshake()?;
    let fingerprint = check_host_key(&next_sess, next.known_fingerprint.as_deref())?;
    authenticate_session(&next_sess, next)?;
    Ok((next_sess, fingerprint))
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
