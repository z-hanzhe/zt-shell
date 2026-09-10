//! SSH 会话：建立连接、认证、终端通道读写、隧道通道与窗口变更

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use russh::client::{self, Handle};
use russh::keys::PrivateKeyWithHashAlg;
use russh::{ChannelMsg, Sig};
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, watch, RwLock};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::host_keys::{HostKeyStore, HostKeyVerification};
use super::proxy::connect_through_proxy;
use super::remote_command::CancellableCommandPlan;
use super::tunnel::{forward_remote_tunnel_channel, RemoteTunnelRegistry, RemoteTunnelTarget};
use super::types::{AuthType, ConnectionConfig, HostKeyApproval, HostKeyChallenge};

/// 可中断文件操作主动结束时返回的统一错误文案
pub const OPERATION_CANCELLED_MESSAGE: &str = "文件操作已中断";

/// SSH 转发通道创建的最长等待时间
const SSH_CHANNEL_OPEN_TIMEOUT: Duration = Duration::from_secs(15);
/// 发送远端进程控制请求的最长等待时间
const EXEC_CONTROL_TIMEOUT: Duration = Duration::from_secs(1);
/// 取消命令后等待远端进程正常退出的时间
const EXEC_TERMINATE_TIMEOUT: Duration = Duration::from_secs(2);
/// 强制终止命令后等待远端通道结束的时间
const EXEC_KILL_TIMEOUT: Duration = Duration::from_secs(2);
/// 通过独立通道终止远端命令进程组的最长等待时间
const REMOTE_PROCESS_CANCEL_TIMEOUT: Duration = Duration::from_secs(3);

/// russh 客户端事件回调处理器
pub(crate) struct ClientHandler {
    /// 远程传入隧道映射，用于处理服务端打开的 forwarded-tcpip 通道
    remote_tunnels: RemoteTunnelRegistry,
    /// 建连目标主机
    host: String,
    /// 建连目标端口
    port: u16,
    /// 应用私有的可信主机密钥存储
    host_keys: HostKeyStore,
    /// 用户对本次完整公钥的授权
    host_key_approval: Option<HostKeyApproval>,
}

/// 建连在认证前等待用户确认主机密钥
#[derive(Debug)]
struct HostKeyConfirmationError {
    challenge: HostKeyChallenge,
}

impl fmt::Display for HostKeyConfirmationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "需要确认服务器主机密钥")
    }
}

impl std::error::Error for HostKeyConfirmationError {}

/// SSH 传输层建连结果
pub enum SessionConnectOutcome {
    /// 主机密钥与用户认证均已通过
    Connected(SshSession),
    /// 已停止握手，等待用户确认主机密钥
    HostKeyConfirmationRequired(HostKeyChallenge),
}

impl client::Handler for ClientHandler {
    type Error = anyhow::Error;

    /// 校验服务端公钥，未知或变化的密钥必须先交由前端确认
    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        match self
            .host_keys
            .verify(
                &self.host,
                self.port,
                server_public_key,
                self.host_key_approval.as_ref(),
            )
            .await?
        {
            HostKeyVerification::Trusted => Ok(true),
            HostKeyVerification::ConfirmationRequired(challenge) => {
                Err(HostKeyConfirmationError { challenge }.into())
            }
        }
    }

    /// 处理远程传入隧道的新连接
    fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<client::Msg>,
        _connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        _session: &mut client::Session,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        let target = self.remote_tunnels.get(connected_port);
        async move {
            if let Some(target) = target {
                tokio::spawn(async move {
                    let _ = forward_remote_tunnel_channel(channel, target).await;
                });
            } else {
                let _ = channel.close().await;
            }
            Ok(())
        }
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
    /// russh 客户端句柄；普通通道可并发创建，远程监听申请独占修改
    handle: RwLock<Handle<ClientHandler>>,
    /// 远程传入隧道映射
    remote_tunnels: RemoteTunnelRegistry,
}

