use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use serde_json::{json, Value};
use ssh2::{Channel, Session};
use crate::ssh::{connect_chain, SshTarget};
use crate::state::lock;
use crate::types::TunnelRule;

struct ActiveTunnel {
    port: u16,
    conns: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

pub struct TunnelManager {
    active: Mutex<HashMap<String, ActiveTunnel>>,
    errors: Mutex<HashMap<String, String>>,
}

impl TunnelManager {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(HashMap::new()),
            errors: Mutex::new(HashMap::new()),
        }
    }

    pub fn start(&self, rule: &TunnelRule, chain: &[SshTarget]) -> Result<u16, String> {
        self.stop(&rule.id);
        lock(&self.errors).remove(&rule.id);
        let listener = if rule.mode == "remote" {
            None
        } else {
            let bound = TcpListener::bind((rule.local_host.as_str(), rule.local_port))
                .map_err(|err| format!("无法监听 {}:{}: {err}", rule.local_host, rule.local_port))?;
            let _ = bound.set_nonblocking(true);
            Some(bound)
        };
        let (session, _) = connect_chain(chain).map_err(|err| {
            let message = err.to_string();
            lock(&self.errors).insert(rule.id.clone(), message.clone());
            message
        })?;
        let stop = Arc::new(AtomicBool::new(false));
        let conns = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        let mode = rule.mode.clone();
        let local_host = rule.local_host.clone();
        let local_port = rule.local_port;
        let remote_host = rule.remote_host.clone().unwrap_or_else(|| "127.0.0.1".into());
        let remote_port = rule.remote_port.unwrap_or(80);
        let stop_thread = stop.clone();
        let conns_thread = conns.clone();
        let join = thread::spawn(move || {
            if mode == "remote" {
                remote_loop(session, local_host, local_port, remote_host, remote_port, stop_thread, conns_thread, tx);
            } else if let Some(listener) = listener {
                local_loop(
                    session,
                    listener,
                    mode == "socks5",
                    remote_host,
                    remote_port,
                    stop_thread,
                    conns_thread,
                    tx,
                );
            } else {
                let _ = tx.send(Err("没有监听套接字".into()));
            }
        });
        let port = match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(Ok(port)) => port,
            Ok(Err(err)) => {
                stop.store(true, Ordering::Relaxed);
                let _ = join.join();
                lock(&self.errors).insert(rule.id.clone(), err.clone());
                return Err(err);
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                let _ = join.join();
                let err = "隧道启动超时".to_string();
                lock(&self.errors).insert(rule.id.clone(), err.clone());
                return Err(err);
            }
        };
        lock(&self.active).insert(
            rule.id.clone(),
            ActiveTunnel {
                port,
                conns,
                stop,
                join: Some(join),
            },
        );
        Ok(port)
    }

    pub fn stop(&self, id: &str) {
        let active = lock(&self.active).remove(id);
        if let Some(mut active) = active {
            active.stop.store(true, Ordering::Relaxed);
            if let Some(join) = active.join.take() {
                let _ = join.join();
            }
        }
    }

    pub fn stop_all(&self) {
        let ids: Vec<String> = lock(&self.active).keys().cloned().collect();
        for id in ids {
            self.stop(&id);
        }
    }

    pub fn runtimes(&self, rules: &[TunnelRule]) -> Vec<Value> {
        let active = lock(&self.active);
        let errors = lock(&self.errors);
        rules
            .iter()
            .map(|rule| {
                let mut value = serde_json::to_value(rule).unwrap_or_else(|_| json!({}));
                let running = active.get(&rule.id);
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("active".into(), json!(running.is_some()));
                    obj.insert(
                        "connections".into(),
                        json!(running.map(|item| item.conns.load(Ordering::Relaxed)).unwrap_or(0)),
                    );
                    if let Some(item) = running {
                        obj.insert("localPort".into(), json!(item.port));
                    }
                    if let Some(err) = errors.get(&rule.id) {
                        obj.insert("error".into(), json!(err));
                    }
                }
                value
            })
            .collect()
    }
}

impl Default for TunnelManager {
    fn default() -> Self {
        Self::new()
    }
}

fn local_loop(
    session: Session,
    listener: TcpListener,
    socks: bool,
    remote_host: String,
    remote_port: u16,
    stop: Arc<AtomicBool>,
    conns: Arc<AtomicUsize>,
    ready: mpsc::Sender<Result<u16, String>>,
) {
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(err) => {
            let _ = ready.send(Err(err.to_string()));
            return;
        }
    };
    let _ = ready.send(Ok(port));
    let mut pairs: Vec<(TcpStream, Channel)> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nodelay(true);
                let opened = if socks {
                    socks_accept(&session, &mut stream)
                } else {
                    open_direct(&session, &remote_host, remote_port)
                        .map(|channel| {
                            let _ = stream.set_nonblocking(true);
                            channel
                        })
                        .map_err(|err| err.to_string())
                };
                match opened {
                    Ok(channel) => {
                        conns.fetch_add(1, Ordering::Relaxed);
                        pairs.push((stream, channel));
                    }
                    Err(_) => {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                    }
                }
            }
            Err(err) if retry_io(&err) => {}
            Err(_) => break,
        }
        sweep(&session, &mut pairs, &conns);
        thread::sleep(Duration::from_millis(8));
    }
}

