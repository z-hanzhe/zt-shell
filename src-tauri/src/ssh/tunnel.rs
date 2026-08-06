//! SSH 隧道运行时：本地拨出、远程传入、动态 SOCKS4/5 与动态 HTTP 转发

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use russh::client;
use tokio::io::{copy_bidirectional, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::session::SshSession;
use super::types::{ExtensionEntry, ExtensionKind, TunnelConfig, TunnelType};

const HTTP_HEADER_LIMIT: usize = 64 * 1024;
/// 远程传入隧道回连客户端侧目标的最长等待时间
const REMOTE_TARGET_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// 远程传入隧道的客户端侧目标
#[derive(Clone)]
pub struct RemoteTunnelTarget {
    /// 目标主机
    host: String,
    /// 目标端口
    port: u16,
    /// 会话级隧道取消信号
    cancellation: watch::Receiver<bool>,
}

/// 远程传入隧道映射表
#[derive(Clone, Default)]
pub struct RemoteTunnelRegistry {
    targets: Arc<DashMap<u32, RemoteTunnelTarget>>,
}

impl RemoteTunnelRegistry {
    /// 登记远程监听端口对应的客户端侧目标
    pub fn insert(&self, port: u32, target: RemoteTunnelTarget) {
        self.targets.insert(port, target);
    }

    /// 移除远程监听端口映射
    pub fn remove(&self, port: u32) {
        self.targets.remove(&port);
    }

    /// 读取远程监听端口对应的客户端侧目标
    pub fn get(&self, port: u32) -> Option<RemoteTunnelTarget> {
        self.targets.get(&port).map(|target| target.clone())
    }
}

/// 已预绑定的本机监听器，避免 SSH 认证期间端口被其他进程抢占
#[derive(Default)]
pub struct PreparedTunnelListeners {
    listeners: HashMap<String, TcpListener>,
    failures: HashMap<String, String>,
}

impl PreparedTunnelListeners {
    /// 取出指定隧道的预绑定监听器
    fn take(&mut self, tunnel: &TunnelConfig) -> Option<TcpListener> {
        self.listeners.remove(&tunnel.id)
    }

    /// 读取指定隧道预绑定失败原因
    fn take_failure(&mut self, tunnel: &TunnelConfig) -> Option<String> {
        self.failures.remove(&tunnel.id)
    }
}

/// 隧道启动结果，失败项以条目状态返回，不阻断 SSH 主连接
pub struct TunnelStartResult {
    /// 已启动的本地监听任务
    pub tasks: Vec<JoinHandle<()>>,
    /// 各启用隧道的启动情况，顺序与连接配置一致
    pub entries: Vec<ExtensionEntry>,
}

/// 连接 SSH 前预绑定需要监听本机端口的隧道
pub async fn prepare_listeners(tunnels: &[TunnelConfig]) -> PreparedTunnelListeners {
    let mut prepared = PreparedTunnelListeners::default();
    for tunnel in tunnels.iter().filter(|tunnel| tunnel.enabled) {
        if !matches!(
            tunnel.tunnel_type,
            TunnelType::Local | TunnelType::Dynamic | TunnelType::DynamicHttp
        ) {
            continue;
        }
        let result = async {
            validate_listen_port(tunnel.listen_port)?;
            bind_listener(tunnel).await
        }
        .await;
        match result {
            Ok(listener) => {
                prepared.listeners.insert(tunnel.id.clone(), listener);
            }
            Err(error) => {
                prepared.failures.insert(tunnel.id.clone(), error.to_string());
            }
        }
    }
    prepared
}

/// 启动当前会话启用的全部隧道
pub async fn start_tunnels(
    session: Arc<SshSession>,
    tunnels: &[TunnelConfig],
    mut prepared: PreparedTunnelListeners,
    cancellation: watch::Receiver<bool>,
) -> TunnelStartResult {
    let mut tasks = Vec::new();
    // 各隧道启动失败原因，成功的隧道不入表
    let mut failures: HashMap<String, String> = HashMap::new();

    // 远程监听需要独占 SSH 句柄，先完成申请再开放本地监听任务接收连接
    for tunnel in tunnels
        .iter()
        .filter(|tunnel| tunnel.enabled && tunnel.tunnel_type == TunnelType::Remote)
    {
        if let Err(error) = start_remote_tunnel(session.clone(), tunnel, cancellation.clone()).await
        {
            failures.insert(tunnel.id.clone(), error.to_string());
        }
    }

    for tunnel in tunnels
        .iter()
        .filter(|tunnel| tunnel.enabled && tunnel.tunnel_type != TunnelType::Remote)
    {
        match tunnel.tunnel_type {
            TunnelType::Local => {
                let Some(listener) = take_prepared_listener(tunnel, &mut prepared, &mut failures)
                else {
                    continue;
                };
                if let Err(error) = target(tunnel) {
                    failures.insert(tunnel.id.clone(), error.to_string());
                    continue;
                }
                let session = session.clone();
                let tunnel = tunnel.clone();
                let cancellation = cancellation.clone();
                tasks.push(tokio::spawn(async move {
                    run_local_tunnel(session, tunnel, listener, cancellation).await;
                }));
            }
            TunnelType::Dynamic => {
                let Some(listener) = take_prepared_listener(tunnel, &mut prepared, &mut failures)
                else {
                    continue;
                };
                let session = session.clone();
                let tunnel = tunnel.clone();
                let cancellation = cancellation.clone();
                tasks.push(tokio::spawn(async move {
                    run_dynamic_tunnel(session, tunnel, listener, cancellation).await;
                }));
            }
            TunnelType::DynamicHttp => {
                let Some(listener) = take_prepared_listener(tunnel, &mut prepared, &mut failures)
                else {
                    continue;
                };
                let session = session.clone();
                let tunnel = tunnel.clone();
                let cancellation = cancellation.clone();
                tasks.push(tokio::spawn(async move {
                    run_http_tunnel(session, tunnel, listener, cancellation).await;
                }));
            }
            TunnelType::Remote => {}
        }
    }

    // 按连接配置顺序汇总启用隧道的最终状态，保证面板展示顺序稳定
    let entries = tunnels
        .iter()
        .filter(|tunnel| tunnel.enabled)
        .map(|tunnel| tunnel_entry(tunnel, failures.remove(&tunnel.id)))
        .collect();
    TunnelStartResult { tasks, entries }
}

/// 取监听器；预绑定失败的隧道直接记录失败原因
fn take_prepared_listener(
    tunnel: &TunnelConfig,
    prepared: &mut PreparedTunnelListeners,
    failures: &mut HashMap<String, String>,
) -> Option<TcpListener> {
    if let Some(reason) = prepared.take_failure(tunnel) {
        failures.insert(tunnel.id.clone(), reason);
        return None;
    }
    if let Some(listener) = prepared.take(tunnel) {
        return Some(listener);
    }
    failures.insert(tunnel.id.clone(), "监听器未准备".to_string());
    None
}

/// 隧道类型的中文名称
fn tunnel_category(tunnel_type: &TunnelType) -> &'static str {
    match tunnel_type {
        TunnelType::Local => "本地拨出",
        TunnelType::Remote => "远程传入",
        TunnelType::Dynamic => "动态 SOCKS4/5",
        TunnelType::DynamicHttp => "动态 HTTP",
    }
}

