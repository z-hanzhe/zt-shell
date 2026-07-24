//! SSH 代理传输层：建立 SOCKS 或 HTTP CONNECT 隧道并返回异步 TCP 流

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};

use anyhow::{anyhow, Result};
use async_http_proxy::{http_connect_tokio, http_connect_tokio_with_basic_auth};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{lookup_host, TcpStream};
use tokio_socks::tcp::{Socks4Stream, Socks5Stream};

use super::types::{ProxyConfig, ProxyType};

/// 可交给 russh 的异步传输流
pub trait AsyncTransport: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> AsyncTransport for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

/// 装箱后的异步传输流，用于统一不同代理协议的具体流类型
pub type BoxedTransport = Box<dyn AsyncTransport>;

/// 连接代理服务器并启用 TCP_NODELAY
async fn connect_proxy_server(proxy: &ProxyConfig) -> Result<TcpStream> {
    if proxy.host.trim().is_empty() {
        return Err(anyhow!("代理服务器地址不能为空"));
    }
    let stream = TcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .map_err(|error| anyhow!("连接代理服务器 [ {} ] 失败：{}", proxy.name, error))?;
    stream
        .set_nodelay(true)
        .map_err(|error| anyhow!("配置代理连接 [ {} ] 失败：{}", proxy.name, error))?;
    Ok(stream)
}

/// 将 SOCKS4 目标解析为 IPv4，SOCKS4 不支持远端域名解析和 IPv6
async fn resolve_socks4_target(host: &str, port: u16) -> Result<SocketAddrV4> {
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return Ok(SocketAddrV4::new(ip, port));
    }
    if host.parse::<IpAddr>().is_ok() {
        return Err(anyhow!("SOCKS4 不支持 IPv6 目标地址"));
    }
    lookup_host((host, port))
        .await
        .map_err(|error| anyhow!("SOCKS4 解析目标地址失败：{}", error))?
        .find_map(|address| match address {
            SocketAddr::V4(address) => Some(address),
            SocketAddr::V6(_) => None,
        })
        .ok_or_else(|| anyhow!("SOCKS4 未找到可用的目标 IPv4 地址"))
}

/// 读取并校验需要成对出现的代理用户名与密码
fn proxy_credentials(proxy: &ProxyConfig) -> Result<Option<(&str, &str)>> {
    let username = proxy
        .username
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let password = proxy.password.as_deref().filter(|value| !value.is_empty());
    match (username, password) {
        (Some(username), Some(password)) => Ok(Some((username, password))),
        (None, None) => Ok(None),
        _ => Err(anyhow!("代理用户名和密码必须同时填写或同时留空")),
    }
}

