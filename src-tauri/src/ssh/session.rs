//! SSH 会话：建立连接、认证、终端通道读写与窗口变更

use std::sync::Arc;

use anyhow::{anyhow, Result};
use russh::client::{self, Handle};
use russh::keys::PrivateKeyWithHashAlg;
use russh::ChannelMsg;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use super::types::{AuthType, ConnectionConfig};

/// russh 客户端事件回调处理器。终端数据通过主动 wait 循环读取，此处仅需接受服务端公钥
struct ClientHandler;

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    /// 校验服务端公钥。当前实现信任所有服务端（后续可扩展 known_hosts 校验）
    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// 发送给终端通道任务的控制指令
pub enum TerminalCommand {
    /// 向远端写入用户输入
    Write(Vec<u8>),
    /// 终端窗口尺寸变更（列、行）
    Resize(u32, u32),
    /// 主动关闭终端通道
    Close,
}

/// 一个已建立的 SSH 会话，持有可复用的连接句柄
pub struct SshSession {
    /// russh 客户端句柄，可克隆用于开启多个通道（终端、SFTP、监控）
    handle: Handle<ClientHandler>,
}

impl SshSession {
    /// 建立到远端的 SSH 连接并完成认证
    pub async fn connect(config: &ConnectionConfig) -> Result<Self> {
        let ssh_config = Arc::new(client::Config::default());
        let addr = format!("{}:{}", config.host, config.port);

        let mut handle = client::connect(ssh_config, addr, ClientHandler)
            .await
            .map_err(|e| anyhow!("连接失败：{}", e))?;

        // 根据认证方式完成认证
        let authenticated = match config.auth_type {
            AuthType::Password => {
                let password = config
                    .password
                    .clone()
                    .ok_or_else(|| anyhow!("缺少密码"))?;
                handle
                    .authenticate_password(&config.username, password)
                    .await
                    .map_err(|e| anyhow!("密码认证异常：{}", e))?
            }
            AuthType::PrivateKey => {
                let key_path = config
                    .private_key_path
                    .clone()
                    .ok_or_else(|| anyhow!("缺少私钥路径"))?;
                let key_pair =
                    russh::keys::load_secret_key(&key_path, config.passphrase.as_deref())
                        .map_err(|e| anyhow!("加载私钥失败：{}", e))?;
                let hash_alg = handle.best_supported_rsa_hash().await?.flatten();
                handle
                    .authenticate_publickey(
                        &config.username,
                        PrivateKeyWithHashAlg::new(Arc::new(key_pair), hash_alg),
                    )
                    .await
                    .map_err(|e| anyhow!("私钥认证异常：{}", e))?
            }
        };

        if !authenticated.success() {
            return Err(anyhow!("认证失败，请检查用户名与凭据"));
        }

        Ok(Self { handle })
    }

    /// 开启一个交互式终端通道，返回用于向远端发送指令的通道
    ///
    /// 通道任务在独立异步任务内运行，读取远端输出并通过事件推送到前端，
    /// 事件名为 `terminal://data//{session_id}`，通道关闭时推送 `terminal://close//{session_id}`
    pub async fn open_terminal(
        &self,
        app: AppHandle,
        session_id: String,
        cols: u32,
        rows: u32,
    ) -> Result<mpsc::UnboundedSender<TerminalCommand>> {
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| anyhow!("打开终端通道失败：{}", e))?;

        // 申请伪终端并启动 shell
        channel
            .request_pty(false, "xterm-256color", cols, rows, 0, 0, &[])
            .await
            .map_err(|e| anyhow!("申请 PTY 失败：{}", e))?;
        channel
            .request_shell(true)
            .await
            .map_err(|e| anyhow!("启动 shell 失败：{}", e))?;

        let (tx, mut rx) = mpsc::unbounded_channel::<TerminalCommand>();
        let data_event = format!("terminal://data//{}", session_id);
        let close_event = format!("terminal://close//{}", session_id);

