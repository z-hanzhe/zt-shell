//! 会话管理器：集中管理所有活动 SSH 会话、终端通道与 SFTP 会话

use std::sync::Arc;

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use russh_sftp::client::SftpSession;
use tauri::AppHandle;
use tokio::sync::Mutex;

use super::session::{SshSession, TerminalCommand};
use super::types::ConnectionConfig;

/// 单个会话持有的运行时资源
struct SessionEntry {
    /// SSH 连接会话
    session: Arc<SshSession>,
    /// 终端通道控制发送端（打开终端后写入）
    terminal_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<TerminalCommand>>>,
    /// SFTP 会话（首次使用文件管理时惰性建立）
    sftp: Mutex<Option<Arc<SftpSession>>>,
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
            terminal_tx: Mutex::new(None),
            sftp: Mutex::new(None),
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
    ) -> Result<()> {
        let entry = self.entry(session_id)?;
        let tx = entry
            .session
            .open_terminal(app, session_id.to_string(), cols, rows)
            .await?;
        *entry.terminal_tx.lock().await = Some(tx);
        Ok(())
    }

    /// 向终端写入用户输入
    pub async fn write_terminal(&self, session_id: &str, data: Vec<u8>) -> Result<()> {
        let entry = self.entry(session_id)?;
        let guard = entry.terminal_tx.lock().await;
        if let Some(tx) = guard.as_ref() {
            tx.send(TerminalCommand::Write(data))
                .map_err(|_| anyhow!("终端已关闭"))?;
        }
        Ok(())
    }

    /// 变更终端窗口尺寸
    pub async fn resize_terminal(&self, session_id: &str, cols: u32, rows: u32) -> Result<()> {
        let entry = self.entry(session_id)?;
        let guard = entry.terminal_tx.lock().await;
        if let Some(tx) = guard.as_ref() {
            tx.send(TerminalCommand::Resize(cols, rows))
                .map_err(|_| anyhow!("终端已关闭"))?;
        }
        Ok(())
    }

    /// 在远端执行一次性命令并返回输出
    pub async fn exec(&self, session_id: &str, command: &str) -> Result<String> {
        let entry = self.entry(session_id)?;
        entry.session.exec_command(command).await
    }

    /// 获取或惰性创建会话的 SFTP 客户端
    pub async fn sftp(&self, session_id: &str) -> Result<Arc<SftpSession>> {
        let entry = self.entry(session_id)?;
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
}
