//! 会话管理器：集中管理所有活动 SSH 会话、终端通道与 SFTP 会话

use std::sync::Arc;

use anyhow::{anyhow, Result};
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use russh_sftp::client::SftpSession;
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, Manager};
use tokio::sync::{mpsc::UnboundedSender, watch, Mutex};

use super::session::{SshSession, TerminalCommand};
use super::transfer::TransferManager;
use super::types::ConnectionConfig;

/// 单个会话持有的运行时资源
struct SessionEntry {
    /// SSH 连接会话
    session: Arc<SshSession>,
    /// 登录密码（用于 sudo 提权时复用，密码认证时才有）
    login_password: Option<String>,
    /// 终端通道控制发送端（打开终端后写入）
    terminal_tx: Mutex<Option<UnboundedSender<TerminalCommand>>>,
    /// 普通 SFTP 会话（首次使用文件管理时惰性建立）
    sftp: Mutex<Option<Arc<SftpSession>>>,
    /// 是否启用 sudo 提权文件管理
    sudo_active: Mutex<bool>,
    /// sudo 提权 SFTP 会话（启用提权时惰性建立）
    sudo_sftp: Mutex<Option<Arc<SftpSession>>>,
}

/// 单个可中断文件操作的控制信息
struct OperationControl {
    /// 操作所属会话，防止跨会话误中断
    session_id: String,
    /// 中断通知发送端
    cancel_tx: watch::Sender<bool>,
}

/// 可中断文件操作句柄，离开作用域时自动移除注册信息
pub struct OperationGuard<'a> {
    operations: &'a DashMap<String, OperationControl>,
    operation_id: String,
    cancel_rx: watch::Receiver<bool>,
}

impl OperationGuard<'_> {
    /// 获取供具体操作监听的中断通知接收端
    pub fn cancellation(&mut self) -> &mut watch::Receiver<bool> {
        &mut self.cancel_rx
    }
}

impl Drop for OperationGuard<'_> {
    fn drop(&mut self) {
        self.operations.remove(&self.operation_id);
    }
}

/// 全局会话管理器，作为 Tauri 托管状态
#[derive(Default)]
pub struct SessionManager {
    sessions: DashMap<String, Arc<SessionEntry>>,
    /// 正在运行且允许中断的文件操作
    operations: DashMap<String, OperationControl>,
}

impl SessionManager {
    /// 关闭已移除会话的终端控制通道，其余 SSH/SFTP 资源随条目引用释放
    fn close_entry(entry: Arc<SessionEntry>) {
        if let Ok(mut guard) = entry.terminal_tx.try_lock() {
            if let Some(tx) = guard.take() {
                let _ = tx.send(TerminalCommand::Close);
            }
        }
    }

    /// 建立新会话并纳入管理，返回会话标识
    pub async fn connect(&self, config: &ConnectionConfig) -> Result<String> {
        let session = SshSession::connect(config).await?;
        let entry = Arc::new(SessionEntry {
            session: Arc::new(session),
            login_password: config.password.clone(),
            terminal_tx: Mutex::new(None),
            sftp: Mutex::new(None),
            sudo_active: Mutex::new(false),
            sudo_sftp: Mutex::new(None),
        });
        self.sessions.insert(config.id.clone(), entry);
        Ok(config.id.clone())
    }

    /// 断开并移除会话，释放其终端与 SFTP 资源
    pub fn disconnect(&self, session_id: &str) {
        self.cancel_session_operations(session_id);
        if let Some((_, entry)) = self.sessions.remove(session_id) {
            Self::close_entry(entry);
        }
    }

    /**
     * 仅当 sessionId 仍指向指定会话条目时断开，避免旧终端关闭事件误删重连后的新会话
     */
    fn disconnect_if_current(&self, session_id: &str, expected: &Arc<SessionEntry>) -> bool {
        let removed = self
            .sessions
            .remove_if(session_id, |_, entry| Arc::ptr_eq(entry, expected));
        if let Some((_, entry)) = removed {
            self.cancel_session_operations(session_id);
            Self::close_entry(entry);
            true
        } else {
            false
        }
    }

    /// 获取会话条目，不存在时报错
    fn entry(&self, session_id: &str) -> Result<Arc<SessionEntry>> {
        self.sessions
            .get(session_id)
            .map(|e| e.clone())
            .ok_or_else(|| anyhow!("会话不存在或已断开：{}", session_id))
    }

    /// 为指定会话开启终端通道
    pub async fn open_terminal(
        &self,
        app: AppHandle,
        session_id: &str,
        cols: u32,
        rows: u32,
        on_data: Channel<Response>,
    ) -> Result<()> {
        let entry = self.entry(session_id)?;
        let close_app = app.clone();
        let close_session_id = session_id.to_string();
        let close_entry = entry.clone();
        let tx = entry
            .session
            .open_terminal(
                app,
                session_id.to_string(),
                cols,
                rows,
                on_data,
                move || {
                    let manager = close_app.state::<SessionManager>();
                    if manager.disconnect_if_current(&close_session_id, &close_entry) {
                        close_app
                            .state::<TransferManager>()
                            .remove_session(&close_app, &close_session_id);
                    }
                },
            )
            .await?;
        *entry.terminal_tx.lock().await = Some(tx);
        Ok(())
    }

    /// 向终端写入用户输入
    pub async fn write_terminal(&self, session_id: &str, data: Vec<u8>) -> Result<()> {
        let entry = self.entry(session_id)?;
        let guard = entry.terminal_tx.lock().await;
        let tx = guard.as_ref().ok_or_else(|| anyhow!("终端尚未打开"))?;
        tx.send(TerminalCommand::Write(data))
            .map_err(|_| anyhow!("终端已关闭"))?;
        Ok(())
    }

