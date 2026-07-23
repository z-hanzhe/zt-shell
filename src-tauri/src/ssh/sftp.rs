//! SFTP 文件操作：目录列举、读写、增删改等

use anyhow::{anyhow, Result};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileType;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use tokio::time::{timeout, Duration};
use uuid::Uuid;

use super::manager::SessionManager;
use super::session::OPERATION_CANCELLED_MESSAGE;
use super::transfer::shell_quote;
use super::types::FileEntry;

/// 批量删除命令中的单个远端条目
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveEntryArg {
    pub path: String,
    pub is_dir: bool,
}

/// 将 SFTP 错误格式化为简洁文案，避免状态码与消息重复（如 Failure: Failure）
pub fn format_sftp_error(e: &SftpError) -> String {
    if let SftpError::Status(status) = e {
        let message = status.error_message.trim();
        let code = status.status_code.to_string();
        if message.is_empty() {
            return code;
        }
        if message.eq_ignore_ascii_case(&code) {
            return message.to_string();
        }
        return format!("{}: {}", code, message);
    }
    e.to_string()
}

/// 将 SFTP 文件属性格式化为 Unix 风格权限字符串（如 drwxr-xr-x）
fn format_permissions(file_type: &FileType, mode: u32) -> String {
    let type_char = match file_type {
        FileType::Dir => 'd',
        FileType::Symlink => 'l',
        _ => '-',
    };
    // 依次解析属主、属组、其他的 rwx 位
    let mut s = String::with_capacity(10);
    s.push(type_char);
    let bits = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    for (mask, ch) in bits {
        if mode & mask != 0 {
            s.push(ch);
        } else {
            s.push('-');
        }
    }
    s
}