/// 通过指定代理建立到 SSH 目标的 TCP 隧道
pub async fn connect_through_proxy(
    proxy: &ProxyConfig,
    target_host: &str,
    target_port: u16,
) -> Result<BoxedTransport> {
    let socket = connect_proxy_server(proxy).await?;
    match proxy.proxy_type {
        ProxyType::Socks4 => {
            let target = resolve_socks4_target(target_host, target_port).await?;
            let user_id = proxy
                .username
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let stream = if let Some(user_id) = user_id {
                Socks4Stream::connect_with_userid_and_socket(socket, target, user_id).await
            } else {
                Socks4Stream::connect_with_socket(socket, target).await
            }
            .map_err(|error| anyhow!("SOCKS4 代理 [ {} ] 建立隧道失败：{}", proxy.name, error))?;
            Ok(Box::new(stream))
        }
        ProxyType::Socks4a => {
            let target = (target_host, target_port);
            let user_id = proxy
                .username
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let stream = if let Some(user_id) = user_id {
                Socks4Stream::connect_with_userid_and_socket(socket, target, user_id).await
            } else {
                Socks4Stream::connect_with_socket(socket, target).await
            }
            .map_err(|error| anyhow!("SOCKS4A 代理 [ {} ] 建立隧道失败：{}", proxy.name, error))?;
            Ok(Box::new(stream))
        }
        ProxyType::Socks5 => {
            let target = (target_host, target_port);
            let stream = if let Some((username, password)) = proxy_credentials(proxy)? {
                Socks5Stream::connect_with_password_and_socket(socket, target, username, password)
                    .await
            } else {
                Socks5Stream::connect_with_socket(socket, target).await
            }
            .map_err(|error| anyhow!("SOCKS5 代理 [ {} ] 建立隧道失败：{}", proxy.name, error))?;
            Ok(Box::new(stream))
        }
        ProxyType::Http => {
            let mut stream = socket;
            let http_target_host = match target_host.parse::<IpAddr>() {
                Ok(IpAddr::V6(_)) => format!("[{}]", target_host),
                _ => target_host.to_string(),
            };
            if let Some((username, password)) = proxy_credentials(proxy)? {
                http_connect_tokio_with_basic_auth(
                    &mut stream,
                    &http_target_host,
                    target_port,
                    username,
                    password,
                )
                .await
            } else {
                http_connect_tokio(&mut stream, &http_target_host, target_port).await
            }
            .map_err(|error| {
                anyhow!(
                    "HTTP 代理 [ {} ] 建立 CONNECT 隧道失败：{}",
                    proxy.name,
                    error
                )
            })?;
            Ok(Box::new(stream))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// 创建测试代理配置
    fn test_config(proxy_type: ProxyType, port: u16) -> ProxyConfig {
        ProxyConfig {
            id: "test-proxy".to_string(),
            name: "测试代理".to_string(),
            proxy_type,
            host: "127.0.0.1".to_string(),
            port,
            username: None,
            password: None,
        }
    }

    /// 启动测试 SOCKS4 服务端并读取用户标识与可选域名
    async fn read_socks4_request(
        socket: &mut TcpStream,
        expect_domain: bool,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let mut request = vec![0; 8];
        socket.read_exact(&mut request).await?;
        loop {
            let byte = socket.read_u8().await?;
            request.push(byte);
            if byte == 0 {
                break;
            }
        }
        if expect_domain {
            loop {
                let byte = socket.read_u8().await?;
                request.push(byte);
                if byte == 0 {
                    break;
                }
            }
        }
        socket.write_all(&[0, 0x5a, 0, 22, 127, 0, 0, 1]).await?;
        Ok(request)
    }

    /// 测试 SOCKS4 使用 IPv4 目标与用户标识
    #[tokio::test]
    async fn connects_through_socks4() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let request = read_socks4_request(&mut socket, false)
                .await
                .map_err(|error| anyhow!(error.to_string()))?;
            if request[0] != 4
                || request[1] != 1
                || request[2..4] != [0, 22]
                || request[4..8] != [127, 0, 0, 1]
                || request[8..] != [b't', b'e', b's', b't', 0]
            {
                return Err(anyhow!("SOCKS4 请求内容不正确"));
            }
            Ok::<(), anyhow::Error>(())
        });
        let mut proxy = test_config(ProxyType::Socks4, port);
        proxy.username = Some("test".to_string());
        let _stream = connect_through_proxy(&proxy, "127.0.0.1", 22).await?;
        server.await??;
        Ok(())
    }

    /// 测试 SOCKS4A 将域名原样交给代理解析
    #[tokio::test]
    async fn connects_through_socks4a() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let request = read_socks4_request(&mut socket, true)
                .await
                .map_err(|error| anyhow!(error.to_string()))?;
            if request[0] != 4
                || request[1] != 1
                || request[4..8] != [0, 0, 0, 1]
                || request[8..]
                    != [
                        0, b's', b's', b'h', b'.', b'i', b'n', b't', b'e', b'r', b'n', b'a', b'l',
                        0,
                    ]
            {
                return Err(anyhow!("SOCKS4A 请求内容不正确"));
            }
            Ok::<(), anyhow::Error>(())
        });
        let proxy = test_config(ProxyType::Socks4a, port);
        let _stream = connect_through_proxy(&proxy, "ssh.internal", 22).await?;
        server.await??;
        Ok(())
    }

    /// 测试 SOCKS5 用户名密码认证和域名目标
    #[tokio::test]
    async fn connects_through_socks5_with_password() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut methods = [0; 2];
            socket.read_exact(&mut methods).await?;
            let mut offered = vec![0; methods[1] as usize];
            socket.read_exact(&mut offered).await?;
            if methods != [5, 2] || offered != [0, 2] {
                return Err(anyhow!("SOCKS5 认证方法请求不正确"));
            }
            socket.write_all(&[5, 2]).await?;

            let mut auth = [0; 2];
            socket.read_exact(&mut auth).await?;
            let mut auth_data = vec![0; auth[1] as usize];
            socket.read_exact(&mut auth_data).await?;
            let password_len = socket.read_u8().await? as usize;
            let mut password = vec![0; password_len];
            socket.read_exact(&mut password).await?;
            if auth != [1, 4] || auth_data != b"user" || password != b"pass" {
                return Err(anyhow!("SOCKS5 用户名密码请求不正确"));
            }
            socket.write_all(&[1, 0]).await?;

            let mut request = [0; 4];
            socket.read_exact(&mut request).await?;
            let domain_len = socket.read_u8().await? as usize;
            let mut domain = vec![0; domain_len];
            socket.read_exact(&mut domain).await?;
            let mut target_port = [0; 2];
            socket.read_exact(&mut target_port).await?;
            if request != [5, 1, 0, 3] || domain != b"ssh.internal" || target_port != [0, 22] {
                return Err(anyhow!("SOCKS5 目标请求不正确"));
            }
            socket.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 22]).await?;
            Ok::<(), anyhow::Error>(())
        });
        let mut proxy = test_config(ProxyType::Socks5, port);
        proxy.username = Some("user".to_string());
        proxy.password = Some("pass".to_string());
        let _stream = connect_through_proxy(&proxy, "ssh.internal", 22).await?;
        server.await??;
        Ok(())
    }

    /// 测试 HTTP CONNECT Basic 认证
    #[tokio::test]
    async fn connects_through_http_connect_with_basic_auth() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut request = Vec::new();
            loop {
                request.push(socket.read_u8().await?);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request)?;
            if !request.starts_with("CONNECT ssh.internal:22 HTTP/1.1\r\n")
                || !request.contains("Proxy-Authorization: Basic dXNlcjpwYXNz\r\n")
            {
                return Err(anyhow!("HTTP CONNECT 请求内容不正确"));
            }
            socket
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?;
            Ok::<(), anyhow::Error>(())
        });
        let mut proxy = test_config(ProxyType::Http, port);
        proxy.username = Some("user".to_string());
        proxy.password = Some("pass".to_string());
        let _stream = connect_through_proxy(&proxy, "ssh.internal", 22).await?;
        server.await??;
        Ok(())
    }
}