/// 隧道的转发链路描述
fn tunnel_detail(tunnel: &TunnelConfig) -> String {
    let scope = if tunnel.local_only {
        "仅本机"
    } else {
        "允许外部"
    };
    let host = tunnel.target_host.as_deref().unwrap_or("").trim();
    let port = tunnel.target_port.unwrap_or(0);
    match tunnel.tunnel_type {
        TunnelType::Local => format!(
            "本机 {} → 服务器侧 {}:{}（{}）",
            tunnel.listen_port, host, port, scope
        ),
        TunnelType::Remote => format!(
            "服务器 {} → 本机侧 {}:{}（{}）",
            tunnel.listen_port, host, port, scope
        ),
        TunnelType::Dynamic | TunnelType::DynamicHttp => {
            format!("本机 {} 动态转发（{}）", tunnel.listen_port, scope)
        }
    }
}

/// 生成单条隧道的扩展条目，failure 为空表示启动成功
fn tunnel_entry(tunnel: &TunnelConfig, failure: Option<String>) -> ExtensionEntry {
    ExtensionEntry {
        kind: ExtensionKind::Tunnel,
        name: tunnel.name.clone(),
        category: tunnel_category(&tunnel.tunnel_type).to_string(),
        detail: tunnel_detail(tunnel),
        ok: failure.is_none(),
        error: failure.unwrap_or_default(),
    }
}