    /// 变更终端窗口尺寸
    pub async fn resize_terminal(&self, session_id: &str, cols: u32, rows: u32) -> Result<()> {
        let entry = self.entry(session_id)?;
        let guard = entry.terminal_tx.lock().await;
        let tx = guard.as_ref().ok_or_else(|| anyhow!("终端尚未打开"))?;
        tx.send(TerminalCommand::Resize(cols, rows))
            .map_err(|_| anyhow!("终端已关闭"))?;
        Ok(())
    }

    /// 在远端执行一次性命令并返回输出
    pub async fn exec(&self, session_id: &str, command: &str) -> Result<String> {
        let entry = self.entry(session_id)?;
        entry.session.exec_command(command).await
    }

    /// 在远端执行允许中断的一次性命令
    pub async fn exec_cancellable(
        &self,
        session_id: &str,
        command: &str,
        cancellation: &mut watch::Receiver<bool>,
    ) -> Result<String> {
        let entry = self.entry(session_id)?;
        entry
            .session
            .exec_command_cancellable(command, cancellation)
            .await
    }

    /// 登记一个可中断文件操作，操作结束时由返回的句柄自动清理
    pub fn begin_operation<'a>(
        &'a self,
        session_id: &str,
        operation_id: &str,
    ) -> Result<OperationGuard<'a>> {
        self.entry(session_id)?;
        if operation_id.trim().is_empty() {
            return Err(anyhow!("文件操作标识不能为空"));
        }
        let (cancel_tx, cancel_rx) = watch::channel(false);
        match self.operations.entry(operation_id.to_string()) {
            Entry::Occupied(_) => Err(anyhow!("文件操作标识已存在")),
            Entry::Vacant(entry) => {
                entry.insert(OperationControl {
                    session_id: session_id.to_string(),
                    cancel_tx,
                });
                Ok(OperationGuard {
                    operations: &self.operations,
                    operation_id: operation_id.to_string(),
                    cancel_rx,
                })
            }
        }
    }

    /// 请求中断指定会话中的文件操作，操作已经结束时返回 false
    pub fn cancel_operation(&self, session_id: &str, operation_id: &str) -> Result<bool> {
        let Some(operation) = self.operations.get(operation_id) else {
            return Ok(false);
        };
        if operation.session_id != session_id {
            return Err(anyhow!("文件操作不属于当前会话"));
        }
        operation
            .cancel_tx
            .send(true)
            .map_err(|_| anyhow!("文件操作已经结束"))?;
        Ok(true)
    }

    /// 请求中断会话中的全部文件操作
    fn cancel_session_operations(&self, session_id: &str) {
        for operation in self.operations.iter() {
            if operation.session_id == session_id {
                let _ = operation.cancel_tx.send(true);
            }
        }
    }

    /// 获取会话当前生效的 SFTP 客户端：按是否启用 sudo 返回提权或普通会话
    pub async fn sftp(&self, session_id: &str) -> Result<Arc<SftpSession>> {
        let entry = self.entry(session_id)?;
        if *entry.sudo_active.lock().await {
            Self::ensure_sudo_sftp(&entry).await
        } else {
            Self::ensure_normal_sftp(&entry).await
        }
    }

    /// 获取或惰性创建普通 SFTP 会话
    async fn ensure_normal_sftp(entry: &Arc<SessionEntry>) -> Result<Arc<SftpSession>> {
        let mut guard = entry.sftp.lock().await;
        if let Some(sftp) = guard.as_ref() {
            return Ok(sftp.clone());
        }
        let channel = entry.session.open_sftp_channel().await?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| anyhow!("初始化 SFTP 会话失败：{}", e))?;
        let sftp = Arc::new(sftp);
        *guard = Some(sftp.clone());
        Ok(sftp)
    }

    /// 获取或惰性创建 sudo 提权 SFTP 会话
    async fn ensure_sudo_sftp(entry: &Arc<SessionEntry>) -> Result<Arc<SftpSession>> {
        let mut guard = entry.sudo_sftp.lock().await;
        if let Some(sftp) = guard.as_ref() {
            return Ok(sftp.clone());
        }
        // 复用登录密码；私钥认证等无密码场景以空串尝试（仅 NOPASSWD 可成功）
        let password = entry.login_password.clone().unwrap_or_default();
        let channel = entry.session.open_sudo_sftp_channel(&password).await?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| anyhow!("初始化提权 SFTP 会话失败：{}", e))?;
        let sftp = Arc::new(sftp);
        *guard = Some(sftp.clone());
        Ok(sftp)
    }

    /// 切换会话的 sudo 提权文件管理开关
    ///
    /// 启用时立即建立提权 SFTP 会话，密码错误或无权限会在此报错并回滚开关；
    /// 关闭时清空提权会话缓存，后续文件操作回落普通权限
    pub async fn set_sudo(&self, session_id: &str, enabled: bool) -> Result<()> {
        let entry = self.entry(session_id)?;
        if enabled {
            // 先建立提权会话，成功后再置位，失败则保持普通模式
            Self::ensure_sudo_sftp(&entry).await?;
            *entry.sudo_active.lock().await = true;
        } else {
            *entry.sudo_active.lock().await = false;
            *entry.sudo_sftp.lock().await = None;
        }
        Ok(())
    }

    /// 查询会话当前是否处于 sudo 提权文件管理模式
    pub async fn is_sudo(&self, session_id: &str) -> Result<bool> {
        let entry = self.entry(session_id)?;
        let active = *entry.sudo_active.lock().await;
        Ok(active)
    }
}