fn remote_loop(
    session: Session,
    bind_host: String,
    bind_port: u16,
    dst_host: String,
    dst_port: u16,
    stop: Arc<AtomicBool>,
    conns: Arc<AtomicUsize>,
    ready: mpsc::Sender<Result<u16, String>>,
) {
    session.set_blocking(true);
    let listened = session.channel_forward_listen(bind_port, Some(bind_host.as_str()), None);
    let (mut listener, port) = match listened {
        Ok(pair) => pair,
        Err(err) => {
            let _ = ready.send(Err(format!("远程转发失败: {err}")));
            return;
        }
    };
    let _ = ready.send(Ok(port));
    session.set_blocking(false);
    let mut pairs: Vec<(TcpStream, Channel)> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok(mut channel) => {
                if let Some(addr) = (dst_host.as_str(), dst_port).to_socket_addrs().ok().and_then(|mut it| it.next()) {
                    if let Ok(stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(8)) {
                        let _ = stream.set_nonblocking(true);
                        conns.fetch_add(1, Ordering::Relaxed);
                        pairs.push((stream, channel));
                    } else {
                        session.set_blocking(true);
                        let _ = channel.close();
                        session.set_blocking(false);
                    }
                }
            }
            Err(err) if ssh_retry(&err) => {}
            Err(_) => break,
        }
        sweep(&session, &mut pairs, &conns);
        thread::sleep(Duration::from_millis(8));
    }
}

fn open_direct(session: &Session, host: &str, port: u16) -> Result<Channel, ssh2::Error> {
    session.set_blocking(true);
    let channel = session.channel_direct_tcpip(host, port, None)?;
    session.set_blocking(false);
    Ok(channel)
}

fn socks_accept(session: &Session, stream: &mut TcpStream) -> Result<Channel, String> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(8)));
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).map_err(|err| err.to_string())?;
    if header[0] != 0x05 {
        return Err("不是 SOCKS5 握手".into());
    }
    let mut methods = vec![0u8; header[1] as usize];
    if !methods.is_empty() {
        stream.read_exact(&mut methods).map_err(|err| err.to_string())?;
    }
    stream.write_all(&[0x05, 0x00]).map_err(|err| err.to_string())?;
    let mut request = [0u8; 4];
    stream.read_exact(&mut request).map_err(|err| err.to_string())?;
    if request[0] != 0x05 || request[1] != 0x01 {
        let _ = stream.write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0]);
        return Err("不支持的 SOCKS5 命令".into());
    }
    let target = read_socks_target(stream, request[3]).ok_or_else(|| "无法解析 SOCKS5 目标".to_string())?;
    match open_direct(session, &target.0, target.1) {
        Ok(channel) => {
            let _ = stream.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]);
            let _ = stream.set_nonblocking(true);
            Ok(channel)
        }
        Err(err) => {
            let _ = stream.write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0]);
            Err(err.to_string())
        }
    }
}

fn read_socks_target(stream: &mut TcpStream, atyp: u8) -> Option<(String, u16)> {
    match atyp {
        0x01 => {
            let mut buf = [0u8; 6];
            stream.read_exact(&mut buf).ok()?;
            Some((format!("{}.{}.{}.{}", buf[0], buf[1], buf[2], buf[3]), u16::from_be_bytes([buf[4], buf[5]])))
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).ok()?;
            let mut host = vec![0u8; len[0] as usize];
            stream.read_exact(&mut host).ok()?;
            let mut port = [0u8; 2];
            stream.read_exact(&mut port).ok()?;
            Some((String::from_utf8(host).ok()?, u16::from_be_bytes(port)))
        }
        0x04 => {
            let mut buf = [0u8; 18];
            stream.read_exact(&mut buf).ok()?;
            let host = buf[..16]
                .chunks(2)
                .map(|chunk| format!("{:x}", u16::from_be_bytes([chunk[0], chunk[1]])))
                .collect::<Vec<_>>()
                .join(":");
            Some((host, u16::from_be_bytes([buf[16], buf[17]])))
        }
        _ => None,
    }
}

fn sweep(session: &Session, pairs: &mut Vec<(TcpStream, Channel)>, conns: &AtomicUsize) {
    let mut index = 0;
    while index < pairs.len() {
        let pair = &mut pairs[index];
        if pump(session, &mut pair.0, &mut pair.1) {
            index += 1;
        } else {
            conns.fetch_sub(1, Ordering::Relaxed);
            pairs.remove(index);
        }
    }
}

fn pump(session: &Session, stream: &mut TcpStream, channel: &mut Channel) -> bool {
    let mut buf = [0u8; 8192];
    match stream.read(&mut buf) {
        Ok(0) => return false,
        Ok(n) => {
            session.set_blocking(true);
            let ok = channel.write_all(&buf[..n]).is_ok();
            session.set_blocking(false);
            if !ok {
                return false;
            }
        }
        Err(err) if retry_io(&err) => {}
        Err(_) => return false,
    }
    match channel.read(&mut buf) {
        Ok(0) => return false,
        Ok(n) => {
            if stream.write_all(&buf[..n]).is_err() {
                return false;
            }
        }
        Err(err) if retry_io(&err) => {}
        Err(_) => return false,
    }
    true
}

fn retry_io(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted
    )
}

fn ssh_retry(err: &ssh2::Error) -> bool {
    let text = err.to_string().to_ascii_lowercase();
    text.contains("eagain") || text.contains("would block") || text.contains("try again")
}