/// 处理远程传入隧道中新到达的 forwarded-tcpip 通道
pub async fn forward_remote_tunnel_channel(
    channel: russh::Channel<client::Msg>,
    target: RemoteTunnelTarget,
) -> Result<()> {
    let stream = timeout(
        REMOTE_TARGET_CONNECT_TIMEOUT,
        TcpStream::connect((target.host.as_str(), target.port)),
    )
    .await
    .map_err(|_| anyhow!("连接远程隧道目标超时：{}:{}", target.host, target.port))?
    .map_err(|e| anyhow!("连接远程隧道目标失败：{}", e))?;
    bridge_tcp_and_channel(stream, channel, target.cancellation).await
}

/// 等待会话级隧道取消信号
async fn wait_cancelled(cancellation: &mut watch::Receiver<bool>) {
    if *cancellation.borrow() {
        return;
    }
    while cancellation.changed().await.is_ok() {
        if *cancellation.borrow() {
            return;
        }
    }
}

/// 校验本地监听端口
fn validate_listen_port(port: u16) -> Result<()> {
    if port == 0 {
        return Err(anyhow!("隧道监听端口必须在 1 到 65535 之间"));
    }
    Ok(())
}

/// 根据仅本地连接选项计算监听地址
fn bind_host(tunnel: &TunnelConfig) -> &'static str {
    if tunnel.local_only {
        "127.0.0.1"
    } else {
        "0.0.0.0"
    }
}

/// 绑定隧道监听器
async fn bind_listener(tunnel: &TunnelConfig) -> Result<TcpListener> {
    match TcpListener::bind((bind_host(tunnel), tunnel.listen_port)).await {
        Ok(listener) => Ok(listener),
        Err(error) if error.kind() == ErrorKind::AddrInUse => Err(anyhow!(
            "{} 端口被占用，请关闭隧道或调整端口后重试",
            tunnel.listen_port
        )),
        Err(error) => Err(anyhow!("监听 {} 端口失败：{}", tunnel.listen_port, error)),
    }
}

/// 读取本地/远程隧道的目标主机和端口
fn target(tunnel: &TunnelConfig) -> Result<(String, u16)> {
    let host = tunnel.target_host.as_deref().unwrap_or("").trim();
    if host.is_empty() {
        return Err(anyhow!("缺少目标主机"));
    }
    let port = tunnel
        .target_port
        .ok_or_else(|| anyhow!("缺少目标端口"))?;
    if port == 0 {
        return Err(anyhow!("目标端口必须在 1 到 65535 之间"));
    }
    Ok((host.to_string(), port))
}

/// 启动本地拨出隧道监听循环
async fn run_local_tunnel(
    session: Arc<SshSession>,
    tunnel: TunnelConfig,
    listener: TcpListener,
    mut cancellation: watch::Receiver<bool>,
) {
    let Ok((target_host, target_port)) = target(&tunnel) else {
        return;
    };
    loop {
        tokio::select! {
            _ = wait_cancelled(&mut cancellation) => break,
            accepted = listener.accept() => {
                let Ok((stream, origin)) = accepted else {
                    break;
                };
                let session = session.clone();
                let target_host = target_host.clone();
                let cancellation = cancellation.clone();
                tokio::spawn(async move {
                    let _ = handle_local_tunnel_client(
                        session,
                        stream,
                        origin,
                        target_host,
                        target_port,
                        cancellation,
                    )
                    .await;
                });
            }
        }
    }
}

/// 处理本地拨出隧道单个客户端连接
async fn handle_local_tunnel_client(
    session: Arc<SshSession>,
    stream: TcpStream,
    origin: SocketAddr,
    target_host: String,
    target_port: u16,
    cancellation: watch::Receiver<bool>,
) -> Result<()> {
    let channel = session
        .open_direct_tcpip(
            &target_host,
            target_port,
            &origin.ip().to_string(),
            origin.port(),
            cancellation.clone(),
        )
        .await?;
    bridge_tcp_and_channel(stream, channel, cancellation).await
}