/// 将常见系统账号数字标识显示为名称，未知账号保留原始数字
fn format_owner_id(id: Option<u32>, root_name: &str) -> String {
    match id {
        Some(0) => root_name.to_string(),
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

/// 列举远端目录内容
pub async fn list_dir(sftp: &SftpSession, path: &str) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    let read_dir = sftp
        .read_dir(path)
        .await
        .map_err(|e| anyhow!("读取目录失败：{}", format_sftp_error(&e)))?;
    for item in read_dir {
        let meta = item.metadata();
        let file_type = meta.file_type();
        let permissions = meta.permissions.unwrap_or(0);
        entries.push(FileEntry {
            name: item.file_name(),
            is_dir: matches!(file_type, FileType::Dir),
            is_symlink: matches!(file_type, FileType::Symlink),
            size: meta.size.unwrap_or(0),
            permissions,
            permissions_str: format_permissions(&file_type, permissions),
            modified: meta.mtime.unwrap_or(0) as u64,
            owner: format_owner_id(meta.uid, "root"),
            group: format_owner_id(meta.gid, "root"),
        });
    }
    // 目录在前、文件在后，同类按名称排序
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(entries)
}

/// 读取远端文件全部内容
pub async fn read_file(sftp: &SftpSession, path: &str) -> Result<Vec<u8>> {
    sftp.read(path)
        .await
        .map_err(|e| anyhow!("读取文件失败：{}", format_sftp_error(&e)))
}

/// 将内容写入远端文件（覆盖）
pub async fn write_file(sftp: &SftpSession, path: &str, data: &[u8]) -> Result<()> {
    let mut file = sftp
        .create(path)
        .await
        .map_err(|e| anyhow!("创建文件失败：{}", format_sftp_error(&e)))?;
    file.write_all(data)
        .await
        .map_err(|e| anyhow!("写入文件失败：{}", e))?;
    file.flush()
        .await
        .map_err(|e| anyhow!("刷新文件失败：{}", e))
}

/// 删除远端文件
pub async fn remove_file(sftp: &SftpSession, path: &str) -> Result<()> {
    sftp.remove_file(path)
        .await
        .map_err(|e| anyhow!("删除文件失败：{}", format_sftp_error(&e)))
}

/// 删除远端空目录
pub async fn remove_dir(sftp: &SftpSession, path: &str) -> Result<()> {
    sftp.remove_dir(path)
        .await
        .map_err(|e| anyhow!("删除目录失败：{}", format_sftp_error(&e)))
}

/// 递归删除远端目录及其全部内容
///
/// 通过 SFTP 遍历删除以保持与当前权限模式（普通/sudo 提权）一致，
/// 先收集全部条目（父先于子）再逆序删除，确保先删文件与深层目录
pub async fn remove_dir_all(sftp: &SftpSession, path: &str) -> Result<()> {
    remove_dir_all_inner(sftp, path, None).await
}

/// 递归删除远端目录，并在每次 SFTP 请求之间响应中断
async fn remove_dir_all_cancellable(
    sftp: &SftpSession,
    path: &str,
    cancellation: &watch::Receiver<bool>,
) -> Result<()> {
    remove_dir_all_inner(sftp, path, Some(cancellation)).await
}

/// 递归删除的公共实现，中断不会回滚已经完成的删除
async fn remove_dir_all_inner(
    sftp: &SftpSession,
    path: &str,
    cancellation: Option<&watch::Receiver<bool>>,
) -> Result<()> {
    let mut all: Vec<(String, bool)> = vec![(path.to_string(), true)];
    let mut stack: Vec<String> = vec![path.to_string()];
    while let Some(dir) = stack.pop() {
        ensure_not_cancelled(cancellation)?;
        let read = sftp
            .read_dir(&dir)
            .await
            .map_err(|e| anyhow!("读取目录失败（{}）：{}", dir, format_sftp_error(&e)))?;
        ensure_not_cancelled(cancellation)?;
        for item in read {
            let child = format!("{}/{}", dir.trim_end_matches('/'), item.file_name());
            // 符号链接按文件删除（unlink），不深入目标避免误删与循环
            let is_dir = matches!(item.metadata().file_type(), FileType::Dir);
            all.push((child.clone(), is_dir));
            if is_dir {
                stack.push(child);
            }
        }
    }
    for (entry_path, is_dir) in all.iter().rev() {
        ensure_not_cancelled(cancellation)?;
        if *is_dir {
            remove_dir(sftp, entry_path).await?;
        } else {
            remove_file(sftp, entry_path).await?;
        }
    }
    ensure_not_cancelled(cancellation)?;
    Ok(())
}

/// 批量删除远端条目，普通目录使用独立 exec 通道，sudo 目录使用共享 SFTP 递归
pub async fn remove_entries(
    manager: &SessionManager,
    session_id: &str,
    entries: &[RemoveEntryArg],
    cancellation: &mut watch::Receiver<bool>,
) -> Result<()> {
    if entries.is_empty() {
        return Err(anyhow!("未选择需要删除的文件"));
    }
    for entry in entries {
        validate_removal_path(&entry.path)?;
    }

    let sudo = manager.is_sudo(session_id).await?;
    let needs_sftp = sudo || entries.iter().any(|entry| !entry.is_dir);
    let sftp = if needs_sftp {
        ensure_not_cancelled(Some(cancellation))?;
        Some(manager.sftp(session_id).await?)
    } else {
        None
    };

    for entry in entries {
        ensure_not_cancelled(Some(cancellation))?;
        if entry.is_dir && !sudo {
            let command = format!(
                "rm -rf -- {} && printf __ZTOK__ || printf __ZTFAIL__",
                shell_quote(&entry.path)
            );
            let output = manager
                .exec_cancellable(session_id, &command, cancellation)
                .await?;
            if !output.contains("__ZTOK__") {
                return Err(anyhow!("删除目录失败，请检查文件权限"));
            }
        } else if entry.is_dir {
            remove_dir_all_cancellable(
                sftp.as_deref().ok_or_else(|| anyhow!("SFTP 会话未建立"))?,
                &entry.path,
                cancellation,
            )
            .await?;
        } else {
            remove_file(
                sftp.as_deref().ok_or_else(|| anyhow!("SFTP 会话未建立"))?,
                &entry.path,
            )
            .await?;
        }
    }
    ensure_not_cancelled(Some(cancellation))
}

/// 创建远端目录
pub async fn create_dir(sftp: &SftpSession, path: &str) -> Result<()> {
    sftp.create_dir(path)
        .await
        .map_err(|e| anyhow!("创建目录失败：{}", format_sftp_error(&e)))
}

/// 重命名（移动）远端文件或目录
pub async fn rename(sftp: &SftpSession, from: &str, to: &str) -> Result<()> {
    sftp.rename(from, to)
        .await
        .map_err(|e| anyhow!("重命名失败：{}", format_sftp_error(&e)))
}

/// 获取远端用户主目录的绝对路径
pub async fn canonicalize(sftp: &SftpSession, path: &str) -> Result<String> {
    sftp.canonicalize(path)
        .await
        .map_err(|e| anyhow!("解析路径失败：{}", format_sftp_error(&e)))
}

/// 上传本地文件到远端
pub async fn upload(sftp: &SftpSession, local_path: &str, remote_path: &str) -> Result<()> {
    let data = tokio::fs::read(local_path)
        .await
        .map_err(|e| anyhow!("读取本地文件失败：{}", e))?;
    write_file(sftp, remote_path, &data).await
}

/// 下载远端文件到本地
pub async fn download(sftp: &SftpSession, remote_path: &str, local_path: &str) -> Result<()> {
    let data = read_file(sftp, remote_path).await?;
    tokio::fs::write(local_path, &data)
        .await
        .map_err(|e| anyhow!("写入本地文件失败：{}", e))
}

/// 在远端当前目录创建 zip 或 tar.gz 压缩包
pub async fn create_archive(
    manager: &SessionManager,
    session_id: &str,
    directory: &str,
    names: &[String],
    archive_format: &str,
    archive_name: &str,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<()> {
    ensure_normal_exec_mode(manager, session_id).await?;
    validate_directory(directory)?;
    validate_entry_name(archive_name, "压缩包名称")?;
    if names.is_empty() {
        return Err(anyhow!("未选择需要压缩的文件"));
    }
    for name in names {
        validate_entry_name(name, "文件名")?;
    }
    if names.iter().any(|name| name == archive_name) {
        return Err(anyhow!("压缩包名称不能与选中的文件同名"));
    }

    let (tool, suffix) = match archive_format {
        "zip" => ("zip", ".zip"),
        "tarGz" => ("tar", ".tar.gz"),
        _ => return Err(anyhow!("不支持的压缩格式")),
    };
    if !archive_name.to_ascii_lowercase().ends_with(suffix) {
        return Err(anyhow!("压缩包扩展名与压缩格式不匹配"));
    }
    ensure_remote_tool(manager, session_id, tool, cancellation).await?;

    let temp_name = format!("./.ztshell-{}{}", Uuid::new_v4(), suffix);
    let target_name = format!("./{}", archive_name);
    let archive_command = build_archive_command(&temp_name, names, archive_format);
    let command = format!(
        "cd {} && test ! -d {} && rm -f -- {} && {} && mv -f -- {} {} && printf __ZTOK__ || {{ rm -f -- {}; printf __ZTFAIL__; }}",
        shell_quote(directory),
        shell_quote(&target_name),
        shell_quote(&temp_name),
        archive_command,
        shell_quote(&temp_name),
        shell_quote(&target_name),
        shell_quote(&temp_name)
    );
    let output = match manager
        .exec_cancellable(session_id, &command, cancellation)
        .await
    {
        Ok(output) => output,
        Err(error) => {
            cleanup_archive_temp(manager, session_id, directory, &temp_name).await;
            return Err(error);
        }
    };
    if !output.contains("__ZTOK__") {
        cleanup_archive_temp(manager, session_id, directory, &temp_name).await;
        return Err(anyhow!("远端压缩失败，请检查文件权限和剩余空间"));
    }
    Ok(())
}

/// 构建远端压缩命令，确保 tar 包内条目不携带当前目录前缀
fn build_archive_command(temp_name: &str, names: &[String], archive_format: &str) -> String {
    let source_args = names
        .iter()
        .map(|name| {
            if archive_format == "tarGz" {
                shell_quote(name)
            } else {
                shell_quote(&format!("./{}", name))
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if archive_format == "zip" {
        format!(
            "zip -rq {} {} >/dev/null 2>&1",
            shell_quote(temp_name),
            source_args
        )
    } else {
        format!(
            "tar -czf {} -- {} >/dev/null 2>&1",
            shell_quote(temp_name),
            source_args
        )
    }
}

/// 将远端 zip 或 tar.gz 压缩包解压到当前目录
pub async fn extract_archive(
    manager: &SessionManager,
    session_id: &str,
    directory: &str,
    archive_name: &str,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<()> {
    ensure_normal_exec_mode(manager, session_id).await?;
    validate_directory(directory)?;
    validate_entry_name(archive_name, "压缩包名称")?;

    let lower_name = archive_name.to_ascii_lowercase();
    let (tool, extract_command) = if lower_name.ends_with(".zip") {
        (
            "unzip",
            format!(
                "unzip -oq {} -d . >/dev/null 2>&1",
                shell_quote(&format!("./{}", archive_name))
            ),
        )
    } else if lower_name.ends_with(".tar.gz") || lower_name.ends_with(".tgz") {
        (
            "tar",
            format!(
                "tar -xzf {} >/dev/null 2>&1",
                shell_quote(&format!("./{}", archive_name))
            ),
        )
    } else {
        return Err(anyhow!("仅支持解压 zip、tar.gz 或 tgz 文件"));
    };
    ensure_remote_tool(manager, session_id, tool, cancellation).await?;

    let command = format!(
        "cd {} && {} && printf __ZTOK__ || printf __ZTFAIL__",
        shell_quote(directory),
        extract_command
    );
    let output = manager
        .exec_cancellable(session_id, &command, cancellation)
        .await?;
    if !output.contains("__ZTOK__") {
        return Err(anyhow!(
            "远端解压失败，请检查压缩包内容、文件权限和剩余空间"
        ));
    }
    Ok(())
}

/// 校验压缩命令只能在普通文件管理模式下执行
async fn ensure_normal_exec_mode(manager: &SessionManager, session_id: &str) -> Result<()> {
    if manager.is_sudo(session_id).await? {
        return Err(anyhow!(
            "sudo 文件管理模式暂不支持压缩和解压"
        ));
    }
    Ok(())
}

/// 探测远端压缩工具是否可用
async fn ensure_remote_tool(
    manager: &SessionManager,
    session_id: &str,
    tool: &str,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<()> {
    let command = format!(
        "command -v {} >/dev/null 2>&1 && printf __ZTOK__ || printf __ZTNO__",
        tool
    );
    let output = manager
        .exec_cancellable(session_id, &command, cancellation)
        .await?;
    if !output.contains("__ZTOK__") {
        return Err(anyhow!("远端未找到 {} 命令", tool));
    }
    Ok(())
}

/// 中断或异常后限时清理压缩临时包，清理失败不覆盖原始错误
async fn cleanup_archive_temp(
    manager: &SessionManager,
    session_id: &str,
    directory: &str,
    temp_name: &str,
) {
    let command = format!(
        "cd {} && rm -f -- {}",
        shell_quote(directory),
        shell_quote(temp_name)
    );
    let _ = timeout(Duration::from_secs(5), manager.exec(session_id, &command)).await;
}

/// 校验删除路径为非根绝对路径
fn validate_removal_path(path: &str) -> Result<()> {
    let is_root = path.trim_matches('/').is_empty();
    let has_parent_component = path
        .split('/')
        .any(|component| component == "." || component == "..");
    if path.is_empty()
        || !path.starts_with('/')
        || path.contains('\0')
        || is_root
        || has_parent_component
    {
        return Err(anyhow!("非法的删除路径"));
    }
    Ok(())
}

/// 在 SFTP 安全边界检查是否收到中断通知
fn ensure_not_cancelled(cancellation: Option<&watch::Receiver<bool>>) -> Result<()> {
    if cancellation.is_some_and(|receiver| *receiver.borrow()) {
        return Err(anyhow!(OPERATION_CANCELLED_MESSAGE));
    }
    Ok(())
}

/// 校验远端工作目录为绝对路径且不含空字符
fn validate_directory(directory: &str) -> Result<()> {
    if !directory.starts_with('/') || directory.contains('\0') {
        return Err(anyhow!("非法的远端目录路径"));
    }
    Ok(())
}

/// 校验名称为当前目录下的单个条目，禁止路径穿越
fn validate_entry_name(name: &str, label: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(anyhow!("{}不合法", label));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_archive_command, validate_directory, validate_entry_name, validate_removal_path,
    };

    /// tar 包内条目不应携带会被 Windows 解压工具显示为目录层的 ./ 前缀
    #[test]
    fn tar_archive_entries_have_no_current_directory_prefix() {
        let command = build_archive_command(
            "./临时包.tar.gz",
            &["普通文件".to_string(), "-特殊文件".to_string()],
            "tarGz",
        );

        assert_eq!(
            command,
            "tar -czf './临时包.tar.gz' -- '普通文件' '-特殊文件' >/dev/null 2>&1"
        );
    }

    /// 单层条目名允许 shell 特殊字符，但拒绝路径穿越与空字符
    #[test]
    fn validates_single_entry_names() {
        assert!(validate_entry_name("普通 文件'名", "文件名").is_ok());
        for name in ["", ".", "..", "子目录/文件", "文件\0名"] {
            assert!(validate_entry_name(name, "文件名").is_err());
        }
    }

    /// 工作目录必须为不含空字符的绝对路径
    #[test]
    fn validates_absolute_directories() {
        assert!(validate_directory("/").is_ok());
        assert!(validate_directory("/目录/子目录").is_ok());
        assert!(validate_directory("相对路径").is_err());
        assert!(validate_directory("/目录\0子目录").is_err());
    }

    /// 删除仅允许非根绝对路径
    #[test]
    fn validates_removal_paths() {
        assert!(validate_removal_path("/目录/文件").is_ok());
        assert!(validate_removal_path("/目录/ 文件 ").is_ok());
        for path in [
            "",
            "/",
            "//",
            "相对路径",
            "/.",
            "/..",
            "/目录/./文件",
            "/目录/../文件",
            "/目录\0文件",
        ] {
            assert!(validate_removal_path(path).is_err());
        }
    }
}
