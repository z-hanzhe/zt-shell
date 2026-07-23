//! Tauri 命令层：将前端调用映射到 SSH 内核能力

use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, State};

use crate::ssh::manager::SessionManager;
use crate::ssh::monitor::{self, MonitorData};
use crate::ssh::sftp::{self, RemoveEntryArg};
use crate::ssh::transfer::{
    self, RemoteItemArg, TransferCreateResult, TransferManager, TransferTaskDto,
};
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

/// 断开并释放会话，同时清理该会话的全部传输任务
#[tauri::command]
pub async fn ssh_disconnect(
    app: AppHandle,
    manager: State<'_, SessionManager>,
    transfers: State<'_, TransferManager>,
    session_id: String,
) -> CmdResult<()> {
    transfers.remove_session(&app, &session_id);
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
    on_data: Channel<Response>,
) -> CmdResult<()> {
    map_err(
        manager
            .open_terminal(app, &session_id, cols, rows, on_data)
            .await,
    )
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

/// 判断本地路径是否为目录（供终端拖拽上传前校验，仅允许单文件）
#[tauri::command]
pub async fn path_is_dir(path: String) -> CmdResult<bool> {
    Ok(std::path::Path::new(&path).is_dir())
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

/// 删除远端目录及其全部内容
///
/// 普通模式经 exec 执行 rm -rf 快速删除；sudo 提权模式下 exec 通道不具备提权能力，
/// 回落为 SFTP 递归删除以保持与文件管理权限一致
#[tauri::command]
pub async fn sftp_remove_dir(
    manager: State<'_, SessionManager>,
    session_id: String,
    path: String,
) -> CmdResult<()> {
    // 防御空路径与根目录，避免误删整个系统
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return Err("非法的删除路径".to_string());
    }
    if !map_err(manager.is_sudo(&session_id).await)? {
        let command = format!(
            "rm -rf -- {} && printf __ZTOK__ || printf __ZTFAIL__",
            transfer::shell_quote(trimmed)
        );
        let output = map_err(manager.exec(&session_id, &command).await)?;
        if !output.contains("__ZTOK__") {
            return Err("删除目录失败，请检查文件权限".to_string());
        }
        return Ok(());
    }
    let sftp = map_err(manager.sftp(&session_id).await)?;
    map_err(sftp::remove_dir_all(&sftp, trimmed).await)
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

/// 将选中的远端文件压缩为当前目录下的 zip 或 tar.gz 文件
#[tauri::command]
pub async fn sftp_create_archive(
    manager: State<'_, SessionManager>,
    session_id: String,
    directory: String,
    names: Vec<String>,
    archive_format: String,
    archive_name: String,
    operation_id: String,
) -> CmdResult<()> {
    let mut operation = map_err(manager.begin_operation(&session_id, &operation_id))?;
    map_err(
        sftp::create_archive(
            &manager,
            &session_id,
            &directory,
            &names,
            &archive_format,
            &archive_name,
            operation.cancellation(),
        )
        .await,
    )
}

/// 将远端压缩包解压到当前目录
#[tauri::command]
pub async fn sftp_extract_archive(
    manager: State<'_, SessionManager>,
    session_id: String,
    directory: String,
    archive_name: String,
    operation_id: String,
) -> CmdResult<()> {
    let mut operation = map_err(manager.begin_operation(&session_id, &operation_id))?;
    map_err(
        sftp::extract_archive(
            &manager,
            &session_id,
            &directory,
            &archive_name,
            operation.cancellation(),
        )
        .await,
    )
}

/// 批量删除远端条目，支持在递归处理过程中中断
#[tauri::command]
pub async fn sftp_remove_entries(
    manager: State<'_, SessionManager>,
    session_id: String,
    entries: Vec<RemoveEntryArg>,
    operation_id: String,
) -> CmdResult<()> {
    let mut operation = map_err(manager.begin_operation(&session_id, &operation_id))?;
    map_err(sftp::remove_entries(&manager, &session_id, &entries, operation.cancellation()).await)
}

/// 请求中断当前会话中的文件操作
#[tauri::command]
pub fn sftp_cancel_operation(
    manager: State<'_, SessionManager>,
    session_id: String,
    operation_id: String,
) -> CmdResult<bool> {
    map_err(manager.cancel_operation(&session_id, &operation_id))
}

/// 检测当前权限模式下对远端文件是否有写入权限
///
/// sudo 提权模式以 root 身份读写视为可写；普通模式经 exec 以登录用户执行 test -w 判断
#[tauri::command]
pub async fn sftp_check_writable(
    manager: State<'_, SessionManager>,
    session_id: String,
    path: String,
) -> CmdResult<bool> {
    if map_err(manager.is_sudo(&session_id).await)? {
        return Ok(true);
    }
    let command = format!(
        "test -w {} && printf __ZTOK__ || printf __ZTNO__",
        transfer::shell_quote(&path)
    );
    let output = map_err(manager.exec(&session_id, &command).await)?;
    Ok(output.contains("__ZTOK__"))
}

/// 切换会话的 sudo 提权文件管理开关
#[tauri::command]
pub async fn sftp_set_sudo(
    manager: State<'_, SessionManager>,
    session_id: String,
    enabled: bool,
) -> CmdResult<()> {
    map_err(manager.set_sudo(&session_id, enabled).await)
}

/// 创建上传任务：force 为 false 且文件总数超过阈值时不建任务，返回统计供前端确认
#[tauri::command]
pub async fn transfer_upload(
    app: AppHandle,
    transfers: State<'_, TransferManager>,
    session_id: String,
    local_paths: Vec<String>,
    remote_dir: String,
    force: bool,
    overwrite: bool,
) -> CmdResult<TransferCreateResult> {
    map_err(
        transfers
            .create_upload(&app, &session_id, local_paths, remote_dir, force, overwrite)
            .await,
    )
}

/// 创建下载任务：force 含义同上传
#[tauri::command]
pub async fn transfer_download(
    app: AppHandle,
    transfers: State<'_, TransferManager>,
    session_id: String,
    items: Vec<RemoteItemArg>,
    local_dir: String,
    force: bool,
    overwrite: bool,
) -> CmdResult<TransferCreateResult> {
    map_err(
        transfers
            .create_download(&app, &session_id, items, local_dir, force, overwrite)
            .await,
    )
}

/// 创建打包下载任务：远端 tar 打包后下载，仅 Linux 且要求远端存在 tar 命令
#[tauri::command]
pub async fn transfer_pack_download(
    app: AppHandle,
    transfers: State<'_, TransferManager>,
    session_id: String,
    remote_dir: String,
    names: Vec<String>,
    local_path: String,
) -> CmdResult<()> {
    map_err(
        transfers
            .create_pack_download(&app, &session_id, remote_dir, names, local_path)
            .await,
    )
}

/// 列出全部传输任务
#[tauri::command]
pub async fn transfer_list(transfers: State<'_, TransferManager>) -> CmdResult<Vec<TransferTaskDto>> {
    Ok(transfers.list())
}

/// 暂停传输任务，ids 为空表示全部
#[tauri::command]
pub async fn transfer_pause(
    app: AppHandle,
    transfers: State<'_, TransferManager>,
    ids: Option<Vec<String>>,
) -> CmdResult<()> {
    transfers.pause(&app, ids);
    Ok(())
}

/// 继续传输任务，ids 为空表示全部
#[tauri::command]
pub async fn transfer_resume(
    app: AppHandle,
    transfers: State<'_, TransferManager>,
    ids: Option<Vec<String>>,
) -> CmdResult<()> {
    transfers.resume(&app, ids);
    Ok(())
}

/// 删除传输任务（级联子任务），ids 为空表示全部
#[tauri::command]
pub async fn transfer_remove(
    app: AppHandle,
    transfers: State<'_, TransferManager>,
    ids: Option<Vec<String>>,
) -> CmdResult<()> {
    transfers.remove(&app, ids);
    Ok(())
}

/// 重试失败的传输任务，sessionId 为空表示全部会话
#[tauri::command]
pub async fn transfer_retry_failed(
    app: AppHandle,
    transfers: State<'_, TransferManager>,
    session_id: Option<String>,
) -> CmdResult<()> {
    transfers.retry_failed(&app, session_id.as_deref());
    Ok(())
}
