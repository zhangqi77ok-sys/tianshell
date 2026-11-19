use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::time::Duration;
use ssh2::Session;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    pub private_key: Option<String>,
    pub passphrase: Option<String>,
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
}

fn authenticate_session(sess: &Session, target: &SshTarget) -> Result<(), SshError> {
    if let Some(pw) = &target.password {
        sess.userauth_password(&target.username, pw)?;
    } else if let Some(pk) = &target.private_key {
        let key_file = std::env::temp_dir().join(format!("ts_{}.key", Uuid::new_v4()));
        std::fs::write(&key_file, pk).map_err(SshError::Io)?;
        sess.userauth_pubkey_file(
            &target.username,
            None,
            &key_file,
            target.passphrase.as_deref(),
        )?;
        let _ = std::fs::remove_file(key_file);
    }
    if !sess.authenticated() {
        return Err(SshError::AuthFailed);
    }
    Ok(())
}

pub fn connect_direct(target: &SshTarget) -> Result<Session, SshError> {
    let addr = format!("{}:{}", target.host, target.port);
    let tcp = TcpStream::connect_timeout(
        &addr.parse().map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?,
        Duration::from_secs(10),
    )?;
    let mut sess = Session::new()?;
    sess.set_tcp_stream(tcp);
    sess.handshake()?;
    authenticate_session(&sess, target)?;
    Ok(sess)
}

pub fn connect_chain(chain: &[SshTarget]) -> Result<Session, SshError> {
    let first = chain.first().ok_or(SshError::EmptyChain)?;
    if chain.len() == 1 {
        return connect_direct(first);
    }

    let mut current_sess = connect_direct(first)?;
    for next_target in &chain[1..] {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let local_port = listener.local_addr()?.port();
        let target_host = next_target.host.clone();
        let target_port = next_target.port;

        let channel = current_sess.channel_direct_tcpip(&target_host, target_port, None)?;
        std::thread::spawn(move || {
            if let Ok((mut client_stream, _)) = listener.accept() {
                let client_clone = client_stream.try_clone().ok();
                let mut channel_stream = channel.stream(0);
                let mut channel_clone = channel.stream(0);

                let t1 = std::thread::spawn(move || {
                    let _ = std::io::copy(&mut channel_stream, &mut client_stream);
                });
                if let Some(mut cc) = client_clone {
                    let _ = std::io::copy(&mut cc, &mut channel_clone);
                }
                let _ = t1.join();
            }
        });

        let tcp = TcpStream::connect(("127.0.0.1", local_port))?;
        let mut next_sess = Session::new()?;
        next_sess.set_tcp_stream(tcp);
        next_sess.handshake()?;
        authenticate_session(&next_sess, next_target)?;
        current_sess = next_sess;
    }
    Ok(current_sess)
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

    Ok(ExecResult {
        exit_code,
        stdout,
        stderr,
    })
}