/// 启动远程传入隧道
async fn start_remote_tunnel(
    session: Arc<SshSession>,
    tunnel: &TunnelConfig,
    cancellation: watch::Receiver<bool>,
) -> Result<()> {
    validate_listen_port(tunnel.listen_port)?;
    let (target_host, target_port) = target(tunnel)?;
    let target = RemoteTunnelTarget {
        host: target_host,
        port: target_port,
        cancellation,
    };
    session
        .request_remote_forward(bind_host(tunnel), tunnel.listen_port, target)
        .await
        .map_err(|e| anyhow!("申请服务器端口监听失败：{}", e))
}

/// 启动动态 SOCKS4/5 隧道监听循环
async fn run_dynamic_tunnel(
    session: Arc<SshSession>,
    _tunnel: TunnelConfig,
    listener: TcpListener,
    mut cancellation: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = wait_cancelled(&mut cancellation) => break,
            accepted = listener.accept() => {
                let Ok((stream, origin)) = accepted else {
                    break;
                };
                let session = session.clone();
                let cancellation = cancellation.clone();
                tokio::spawn(async move {
                    let _ = handle_dynamic_client(session, stream, origin, cancellation).await;
                });
            }
        }
    }
}

/// 启动动态 HTTP 代理隧道监听循环
async fn run_http_tunnel(
    session: Arc<SshSession>,
    _tunnel: TunnelConfig,
    listener: TcpListener,
    mut cancellation: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = wait_cancelled(&mut cancellation) => break,
            accepted = listener.accept() => {
                let Ok((stream, origin)) = accepted else {
                    break;
                };
                let session = session.clone();
                let cancellation = cancellation.clone();
                tokio::spawn(async move {
                    let _ = handle_http_proxy_client(session, stream, origin, cancellation).await;
                });
            }
        }
    }
}

/// 处理动态 SOCKS 隧道单个客户端连接
async fn handle_dynamic_client(
    session: Arc<SshSession>,
    mut stream: TcpStream,
    origin: SocketAddr,
    cancellation: watch::Receiver<bool>,
) -> Result<()> {
    let version = stream.read_u8().await?;
    match version {
        4 => handle_socks4_client(session, stream, origin, cancellation).await,
        5 => handle_socks5_client(session, stream, origin, cancellation).await,
        _ => Err(anyhow!("不支持的 SOCKS 协议版本")),
    }
}

/// 处理 SOCKS5 CONNECT 请求
async fn handle_socks5_client(
    session: Arc<SshSession>,
    mut stream: TcpStream,
    origin: SocketAddr,
    cancellation: watch::Receiver<bool>,
) -> Result<()> {
    let method_count = stream.read_u8().await? as usize;
    let mut methods = vec![0; method_count];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        stream.write_all(&[5, 0xff]).await?;
        return Err(anyhow!("SOCKS5 客户端不支持无认证"));
    }
    stream.write_all(&[5, 0]).await?;

    let version = stream.read_u8().await?;
    let command = stream.read_u8().await?;
    let _reserved = stream.read_u8().await?;
    let address_type = stream.read_u8().await?;
    if version != 5 || command != 1 {
        write_socks5_reply(&mut stream, 7).await?;
        return Err(anyhow!("SOCKS5 仅支持 CONNECT 请求"));
    }
    let target_host = read_socks5_host(&mut stream, address_type).await?;
    let target_port = stream.read_u16().await?;
    let channel = match session
        .open_direct_tcpip(
            &target_host,
            target_port,
            &origin.ip().to_string(),
            origin.port(),
            cancellation.clone(),
        )
        .await
    {
        Ok(channel) => channel,
        Err(error) => {
            write_socks5_reply(&mut stream, 5).await?;
            return Err(error);
        }
    };
    write_socks5_reply(&mut stream, 0).await?;
    bridge_tcp_and_channel(stream, channel, cancellation).await
}