impl SshSession {
    /// 建立到远端的 SSH 连接并完成认证
    pub async fn connect(
        config: &ConnectionConfig,
        host_keys: HostKeyStore,
        host_key_approval: Option<&HostKeyApproval>,
    ) -> Result<SessionConnectOutcome> {
        let ssh_config = Arc::new(client::Config::default());
        let remote_tunnels = RemoteTunnelRegistry::default();
        let handler = ClientHandler {
            remote_tunnels: remote_tunnels.clone(),
            host: config.host.clone(),
            port: config.port,
            host_keys,
            host_key_approval: host_key_approval.cloned(),
        };
        let (connection_result, connection_error_prefix) = if let Some(proxy) = &config.proxy {
            let stream = connect_through_proxy(proxy, &config.host, config.port).await?;
            (
                client::connect_stream(ssh_config, stream, handler).await,
                "通过代理建立 SSH 连接失败",
            )
        } else {
            let addr = format!("{}:{}", config.host, config.port);
            (client::connect(ssh_config, addr, handler).await, "连接失败")
        };
        let mut handle = match connection_result {
            Ok(handle) => handle,
            Err(error) => {
                if let Some(confirmation) = error.downcast_ref::<HostKeyConfirmationError>() {
                    return Ok(SessionConnectOutcome::HostKeyConfirmationRequired(
                        confirmation.challenge.clone(),
                    ));
                }
                return Err(anyhow!("{}：{}", connection_error_prefix, error));
            }
        };

        // 根据认证方式完成认证
        let authenticated = match config.auth_type {
            AuthType::Password => {
                let password = config.password.clone().ok_or_else(|| anyhow!("缺少密码"))?;
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

        Ok(SessionConnectOutcome::Connected(Self {
            handle: RwLock::new(handle),
            remote_tunnels,
        }))
    }

    /// 打开 direct-tcpip 通道，用于本地拨出与动态 SOCKS 隧道
    pub async fn open_direct_tcpip(
        self: &Arc<Self>,
        target_host: &str,
        target_port: u16,
        originator_host: &str,
        originator_port: u16,
        mut cancellation: watch::Receiver<bool>,
    ) -> Result<russh::Channel<client::Msg>> {
        let session = Arc::clone(self);
        let request_target_host = target_host.to_string();
        let request_originator_host = originator_host.to_string();
        let mut request_task = tokio::spawn(async move {
            let handle = session.handle.read().await;
            handle
                .channel_open_direct_tcpip(
                    request_target_host,
                    u32::from(target_port),
                    request_originator_host,
                    u32::from(originator_port),
                )
                .await
        });
        tokio::select! {
            result = timeout(SSH_CHANNEL_OPEN_TIMEOUT, &mut request_task) => match result {
                Ok(Ok(Ok(channel))) => Ok(channel),
                Ok(Ok(Err(error))) => Err(anyhow!("打开隧道通道失败：{}", error)),
                Ok(Err(error)) => Err(anyhow!("打开隧道通道任务异常：{}", error)),
                Err(_) => {
                    // russh 的通道请求不可直接取消，稍后成功时主动关闭，避免遗留空闲通道
                    tokio::spawn(async move {
                        tokio::select! {
                            result = &mut request_task => {
                                if let Ok(Ok(channel)) = result {
                                    let _ = channel.close().await;
                                }
                            }
                            _ = wait_for_cancellation(&mut cancellation) => {
                                request_task.abort();
                            }
                        }
                    });
                    Err(anyhow!(
                        "打开隧道通道超时：{}:{}",
                        target_host,
                        target_port
                    ))
                }
            },
            _ = wait_for_cancellation(&mut cancellation) => {
                request_task.abort();
                Err(anyhow!("SSH 会话已断开"))
            }
        }
    }

    /// 请求服务器开启远程传入隧道
    pub async fn request_remote_forward(
        &self,
        bind_host: &str,
        bind_port: u16,
        target: RemoteTunnelTarget,
    ) -> Result<()> {
        self.remote_tunnels.insert(u32::from(bind_port), target);
        let mut handle = self.handle.write().await;
        match handle
            .tcpip_forward(bind_host.to_string(), u32::from(bind_port))
            .await
        {
            // 固定端口申请成功时 russh 返回 0，映射仍须保留配置的监听端口
            Ok(_) => Ok(()),
            Err(error) => {
                self.remote_tunnels.remove(u32::from(bind_port));
                Err(anyhow!("请求远程隧道失败：{}", error))
            }
        }
    }

    /// 开启一个交互式终端通道，返回用于向远端发送指令的通道
    ///
    /// 读写拆分为独立任务，输出通过有序二进制 IPC Channel 推送到前端；
    /// 写任务顺序处理输入、窗口尺寸变更和关闭指令，避免任一方向阻塞另一方向。
    pub async fn open_terminal<F>(
        &self,
        app: AppHandle,
        session_id: String,
        cols: u32,
        rows: u32,
        on_data: Channel<Response>,
        on_close: F,
    ) -> Result<mpsc::UnboundedSender<TerminalCommand>>
    where
        F: FnOnce() -> bool + Send + 'static,
    {
        let channel = self
            .handle
            .read()
            .await
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
        let close_event = format!("terminal://close//{}", session_id);
        let (mut read_half, write_half) = channel.split();

        // 持续消费远端输出；前端已释放 Channel 时停止无效推送
        tokio::spawn(async move {
            while let Some(msg) = read_half.wait().await {
                match msg {
                    ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }
                        if on_data.send(Response::new(data.to_vec())).is_err() =>
                    {
                        break;
                    }
                    ChannelMsg::Eof | ChannelMsg::Close => break,
                    _ => {}
                }
            }
            // 终端是会话核心，终端通道结束后先释放同代 SSH/SFTP 资源，再通知前端更新状态
            if on_close() {
                let _ = app.emit(&close_event, ());
            }
        });

        // 写任务：处理用户输入与控制指令
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    TerminalCommand::Write(data) => {
                        if write_half.data(&data[..]).await.is_err() {
                            break;
                        }
                    }
                    TerminalCommand::Resize(cols, rows) => {
                        let _ = write_half.window_change(cols, rows, 0, 0).await;
                    }
                    TerminalCommand::Close => break,
                }
            }
            // 指令通道关闭或收到关闭指令时关闭 SSH 通道，读任务随之收到 Close 退出
            let _ = write_half.close().await;
        });

        Ok(tx)
    }

    /// 在远端执行一条命令并返回标准输出（用于监控数据采集等一次性命令）
    pub async fn exec_command(self: &Arc<Self>, command: &str) -> Result<String> {
        self.exec_command_inner(command, None, None).await
    }

    /// 在远端执行一条允许中断的命令，中断时终止远端进程组并关闭当前执行通道
    pub async fn exec_command_cancellable(
        self: &Arc<Self>,
        command: &str,
        cancellation: &mut watch::Receiver<bool>,
    ) -> Result<String> {
        let plan = CancellableCommandPlan::new(command);
        let output = self
            .exec_command_inner(
                plan.wrapped_command(),
                Some(cancellation),
                Some(plan.cancel_command()),
            )
            .await?;
        if plan.is_control_failure(&output) {
            return Err(anyhow!("远端无法建立安全的进程组控制环境"));
        }
        Ok(output)
    }

    /// 执行一次性命令的公共实现，可选监听文件操作中断通知
    async fn exec_command_inner(
        self: &Arc<Self>,
        command: &str,
        mut cancellation: Option<&mut watch::Receiver<bool>>,
        remote_cancel_command: Option<&str>,
    ) -> Result<String> {
        if cancellation
            .as_ref()
            .is_some_and(|receiver| *receiver.borrow())
        {
            return Err(anyhow!(OPERATION_CANCELLED_MESSAGE));
        }

        let session = Arc::clone(self);
        let mut request_task = tokio::spawn(async move {
            let handle = session.handle.read().await;
            handle.channel_open_session().await
        });
        let open_result = if let Some(receiver) = cancellation.as_deref_mut() {
            tokio::select! {
                biased;
                _ = wait_for_cancellation(receiver) => {
                    close_late_exec_channel(request_task);
                    return Err(anyhow!(OPERATION_CANCELLED_MESSAGE));
                }
                result = timeout(SSH_CHANNEL_OPEN_TIMEOUT, &mut request_task) => result,
            }
        } else {
            timeout(SSH_CHANNEL_OPEN_TIMEOUT, &mut request_task).await
        };
        let mut channel = match open_result {
            Ok(Ok(Ok(channel))) => channel,
            Ok(Ok(Err(error))) => return Err(anyhow!("打开执行通道失败：{}", error)),
            Ok(Err(error)) => return Err(anyhow!("打开执行通道任务异常：{}", error)),
            Err(_) => {
                close_late_exec_channel(request_task);
                return Err(anyhow!("打开执行通道超时"));
            }
        };

        let exec_result = if let Some(receiver) = cancellation.as_deref_mut() {
            tokio::select! {
                biased;
                _ = wait_for_cancellation(receiver) => None,
                result = timeout(SSH_CHANNEL_OPEN_TIMEOUT, channel.exec(true, command.as_bytes())) => {
                    Some(result)
                }
            }
        } else {
            Some(
                timeout(
                    SSH_CHANNEL_OPEN_TIMEOUT,
                    channel.exec(true, command.as_bytes()),
                )
                .await,
            )
        };
        let Some(exec_result) = exec_result else {
            terminate_exec_channel(self, &mut channel, remote_cancel_command).await;
            return Err(anyhow!(OPERATION_CANCELLED_MESSAGE));
        };
        match exec_result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let _ = timeout(EXEC_CONTROL_TIMEOUT, channel.close()).await;
                return Err(anyhow!("执行命令失败：{}", error));
            }
            Err(_) => {
                // 请求可能已经到达服务端，按运行中命令执行完整终止流程。
                terminate_exec_channel(self, &mut channel, remote_cancel_command).await;
                return Err(anyhow!("执行命令请求超时"));
            }
        }

        let mut output = Vec::new();
        loop {
            let msg = if let Some(receiver) = cancellation.as_deref_mut() {
                tokio::select! {
                    biased;
                    _ = wait_for_cancellation(receiver) => {
                        terminate_exec_channel(self, &mut channel, remote_cancel_command).await;
                        return Err(anyhow!(OPERATION_CANCELLED_MESSAGE));
                    }
                    msg = channel.wait() => msg,
                }
            } else {
                channel.wait().await
            };
            let Some(msg) = msg else {
                break;
            };
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
            .read()
            .await
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
    pub async fn open_sudo_sftp_channel(
        &self,
        password: &str,
    ) -> Result<russh::Channel<client::Msg>> {
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
            .read()
            .await
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

/// 接管已超时或取消的通道打开请求，关闭稍后才成功返回的空闲通道。
fn close_late_exec_channel(
    request_task: JoinHandle<std::result::Result<russh::Channel<client::Msg>, russh::Error>>,
) {
    tokio::spawn(async move {
        if let Ok(Ok(channel)) = request_task.await {
            let _ = timeout(EXEC_CONTROL_TIMEOUT, channel.close()).await;
        }
    });
}

/// 终止远端 exec 进程组，并以有界等待关闭对应通道。
async fn terminate_exec_channel(
    session: &Arc<SshSession>,
    channel: &mut russh::Channel<client::Msg>,
    remote_cancel_command: Option<&str>,
) {
    if let Some(command) = remote_cancel_command {
        let _ = timeout(
            REMOTE_PROCESS_CANCEL_TIMEOUT,
            Box::pin(session.exec_command(command)),
        )
        .await;
    }
    let _ = timeout(EXEC_CONTROL_TIMEOUT, channel.signal(Sig::TERM)).await;
    if timeout(EXEC_TERMINATE_TIMEOUT, wait_for_exec_exit(channel))
        .await
        .is_err()
    {
        let _ = timeout(EXEC_CONTROL_TIMEOUT, channel.signal(Sig::KILL)).await;
        let _ = timeout(EXEC_KILL_TIMEOUT, wait_for_exec_exit(channel)).await;
    }
    let _ = timeout(EXEC_CONTROL_TIMEOUT, channel.close()).await;
}

/// 等待远端 exec 进程退出或通道关闭。
async fn wait_for_exec_exit(channel: &mut russh::Channel<client::Msg>) {
    while let Some(message) = channel.wait().await {
        if matches!(
            message,
            ChannelMsg::ExitStatus { .. } | ChannelMsg::ExitSignal { .. } | ChannelMsg::Close
        ) {
            return;
        }
    }
}

/// 等待收到取消通知
pub(crate) async fn wait_for_cancellation(cancellation: &mut watch::Receiver<bool>) {
    if *cancellation.borrow() {
        return;
    }
    while cancellation.changed().await.is_ok() {
        if *cancellation.borrow() {
            return;
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

#[cfg(test)]
mod tests {
    use std::env;

    use russh::keys::decode_secret_key;
    use russh::server::{self, Auth};
    use serde_json::{from_value, to_value};
    use tokio::net::TcpListener;
    use uuid::Uuid;

    use super::*;
    use crate::ssh::{manager::SessionManager, monitor, process, types::ConnectOutcome};

    /// 测试 SSH 服务使用的 Ed25519 私钥，口令为 blabla
    const TEST_SERVER_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jYmMAAAAGYmNyeXB0AAAAGAAAABDLGyfA39
J2FcJygtYqi5ISAAAAEAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIN+Wjn4+4Fcvl2Jl
KpggT+wCRxpSvtqqpVrQrKN1/A22AAAAkOHDLnYZvYS6H9Q3S3Nk4ri3R2jAZlQlBbUos5
FkHpYgNw65KCWCTXtP7ye2czMC3zjn2r98pJLobsLYQgRiHIv/CUdAdsqbvMPECB+wl/UQ
e+JpiSq66Z6GIt0801skPh20jxOO3F52SoX1IeO5D5PXfZrfSZlw6S8c7bwyp2FHxDewRx
7/wNsnDM0T7nLv/Q==
-----END OPENSSH PRIVATE KEY-----";

    /// 接受测试密码认证的本地 SSH 服务处理器
    struct TestServer;

    impl server::Handler for TestServer {
        type Error = russh::Error;

        /// 测试服务接受任意密码，验证客户端能进入认证阶段
        async fn auth_password(
            &mut self,
            _user: &str,
            _password: &str,
        ) -> Result<Auth, Self::Error> {
            Ok(Auth::Accept)
        }
    }

    /// 构造本地集成测试连接配置
    fn test_connection(port: u16) -> ConnectionConfig {
        ConnectionConfig {
            id: Uuid::new_v4().to_string(),
            name: "本地主机密钥测试".to_string(),
            host: "127.0.0.1".to_string(),
            port,
            username: "tester".to_string(),
            auth_type: AuthType::Password,
            password: Some("password".to_string()),
            has_password: false,
            private_key_path: None,
            passphrase: None,
            has_passphrase: false,
            proxy: None,
            remark: None,
            tunnels: Vec::new(),
            monitor_enabled: true,
            sftp_enabled: true,
        }
    }

    /// 功能开关缺失时兼容旧连接，显式关闭时保留关闭状态
    #[test]
    fn connection_feature_defaults() {
        let mut value = to_value(test_connection(22)).expect("连接应可序列化");
        let object = value.as_object_mut().expect("连接配置应为对象");
        object.remove("monitorEnabled");
        object.remove("sftpEnabled");
        let legacy: ConnectionConfig = from_value(value.clone()).expect("旧配置应可加载");
        assert!(legacy.monitor_enabled);
        assert!(legacy.sftp_enabled);
        value["monitorEnabled"] = false.into();
        value["sftpEnabled"] = false.into();
        let disabled: ConnectionConfig = from_value(value).expect("关闭功能的配置应可加载");
        assert!(!disabled.monitor_enabled);
        assert!(!disabled.sftp_enabled);
    }

    /// 关闭功能后 SSH 认证仍成功，但所有监控及普通、提权 SFTP 入口均被拒绝
    #[tokio::test]
    async fn disabled_features_do_not_open_remote_channels() {
        let key = decode_secret_key(TEST_SERVER_KEY, Some("blabla"))
            .expect("测试服务私钥应加载成功");
        let server_config = Arc::new(server::Config {
            keys: vec![key],
            ..Default::default()
        });
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("测试服务应监听成功");
        let port = listener.local_addr().expect("应取得监听地址").port();
        let server_task = tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.expect("测试连接应接入");
                let config = server_config.clone();
                tokio::spawn(async move {
                    let _ = server::run_stream(config, stream, TestServer).await;
                });
            }
        });
        let store = HostKeyStore::new(env::temp_dir().join(format!(
            "zt-shell-disabled-features-{}.json",
            Uuid::new_v4()
        )));
        let mut config = test_connection(port);
        config.monitor_enabled = false;
        config.sftp_enabled = false;
        let manager = SessionManager::default();
        let first = manager
            .connect(&config, &store, None)
            .await
            .expect("应收到主机密钥确认");
        let ConnectOutcome::HostKeyConfirmationRequired { challenge } = first else {
            panic!("测试连接必须先确认主机密钥");
        };
        let approval = HostKeyApproval {
            public_key: challenge.public_key,
            replace_existing: false,
            persist: false,
        };
        let outcome = manager
            .connect(&config, &store, Some(&approval))
            .await
            .expect("禁用附加功能不应影响 SSH 建连");
        assert!(matches!(outcome, ConnectOutcome::Connected { .. }));
        assert_eq!(
            manager.monitor_state(&config.id).err().unwrap().to_string(),
            "性能监控功能未开启"
        );
        assert_eq!(
            manager.sftp(&config.id).await.err().unwrap().to_string(),
            "SFTP 功能未开启"
        );
        assert_eq!(
            manager.set_sudo(&config.id, true).await.unwrap_err().to_string(),
            "SFTP 功能未开启"
        );
        assert!(!manager.is_sudo(&config.id).await.expect("应保持普通权限状态"));
        assert_eq!(
            monitor::collect(&manager, &config.id).await.err().unwrap().to_string(),
            "性能监控功能未开启"
        );
        assert_eq!(
            process::list(&manager, &config.id).await.err().unwrap().to_string(),
            "性能监控功能未开启"
        );
        manager.disconnect(&config.id);
        server_task.await.expect("测试服务任务应结束");
    }

    /// 真实 SSH 握手必须先返回确认，精确授权同一公钥后才能完成认证
    #[tokio::test]
    async fn requires_confirmation_before_authentication() {
        let mut server_config = server::Config::default();
        server_config.keys.push(
            russh::keys::decode_secret_key(TEST_SERVER_KEY, Some("blabla"))
                .expect("测试服务私钥应加载成功"),
        );
        let server_config = Arc::new(server_config);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("测试服务应监听成功");
        let port = listener
            .local_addr()
            .expect("测试服务应取得监听地址")
            .port();
        let server_task = tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.expect("测试连接应接入");
                let config = server_config.clone();
                tokio::spawn(async move {
                    let _ = server::run_stream(config, stream, TestServer).await;
                });
            }
        });

        let directory =
            std::env::temp_dir().join(format!("zt-shell-session-host-key-{}", Uuid::new_v4()));
        let store = HostKeyStore::new(directory.join("known_hosts.json"));
        let config = test_connection(port);
        let first = tokio::time::timeout(
            Duration::from_secs(5),
            SshSession::connect(&config, store.clone(), None),
        )
        .await
        .expect("首次握手不应超时")
        .expect("首次握手应返回确认结果");
        let SessionConnectOutcome::HostKeyConfirmationRequired(challenge) = first else {
            panic!("首次握手不得跳过主机密钥确认");
        };

        let approval = HostKeyApproval {
            public_key: challenge.public_key,
            replace_existing: false,
            persist: true,
        };
        let second = tokio::time::timeout(
            Duration::from_secs(5),
            SshSession::connect(&config, store, Some(&approval)),
        )
        .await
        .expect("授权后握手不应超时")
        .expect("授权后握手与认证应成功");
        assert!(matches!(second, SessionConnectOutcome::Connected(_)));

        server_task.await.expect("测试服务任务应正常结束");
        let _ = tokio::fs::remove_dir_all(directory).await;
    }
}