        // 单任务同时处理远端输出与本地控制指令，避免通道所有权冲突
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = channel.wait() => {
                        match msg {
                            Some(ChannelMsg::Data { data }) => {
                                let _ = app.emit(&data_event, data.to_vec());
                            }
                            Some(ChannelMsg::ExtendedData { data, .. }) => {
                                let _ = app.emit(&data_event, data.to_vec());
                            }
                            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                                break;
                            }
                            _ => {}
                        }
                    }
                    cmd = rx.recv() => {
                        match cmd {
                            Some(TerminalCommand::Write(bytes)) => {
                                if channel.data(&bytes[..]).await.is_err() {
                                    break;
                                }
                            }
                            Some(TerminalCommand::Resize(c, r)) => {
                                let _ = channel.window_change(c, r, 0, 0).await;
                            }
                            Some(TerminalCommand::Close) | None => {
                                let _ = channel.close().await;
                                break;
                            }
                        }
                    }
                }
            }
            let _ = app.emit(&close_event, ());
        });

        Ok(tx)
    }

    /// 在远端执行一条命令并返回标准输出（用于监控数据采集等一次性命令）
    pub async fn exec_command(&self, command: &str) -> Result<String> {
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| anyhow!("打开执行通道失败：{}", e))?;
        channel
            .exec(true, command.as_bytes())
            .await
            .map_err(|e| anyhow!("执行命令失败：{}", e))?;

        let mut output = Vec::new();
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => output.extend_from_slice(&data),
                ChannelMsg::ExtendedData { .. } => {}
                ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok(String::from_utf8_lossy(&output).into_owned())
    }

    /// 基于本会话开启 SFTP 子系统通道，返回底层通道
    pub async fn open_sftp_channel(&self) -> Result<russh::Channel<client::Msg>> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| anyhow!("打开 SFTP 通道失败：{}", e))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| anyhow!("请求 SFTP 子系统失败：{}", e))?;
        Ok(channel)
    }

    /// 以 sudo 提权方式开启 SFTP 通道，返回底层通道供上层建立 SFTP 会话
    ///
    /// 通过 `exec sudo -S` 在专用通道上以 root 身份启动 sftp-server：登录密码从 stdin 喂入，
    /// sudo 的密码提示与报错走 stderr（russh 的 into_stream 只读 stdout 故不污染二进制协议）。
    /// 命令内跨发行版探测 sftp-server 路径，握手用自定义提示符 `__ZTPW__` 与就绪哨兵 `__ZTOK__`
    pub async fn open_sudo_sftp_channel(&self, password: &str) -> Result<russh::Channel<client::Msg>> {
        // 自定义 sudo 密码提示符与握手哨兵，避免依赖随系统语言变化的默认提示文案
        const PROMPT: &str = "__ZTPW__";
        const READY: &str = "__ZTOK__";
        const MISSING: &str = "__ZTNO__";
        // 探测常见 sftp-server 路径后免密提示启动，printf 就绪哨兵再 exec 交接给 SFTP 协议
        let command = concat!(
            "sudo -S -p __ZTPW__ -- sh -c '",
            "for p in /usr/lib/openssh/sftp-server /usr/libexec/openssh/sftp-server ",
            "/usr/lib/ssh/sftp-server /usr/libexec/sftp-server /usr/lib/sftp-server; do ",
            "[ -x \"$p\" ] && P=\"$p\" && break; done; ",
            "[ -n \"$P\" ] || { echo __ZTNO__ >&2; exit 1; }; ",
            "printf __ZTOK__; exec \"$P\"'"
        );

        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| anyhow!("打开提权通道失败：{}", e))?;
        channel
            .exec(true, command.as_bytes())
            .await
            .map_err(|e| anyhow!("启动提权 SFTP 失败：{}", e))?;

        // 握手：喂密码并等待就绪哨兵。stdout 累积匹配 READY，stderr 提示符区分首次询问与密码错误
        let mut stdout = Vec::new();
        let mut password_sent = false;
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    stdout.extend_from_slice(&data);
                    if find_bytes(&stdout, READY.as_bytes()).is_some() {
                        return Ok(channel);
                    }
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    let text = String::from_utf8_lossy(&data);
                    if text.contains(MISSING) {
                        return Err(anyhow!("远端未找到 sftp-server，无法提权"));
                    }
                    if text.contains(PROMPT) {
                        if password_sent {
                            // 再次出现密码提示，说明上次密码错误
                            return Err(anyhow!("sudo 密码错误或该用户无 sudo 权限"));
                        }
                        channel
                            .data(format!("{}\n", password).as_bytes())
                            .await
                            .map_err(|e| anyhow!("发送提权密码失败：{}", e))?;
                        password_sent = true;
                    }
                }
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                    return Err(anyhow!("提权失败，请检查 sudo 权限与密码"));
                }
                _ => {}
            }
        }
    }
}

/// 在字节切片中查找子序列首次出现的位置
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