/// 读取 SOCKS5 目标主机
async fn read_socks5_host(stream: &mut TcpStream, address_type: u8) -> Result<String> {
    match address_type {
        1 => {
            let mut ip = [0; 4];
            stream.read_exact(&mut ip).await?;
            Ok(Ipv4Addr::from(ip).to_string())
        }
        3 => {
            let len = stream.read_u8().await? as usize;
            let mut bytes = vec![0; len];
            stream.read_exact(&mut bytes).await?;
            String::from_utf8(bytes).map_err(|_| anyhow!("SOCKS5 域名不是有效 UTF-8"))
        }
        4 => {
            let mut ip = [0; 16];
            stream.read_exact(&mut ip).await?;
            Ok(Ipv6Addr::from(ip).to_string())
        }
        _ => Err(anyhow!("SOCKS5 地址类型不支持")),
    }
}

/// 写入 SOCKS5 响应
async fn write_socks5_reply(stream: &mut TcpStream, code: u8) -> Result<()> {
    stream.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    Ok(())
}

/// 处理 SOCKS4/SOCKS4A CONNECT 请求
async fn handle_socks4_client(
    session: Arc<SshSession>,
    mut stream: TcpStream,
    origin: SocketAddr,
    cancellation: watch::Receiver<bool>,
) -> Result<()> {
    let command = stream.read_u8().await?;
    let target_port = stream.read_u16().await?;
    let mut ip = [0; 4];
    stream.read_exact(&mut ip).await?;
    let _user = read_cstring(&mut stream, 1024).await?;
    if command != 1 {
        write_socks4_reply(&mut stream, 0x5b, target_port, ip).await?;
        return Err(anyhow!("SOCKS4 仅支持 CONNECT 请求"));
    }
    let target_host = if ip[0] == 0 && ip[1] == 0 && ip[2] == 0 && ip[3] != 0 {
        read_cstring(&mut stream, 255).await?
    } else {
        Ipv4Addr::from(ip).to_string()
    };
    let channel = match session
        .open_direct_tcpip(
            &target_host,
            target_port,
            &origin.ip().to_string(),
            origin.port(),
            cancellation.clone(),
        )
        .await
    {
        Ok(channel) => channel,
        Err(error) => {
            write_socks4_reply(&mut stream, 0x5b, target_port, ip).await?;
            return Err(error);
        }
    };
    write_socks4_reply(&mut stream, 0x5a, target_port, ip).await?;
    bridge_tcp_and_channel(stream, channel, cancellation).await
}

/// 读取以 NUL 结尾的 SOCKS 字符串
async fn read_cstring(stream: &mut TcpStream, max_len: usize) -> Result<String> {
    let mut bytes = Vec::new();
    loop {
        let byte = stream.read_u8().await?;
        if byte == 0 {
            break;
        }
        if bytes.len() >= max_len {
            return Err(anyhow!("SOCKS 字符串长度超出限制"));
        }
        bytes.push(byte);
    }
    String::from_utf8(bytes).map_err(|_| anyhow!("SOCKS 字符串不是有效 UTF-8"))
}

/// 写入 SOCKS4 响应
async fn write_socks4_reply(
    stream: &mut TcpStream,
    code: u8,
    port: u16,
    ip: [u8; 4],
) -> Result<()> {
    let [high, low] = port.to_be_bytes();
    stream
        .write_all(&[0, code, high, low, ip[0], ip[1], ip[2], ip[3]])
        .await?;
    Ok(())
}

/// 动态 HTTP 代理解析后的首个请求
struct HttpProxyRequest {
    /// 目标主机
    host: String,
    /// 目标端口
    port: u16,
    /// 需要先写入 SSH 通道的普通 HTTP 请求头；CONNECT 请求为空
    forward_header: Option<Vec<u8>>,
}

