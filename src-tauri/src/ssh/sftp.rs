//! SFTP 文件操作：目录列举、读写、增删改等

use anyhow::{anyhow, Result};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileType;
use tokio::io::AsyncWriteExt;

use super::types::FileEntry;

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
    let mut all: Vec<(String, bool)> = vec![(path.to_string(), true)];
    let mut stack: Vec<String> = vec![path.to_string()];
    while let Some(dir) = stack.pop() {
        let read = sftp
            .read_dir(&dir)
            .await
            .map_err(|e| anyhow!("读取目录失败（{}）：{}", dir, format_sftp_error(&e)))?;
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
        if *is_dir {
            remove_dir(sftp, entry_path).await?;
        } else {
            remove_file(sftp, entry_path).await?;
        }
    }
    Ok(())
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
