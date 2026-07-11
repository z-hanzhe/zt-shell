//! Tauri 命令层：将前端调用映射到 SSH 内核能力

use tauri::{AppHandle, State};

use crate::ssh::manager::SessionManager;
use crate::ssh::monitor::{self, MonitorData};
use crate::ssh::sftp;
use crate::ssh::types::{ConnectionConfig, FileEntry};

/// 统一将内部错误转为字符串返回给前端
type CmdResult<T> = Result<T, String>;

fn map_err<T>(r: anyhow::Result<T>) -> CmdResult<T> {
    r.map_err(|e| e.to_string())
}

/// 建立 SSH 连接，返回会话标识
#[tauri::command]
pub async fn ssh_connect(
    manager: State<'_, SessionManager>,
    config: ConnectionConfig,
) -> CmdResult<String> {
    map_err(manager.connect(&config).await)
}

/// 断开并释放会话
#[tauri::command]
pub async fn ssh_disconnect(manager: State<'_, SessionManager>, session_id: String) -> CmdResult<()> {
    manager.disconnect(&session_id);
    Ok(())
}

/// 为会话开启交互式终端
#[tauri::command]
pub async fn terminal_open(
    app: AppHandle,
    manager: State<'_, SessionManager>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> CmdResult<()> {
    map_err(manager.open_terminal(app, &session_id, cols, rows).await)
}

/// 向终端写入用户输入（字节序列）
#[tauri::command]
pub async fn terminal_write(
    manager: State<'_, SessionManager>,
    session_id: String,
    data: Vec<u8>,
) -> CmdResult<()> {
    map_err(manager.write_terminal(&session_id, data).await)
}

/// 变更终端窗口尺寸
#[tauri::command]
pub async fn terminal_resize(
    manager: State<'_, SessionManager>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> CmdResult<()> {
    map_err(manager.resize_terminal(&session_id, cols, rows).await)
}

/// 采集一次远程监控数据
#[tauri::command]
pub async fn monitor_collect(
    manager: State<'_, SessionManager>,
    session_id: String,
) -> CmdResult<MonitorData> {
    map_err(monitor::collect(&manager, &session_id).await)
}

/// 列举远端目录内容
#[tauri::command]
pub async fn sftp_list(
    manager: State<'_, SessionManager>,
    session_id: String,
    path: String,
) -> CmdResult<Vec<FileEntry>> {
    let sftp = map_err(manager.sftp(&session_id).await)?;
    map_err(sftp::list_dir(&sftp, &path).await)
}

/// 获取远端主目录绝对路径
#[tauri::command]
pub async fn sftp_home(
    manager: State<'_, SessionManager>,
    session_id: String,
) -> CmdResult<String> {
    let sftp = map_err(manager.sftp(&session_id).await)?;
    map_err(sftp::canonicalize(&sftp, ".").await)
}

/// 读取远端文件内容
#[tauri::command]
pub async fn sftp_read(
    manager: State<'_, SessionManager>,
    session_id: String,
    path: String,
) -> CmdResult<Vec<u8>> {
    let sftp = map_err(manager.sftp(&session_id).await)?;
    map_err(sftp::read_file(&sftp, &path).await)
}

/// 写入远端文件内容
#[tauri::command]
pub async fn sftp_write(
    manager: State<'_, SessionManager>,
    session_id: String,
    path: String,
    data: Vec<u8>,
) -> CmdResult<()> {
    let sftp = map_err(manager.sftp(&session_id).await)?;
    map_err(sftp::write_file(&sftp, &path, &data).await)
}

/// 删除远端文件
#[tauri::command]
pub async fn sftp_remove_file(
    manager: State<'_, SessionManager>,
    session_id: String,
    path: String,
) -> CmdResult<()> {
    let sftp = map_err(manager.sftp(&session_id).await)?;
    map_err(sftp::remove_file(&sftp, &path).await)
}

/// 删除远端目录
#[tauri::command]
pub async fn sftp_remove_dir(
    manager: State<'_, SessionManager>,
    session_id: String,
    path: String,
) -> CmdResult<()> {
    let sftp = map_err(manager.sftp(&session_id).await)?;
    map_err(sftp::remove_dir(&sftp, &path).await)
}

/// 创建远端目录
#[tauri::command]
pub async fn sftp_create_dir(
    manager: State<'_, SessionManager>,
    session_id: String,
    path: String,
) -> CmdResult<()> {
    let sftp = map_err(manager.sftp(&session_id).await)?;
    map_err(sftp::create_dir(&sftp, &path).await)
}

/// 重命名（移动）远端文件或目录
#[tauri::command]
pub async fn sftp_rename(
    manager: State<'_, SessionManager>,
    session_id: String,
    from: String,
    to: String,
) -> CmdResult<()> {
    let sftp = map_err(manager.sftp(&session_id).await)?;
    map_err(sftp::rename(&sftp, &from, &to).await)
}

/// 上传本地文件到远端
#[tauri::command]
pub async fn sftp_upload(
    manager: State<'_, SessionManager>,
    session_id: String,
    local_path: String,
    remote_path: String,
) -> CmdResult<()> {
    let sftp = map_err(manager.sftp(&session_id).await)?;
    map_err(sftp::upload(&sftp, &local_path, &remote_path).await)
}

/// 下载远端文件到本地
#[tauri::command]
pub async fn sftp_download(
    manager: State<'_, SessionManager>,
    session_id: String,
    remote_path: String,
    local_path: String,
) -> CmdResult<()> {
    let sftp = map_err(manager.sftp(&session_id).await)?;
    map_err(sftp::download(&sftp, &remote_path, &local_path).await)
}