/// 处理动态 HTTP 代理单个客户端连接
async fn handle_http_proxy_client(
    session: Arc<SshSession>,
    mut stream: TcpStream,
    origin: SocketAddr,
    cancellation: watch::Receiver<bool>,
) -> Result<()> {
    let header = match read_http_header(&mut stream).await {
        Ok(header) => header,
        Err(error) => {
            let _ = write_http_error(&mut stream, 400, "Bad Request").await;
            return Err(error);
        }
    };
    let request = match parse_http_proxy_request(&header) {
        Ok(request) => request,
        Err(error) => {
            let _ = write_http_error(&mut stream, 400, "Bad Request").await;
            return Err(error);
        }
    };
    let channel = match session
        .open_direct_tcpip(
            &request.host,
            request.port,
            &origin.ip().to_string(),
            origin.port(),
            cancellation.clone(),
        )
        .await
    {
        Ok(channel) => channel,
        Err(error) => {
            let _ = write_http_error(&mut stream, 502, "Bad Gateway").await;
            return Err(error);
        }
    };
    let mut channel_stream = channel.into_stream();
    if let Some(forward_header) = request.forward_header {
        channel_stream.write_all(&forward_header).await?;
    } else {
        stream
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
    }
    bridge_tcp_and_channel_stream(stream, channel_stream, cancellation).await
}

/// 读取 HTTP 代理请求头，保留请求体给后续双向转发处理
async fn read_http_header(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut header = Vec::new();
    while header.len() < HTTP_HEADER_LIMIT {
        let byte = stream.read_u8().await?;
        header.push(byte);
        if header.ends_with(b"\r\n\r\n") {
            return Ok(header);
        }
    }
    Err(anyhow!("HTTP 代理请求头超出限制"))
}

/// 解析 HTTP 代理请求，CONNECT 建隧道，普通 HTTP 请求改写为 origin-form
fn parse_http_proxy_request(header: &[u8]) -> Result<HttpProxyRequest> {
    let text = str::from_utf8(header).map_err(|_| anyhow!("HTTP 代理请求头不是有效 UTF-8"))?;
    let mut lines = text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| anyhow!("HTTP 代理请求缺少请求行"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| anyhow!("HTTP 代理请求缺少方法"))?;
    let target = parts
        .next()
        .ok_or_else(|| anyhow!("HTTP 代理请求缺少目标地址"))?;
    let version = parts
        .next()
        .ok_or_else(|| anyhow!("HTTP 代理请求缺少协议版本"))?;
    if !version.starts_with("HTTP/") {
        return Err(anyhow!("HTTP 代理请求协议版本不正确"));
    }

    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = parse_authority(target, 443)?;
        return Ok(HttpProxyRequest {
            host,
            port,
            forward_header: None,
        });
    }

    let (host, port, path) = if let Some((host, port, path)) = parse_absolute_http_uri(target)? {
        (host, port, path)
    } else if target.starts_with('/') {
        let host_header =
            find_host_header(text).ok_or_else(|| anyhow!("HTTP 代理请求缺少 Host 头"))?;
        let (host, port) = parse_authority(host_header, 80)?;
        (host, port, target.to_string())
    } else {
        return Err(anyhow!("HTTP 代理仅支持 CONNECT 或 http:// 绝对地址请求"));
    };

    let forward_header = rewrite_http_proxy_header(text, method, &path, version, &host, port)?;
    Ok(HttpProxyRequest {
        host,
        port,
        forward_header: Some(forward_header),
    })
}

/// 解析 http:// 绝对地址，返回目标主机、端口和 origin-form 路径
fn parse_absolute_http_uri(target: &str) -> Result<Option<(String, u16, String)>> {
    if target.len() < 7 || !target[..7].eq_ignore_ascii_case("http://") {
        return Ok(None);
    }
    let rest = &target[7..];
    let end = rest
        .char_indices()
        .find(|(_, ch)| matches!(ch, '/' | '?' | '#'))
        .map(|(index, _)| index)
        .unwrap_or(rest.len());
    let authority = &rest[..end];
    let suffix = &rest[end..];
    let mut path = if suffix.is_empty() || suffix.starts_with('#') {
        "/".to_string()
    } else if suffix.starts_with('?') {
        format!("/{}", suffix)
    } else {
        suffix.to_string()
    };
    if let Some(index) = path.find('#') {
        path.truncate(index);
        if path.is_empty() {
            path.push('/');
        }
    }
    let (host, port) = parse_authority(authority, 80)?;
    Ok(Some((host, port, path)))
}

/// 查找 HTTP Host 头
fn find_host_header(text: &str) -> Option<&str> {
    for line in text.split("\r\n").skip(1) {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("host") {
            return Some(value.trim());
        }
    }
    None
}

