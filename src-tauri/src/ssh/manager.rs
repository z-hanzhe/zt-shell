//! 会话管理器：集中管理所有活动 SSH 会话、终端通道与 SFTP 会话

use std::sync::Arc;

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use russh_sftp::client::SftpSession;
use tauri::ipc::{Channel, Response};
use tauri::AppHandle;
use tokio::sync::{mpsc::UnboundedSender, Mutex};

use super::session::{SshSession, TerminalCommand};
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

/// 全局会话管理器，作为 Tauri 托管状态
#[derive(Default)]
pub struct SessionManager {
    sessions: DashMap<String, Arc<SessionEntry>>,
}

impl SessionManager {
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
        if let Some((_, entry)) = self.sessions.remove(session_id) {
            if let Ok(mut guard) = entry.terminal_tx.try_lock() {
                if let Some(tx) = guard.take() {
                    let _ = tx.send(TerminalCommand::Close);
                }
            }
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
        let tx = entry
            .session
            .open_terminal(app, session_id.to_string(), cols, rows, on_data)
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
