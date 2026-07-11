//! SFTP 文件操作：目录列举、读写、增删改等

use anyhow::{anyhow, Result};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileType;

use super::types::FileEntry;

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

/// 列举远端目录内容
pub async fn list_dir(sftp: &SftpSession, path: &str) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    let read_dir = sftp
        .read_dir(path)
        .await
        .map_err(|e| anyhow!("读取目录失败：{}", e))?;
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
            owner: meta.uid.map(|u| u.to_string()).unwrap_or_default(),
            group: meta.gid.map(|g| g.to_string()).unwrap_or_default(),
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
        .map_err(|e| anyhow!("读取文件失败：{}", e))
}

/// 将内容写入远端文件（覆盖）
pub async fn write_file(sftp: &SftpSession, path: &str, data: &[u8]) -> Result<()> {
    sftp.write(path, data)
        .await
        .map_err(|e| anyhow!("写入文件失败：{}", e))
}

/// 删除远端文件
pub async fn remove_file(sftp: &SftpSession, path: &str) -> Result<()> {
    sftp.remove_file(path)
        .await
        .map_err(|e| anyhow!("删除文件失败：{}", e))
}

/// 删除远端空目录
pub async fn remove_dir(sftp: &SftpSession, path: &str) -> Result<()> {
    sftp.remove_dir(path)
        .await
        .map_err(|e| anyhow!("删除目录失败：{}", e))
}

/// 创建远端目录
pub async fn create_dir(sftp: &SftpSession, path: &str) -> Result<()> {
    sftp.create_dir(path)
        .await
        .map_err(|e| anyhow!("创建目录失败：{}", e))
}

/// 重命名（移动）远端文件或目录
pub async fn rename(sftp: &SftpSession, from: &str, to: &str) -> Result<()> {
    sftp.rename(from, to)
        .await
        .map_err(|e| anyhow!("重命名失败：{}", e))
}

/// 获取远端用户主目录的绝对路径
pub async fn canonicalize(sftp: &SftpSession, path: &str) -> Result<String> {
    sftp.canonicalize(path)
        .await
        .map_err(|e| anyhow!("解析路径失败：{}", e))
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