/// 将代理请求头改写为目标服务器可识别的普通 HTTP 请求头
fn rewrite_http_proxy_header(
    text: &str,
    method: &str,
    path: &str,
    version: &str,
    host: &str,
    port: u16,
) -> Result<Vec<u8>> {
    let mut rewritten = String::new();
    rewritten.push_str(method);
    rewritten.push(' ');
    rewritten.push_str(path);
    rewritten.push(' ');
    rewritten.push_str(version);
    rewritten.push_str("\r\n");

    let mut has_host = false;
    for line in text.split("\r\n").skip(1) {
        if line.is_empty() {
            break;
        }
        let Some((name, _)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("proxy-connection")
            || name.eq_ignore_ascii_case("proxy-authorization")
            || name.eq_ignore_ascii_case("connection")
        {
            continue;
        }
        if name.eq_ignore_ascii_case("host") {
            has_host = true;
        }
        rewritten.push_str(line);
        rewritten.push_str("\r\n");
    }
    if !has_host {
        rewritten.push_str("Host: ");
        rewritten.push_str(&format_authority(host, port, 80));
        rewritten.push_str("\r\n");
    }
    rewritten.push_str("Connection: close\r\n\r\n");
    Ok(rewritten.into_bytes())
}

/// 解析 host[:port] 或 [IPv6][:port]
fn parse_authority(value: &str, default_port: u16) -> Result<(String, u16)> {
    let value = value.trim();
    if value.is_empty() {
        return Err(anyhow!("HTTP 代理目标地址为空"));
    }
    if let Some(rest) = value.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| anyhow!("HTTP 代理 IPv6 地址格式不正确"))?;
        let host = &rest[..end];
        if host.is_empty() {
            return Err(anyhow!("HTTP 代理目标主机为空"));
        }
        let after = &rest[end + 1..];
        let port = if after.is_empty() {
            default_port
        } else {
            let port_text = after
                .strip_prefix(':')
                .ok_or_else(|| anyhow!("HTTP 代理目标端口格式不正确"))?;
            parse_port(port_text)?
        };
        return Ok((host.to_string(), port));
    }
    if let Some((host, port_text)) = value.rsplit_once(':') {
        if !host.contains(':') {
            if host.is_empty() {
                return Err(anyhow!("HTTP 代理目标主机为空"));
            }
            return Ok((host.to_string(), parse_port(port_text)?));
        }
    }
    Ok((value.to_string(), default_port))
}

/// 解析并校验代理目标端口
fn parse_port(value: &str) -> Result<u16> {
    let port = value
        .parse::<u16>()
        .map_err(|_| anyhow!("HTTP 代理目标端口不正确"))?;
    if port == 0 {
        return Err(anyhow!("HTTP 代理目标端口必须在 1 到 65535 之间"));
    }
    Ok(port)
}

/// 格式化 Host 头中的 authority
fn format_authority(host: &str, port: u16, default_port: u16) -> String {
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{}]", host)
    } else {
        host.to_string()
    };
    if port == default_port {
        host
    } else {
        format!("{}:{}", host, port)
    }
}

/// 写入 HTTP 代理错误响应
async fn write_http_error(stream: &mut TcpStream, code: u16, reason: &str) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {} {}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
        code, reason
    );
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

/// 在本地 TCP 连接与 SSH channel 之间双向转发数据
async fn bridge_tcp_and_channel(
    stream: TcpStream,
    channel: russh::Channel<client::Msg>,
    cancellation: watch::Receiver<bool>,
) -> Result<()> {
    let channel_stream = channel.into_stream();
    bridge_tcp_and_channel_stream(stream, channel_stream, cancellation).await
}

/// 在本地 TCP 连接与已打开的 SSH channel stream 之间双向转发数据
async fn bridge_tcp_and_channel_stream<C>(
    mut stream: TcpStream,
    mut channel_stream: C,
    mut cancellation: watch::Receiver<bool>,
) -> Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin,
{
    tokio::select! {
        _ = wait_cancelled(&mut cancellation) => {}
        _ = copy_bidirectional(&mut stream, &mut channel_stream) => {}
    }
    let _ = stream.shutdown().await;
    let _ = channel_stream.shutdown().await;
    Ok(())
}
