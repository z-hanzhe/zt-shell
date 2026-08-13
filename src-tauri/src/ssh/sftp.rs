//! SFTP 文件操作：目录列举、读写、增删改等

use anyhow::{anyhow, Result};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, FileType};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use tokio::time::{timeout, Duration};
use uuid::Uuid;

use super::manager::SessionManager;
use super::session::{wait_for_cancellation, OPERATION_CANCELLED_MESSAGE};
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

/// 修改远端文件或目录的 Unix 权限位。
///
/// 权限修改使用 SFTP `SETSTAT`，不依赖远端 shell。递归处理目录时不跟随符号链接，
/// `scope` 可限制为全部条目、仅文件或仅目录；中断不会回滚已经完成的修改。
pub async fn set_permissions(
    sftp: &SftpSession,
    path: &str,
    mode: u32,
    recursive: bool,
    scope: &str,
    cancellation: Option<&watch::Receiver<bool>>,
) -> Result<()> {
    validate_permission_path(path)?;
    validate_permission_mode(mode)?;
    validate_permission_scope(scope)?;
    validate_permission_application(recursive, scope)?;

    let root_metadata = sftp
        .symlink_metadata(path)
        .await
        .map_err(|e| anyhow!("读取权限失败：{}", format_sftp_error(&e)))?;
    let root_type = root_metadata.file_type();
    if root_type == FileType::Symlink {
        return Err(anyhow!("符号链接不支持直接修改权限"));
    }
    let mut targets = vec![(path.to_string(), root_type)];
    if recursive && root_type == FileType::Dir {
        collect_permission_targets(sftp, path, &mut targets, cancellation).await?;
    }

    // 子项先于父目录修改，避免提前移除父目录执行权限后无法继续访问后代。
    for (target_path, file_type) in targets.into_iter().rev() {
        ensure_not_cancelled(cancellation)?;
        if recursive && !permission_scope_matches(scope, &file_type) {
            continue;
        }
        // 只保留文件类型位并替换权限位；清除 setuid/setgid/sticky，确保界面中的
        // 八进制权限值与实际结果一致，避免修改普通权限时意外保留特殊提权位。
        if file_type == FileType::Symlink {
            continue;
        }
        let current = sftp
            .symlink_metadata(&target_path)
            .await
            .map_err(|e| anyhow!("读取权限失败（{}）：{}", target_path, format_sftp_error(&e)))?
            .permissions
            .unwrap_or(0);
        let mut attrs = FileAttributes::empty();
        attrs.permissions = Some((current & 0o170000) | mode);
        sftp.set_metadata(&target_path, attrs)
            .await
            .map_err(|e| anyhow!("修改权限失败（{}）：{}", target_path, format_sftp_error(&e)))?;
    }
    ensure_not_cancelled(cancellation)?;
    Ok(())
}

/// 递归收集目录下的条目；符号链接按叶子节点处理，避免跟随链接造成越界或循环。
async fn collect_permission_targets(
    sftp: &SftpSession,
    root: &str,
    targets: &mut Vec<(String, FileType)>,
    cancellation: Option<&watch::Receiver<bool>>,
) -> Result<()> {
    let mut stack = vec![root.to_string()];
    while let Some(directory) = stack.pop() {
        ensure_not_cancelled(cancellation)?;
        let children = sftp
            .read_dir(&directory)
            .await
            .map_err(|e| anyhow!("读取目录失败（{}）：{}", directory, format_sftp_error(&e)))?;
        for item in children {
            ensure_not_cancelled(cancellation)?;
            let child = format!("{}/{}", directory.trim_end_matches('/'), item.file_name());
            let file_type = sftp
                .symlink_metadata(&child)
                .await
                .map_err(|e| anyhow!("读取权限失败（{}）：{}", child, format_sftp_error(&e)))?
                .file_type();
            targets.push((child.clone(), file_type));
            if file_type == FileType::Dir {
                stack.push(child);
            }
        }
    }
    Ok(())
}

/// 判断递归权限修改的范围过滤条件。
fn permission_scope_matches(scope: &str, file_type: &FileType) -> bool {
    match scope {
        // “仅应用到文件”只匹配普通文件，符号链接和特殊文件不应被误改。
        "files" => *file_type == FileType::File,
        "directories" => *file_type == FileType::Dir,
        _ => true,
    }
}

/// 校验权限模式仅包含 Unix 九位 rwx 权限位。
fn validate_permission_mode(mode: u32) -> Result<()> {
    if mode > 0o777 {
        return Err(anyhow!("权限值必须在 000 到 777 之间"));
    }
    Ok(())
}

/// 校验递归权限修改范围。
fn validate_permission_scope(scope: &str) -> Result<()> {
    if matches!(scope, "all" | "files" | "directories") {
        Ok(())
    } else {
        Err(anyhow!("权限应用范围不合法"))
    }
}

/// 校验非递归模式不能选择仅文件或仅目录范围。
fn validate_permission_application(recursive: bool, scope: &str) -> Result<()> {
    if !recursive && scope != "all" {
        return Err(anyhow!("非递归权限修改只能应用到全部条目"));
    }
    Ok(())
}

/// 校验权限修改路径，拒绝相对路径、根目录和路径穿越。
fn validate_permission_path(path: &str) -> Result<()> {
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
        return Err(anyhow!("非法的权限修改路径"));
    }
    Ok(())
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

/// 将内容写入远端文件，并在收到操作中断通知时停止等待后续写入
///
/// SFTP 覆盖写不提供事务回滚；中断发生在创建文件之后时，远端文件可能已写入部分内容。
pub async fn write_file_cancellable(
    sftp: &SftpSession,
    path: &str,
    data: &[u8],
    cancellation: &mut watch::Receiver<bool>,
) -> Result<()> {
    ensure_not_cancelled(Some(cancellation))?;
    tokio::select! {
        biased;
        _ = wait_for_cancellation(cancellation) => Err(anyhow!(OPERATION_CANCELLED_MESSAGE)),
        result = write_file(sftp, path, data) => result,
    }
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

/// 支持的远端归档格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
}

/// 根据文件名后缀识别归档格式（不识别单文件压缩流，如 `.gz`）。
fn detect_archive_kind(name: &str) -> Option<ArchiveKind> {
    let lower_name = name.to_ascii_lowercase();
    if lower_name.ends_with(".zip") {
        Some(ArchiveKind::Zip)
    } else if lower_name.ends_with(".tar.gz") || lower_name.ends_with(".tgz") {
        Some(ArchiveKind::TarGz)
    } else if lower_name.ends_with(".tar.bz2")
        || lower_name.ends_with(".tbz2")
        || lower_name.ends_with(".tbz")
    {
        Some(ArchiveKind::TarBz2)
    } else if lower_name.ends_with(".tar.xz") || lower_name.ends_with(".txz") {
        Some(ArchiveKind::TarXz)
    } else if lower_name.ends_with(".tar") {
        Some(ArchiveKind::Tar)
    } else {
        None
    }
}

/// 构建远端解压命令，归档路径和目标路径均须为已校验的 shell 参数。
fn build_extract_command(kind: ArchiveKind, archive_path: &str, destination: &str) -> String {
    match kind {
        ArchiveKind::Zip => format!(
            "{} && unzip -oq {} -d {} >/dev/null 2>&1",
            build_zip_entry_validation_command(archive_path),
            shell_quote(archive_path),
            shell_quote(destination)
        ),
        ArchiveKind::Tar => format!(
            "tar -xf {} -C {} >/dev/null 2>&1",
            shell_quote(archive_path),
            shell_quote(destination)
        ),
        ArchiveKind::TarGz => format!(
            "tar -xzf {} -C {} >/dev/null 2>&1",
            shell_quote(archive_path),
            shell_quote(destination)
        ),
        ArchiveKind::TarBz2 => format!(
            "tar -xjf {} -C {} >/dev/null 2>&1",
            shell_quote(archive_path),
            shell_quote(destination)
        ),
        ArchiveKind::TarXz => format!(
            "tar -xJf {} -C {} >/dev/null 2>&1",
            shell_quote(archive_path),
            shell_quote(destination)
        ),
    }
}

/// 构建 ZIP 条目路径校验命令，拒绝绝对路径和会穿越目标目录的 `..` 条目。
///
/// `unzip` 的不同实现对路径清理策略并不完全一致，因此在解压前显式检查条目名。
/// 仅依赖 POSIX shell 和 Info-ZIP 常见的 `-Z1` 列表模式；列表失败时整个操作失败。
fn build_zip_entry_validation_command(archive_path: &str) -> String {
    let archive = shell_quote(archive_path);
    format!(
        "unzip -Z1 {archive} >/dev/null 2>&1 && if unzip -Z1 {archive} | while IFS= read -r entry; do case \"$entry\" in /*|../*|*/../*|*/..|..|*\\\\**) exit 1 ;; esac; done; then :; else exit 1; fi",
        archive = archive
    )
}

/// 构建“解压到指定子目录”的命令。
///
/// 先在随机 staging 目录中解压，再将内容合并到目标目录。若 staging 顶层
/// 只有一个与目标同名的真实目录，则复制该目录的内容而不是再嵌套一层。
fn build_extract_to_directory_command(
    archive_path: &str,
    staging_path: &str,
    target_path: &str,
    target_name: &str,
    kind: ArchiveKind,
) -> String {
    let extract_command = match kind {
        ArchiveKind::Zip => "unzip -oq \"$archive\" -d \"$staging\" >/dev/null 2>&1",
        ArchiveKind::Tar => "tar -xf \"$archive\" -C \"$staging\" >/dev/null 2>&1",
        ArchiveKind::TarGz => "tar -xzf \"$archive\" -C \"$staging\" >/dev/null 2>&1",
        ArchiveKind::TarBz2 => "tar -xjf \"$archive\" -C \"$staging\" >/dev/null 2>&1",
        ArchiveKind::TarXz => "tar -xJf \"$archive\" -C \"$staging\" >/dev/null 2>&1",
    };
    let archive = shell_quote(archive_path);
    let staging = shell_quote(staging_path);
    let target = shell_quote(target_path);
    let target_name = shell_quote(target_name);
    let zip_validation = if kind == ArchiveKind::Zip {
        format!("{} && ", build_zip_entry_validation_command(archive_path))
    } else {
        String::new()
    };
    // `cp -a source/. target/.` 同时包含隐藏文件并保留归档中的属性与符号链接。
    format!(
        "archive={archive}; staging={staging}; target={target}; target_name={target_name}; (rm -rf -- \"$staging\" || exit 1) && (mkdir -- \"$staging\" || exit 1) && ({zip_validation}{extract} || exit 1) && (if [ -e \"$target\" ] || [ -L \"$target\" ]; then if [ ! -d \"$target\" ] || [ -L \"$target\" ]; then exit 1; fi; else mkdir -- \"$target\" || exit 1; fi; target_links=$(find \"$target\" -type l -print) || exit 1; if [ -n \"$target_links\" ]; then exit 1; fi; top_count=0; only_name=; for item in \"$staging\"/* \"$staging\"/.[!.]* \"$staging\"/..?*; do if [ -e \"$item\" ] || [ -L \"$item\" ]; then top_count=$((top_count + 1)); only_name=\"${{item##*/}}\"; fi; done; if [ \"$top_count\" -eq 1 ] && [ \"$only_name\" = \"$target_name\" ] && [ -d \"$staging/$target_name\" ] && [ ! -L \"$staging/$target_name\" ]; then cp -a \"$staging/$target_name/.\" \"$target/.\" || exit 1; else cp -a \"$staging/.\" \"$target/.\" || exit 1; fi; rm -rf -- \"$staging\" || exit 1) && printf __ZTOK__ || {{ rm -rf -- \"$staging\"; printf __ZTFAIL__; }}",
        extract = extract_command,
        zip_validation = zip_validation,
        archive = archive,
        staging = staging,
        target = target,
        target_name = target_name,
    )
}

/// 将远端归档解压到当前目录或指定子目录。
pub async fn extract_archive(
    manager: &SessionManager,
    session_id: &str,
    directory: &str,
    archive_name: &str,
    target_directory: Option<&str>,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<()> {
    ensure_normal_exec_mode(manager, session_id).await?;
    validate_directory(directory)?;
    validate_entry_name(archive_name, "压缩包名称")?;

    let kind = detect_archive_kind(archive_name).ok_or_else(|| {
        anyhow!("仅支持解压 zip、tar、tar.gz、tgz、tar.bz2、tbz2、tbz、tar.xz 或 txz 文件")
    })?;
    let tool = match kind {
        ArchiveKind::Zip => "unzip",
        ArchiveKind::Tar | ArchiveKind::TarGz | ArchiveKind::TarBz2 | ArchiveKind::TarXz => "tar",
    };
    ensure_remote_tool(manager, session_id, tool, cancellation).await?;

    let archive_path = format!("./{}", archive_name);
    let (command, staging_path) = if let Some(target_name) = target_directory {
        validate_entry_name(target_name, "目标目录名")?;
        let staging_name = format!(".ztshell-extract-{}", Uuid::new_v4());
        let staging_path = format!("./{staging_name}");
        let target_path = format!("./{target_name}");
        let command = format!(
            "cd {} && ({})",
            shell_quote(directory),
            build_extract_to_directory_command(
                &archive_path,
                &staging_path,
                &target_path,
                target_name,
                kind,
            )
        );
        (command, Some(staging_path))
    } else {
        let extract_command = build_extract_command(kind, &archive_path, ".");
        let command = format!(
            "cd {} && {} && printf __ZTOK__ || printf __ZTFAIL__",
            shell_quote(directory),
            extract_command
        );
        (command, None)
    };
    let output = match manager
        .exec_cancellable(session_id, &command, cancellation)
        .await
    {
        Ok(output) => output,
        Err(error) => {
            if let Some(staging_path) = staging_path.as_deref() {
                cleanup_extract_temp(manager, session_id, directory, staging_path).await;
            }
            return Err(error);
        }
    };
    if !output.contains("__ZTOK__") {
        if let Some(staging_path) = staging_path.as_deref() {
            cleanup_extract_temp(manager, session_id, directory, staging_path).await;
        }
        return Err(anyhow!(
            "远端解压失败，请检查压缩包内容、文件权限和剩余空间"
        ));
    }
    Ok(())
}

/// 校验压缩命令只能在普通文件管理模式下执行
async fn ensure_normal_exec_mode(manager: &SessionManager, session_id: &str) -> Result<()> {
    if manager.is_sudo(session_id).await? {
        return Err(anyhow!("sudo 文件管理模式暂不支持压缩和解压"));
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

/// 中断或异常后限时清理解压 staging 目录，清理失败不覆盖原始错误。
async fn cleanup_extract_temp(
    manager: &SessionManager,
    session_id: &str,
    directory: &str,
    staging_path: &str,
) {
    let command = format!(
        "cd {} && rm -rf -- {}",
        shell_quote(directory),
        shell_quote(staging_path)
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
    use russh_sftp::protocol::FileType;

    use super::{
        build_archive_command, build_extract_command, build_extract_to_directory_command,
        detect_archive_kind, permission_scope_matches, validate_directory, validate_entry_name,
        validate_permission_application, validate_permission_mode, validate_permission_path,
        validate_permission_scope, validate_removal_path, ArchiveKind,
    };
    /// 常见归档扩展名映射到正确的解压工具格式。
    #[test]
    fn detects_supported_archive_kinds() {
        for (name, kind) in [
            ("a.ZIP", ArchiveKind::Zip),
            ("a.tar", ArchiveKind::Tar),
            ("a.tar.gz", ArchiveKind::TarGz),
            ("a.tgz", ArchiveKind::TarGz),
            ("a.tar.bz2", ArchiveKind::TarBz2),
            ("a.tbz2", ArchiveKind::TarBz2),
            ("a.tbz", ArchiveKind::TarBz2),
            ("a.tar.xz", ArchiveKind::TarXz),
            ("a.txz", ArchiveKind::TarXz),
        ] {
            assert_eq!(detect_archive_kind(name), Some(kind));
        }
        for name in ["a.gz", "a.7z", "a.rar", "a"] {
            assert_eq!(detect_archive_kind(name), None);
        }
    }

    /// 解压命令使用对应 tar 过滤器并正确引用路径。
    #[test]
    fn builds_extract_commands() {
        assert_eq!(
            build_extract_command(ArchiveKind::TarBz2, "./包.tar.bz2", "./目标"),
            "tar -xjf './包.tar.bz2' -C './目标' >/dev/null 2>&1"
        );
        let zip_command = build_extract_command(ArchiveKind::Zip, "./包.zip", "./目标 目录");
        assert!(zip_command.contains("unzip -oq './包.zip' -d './目标 目录'"));
        assert!(zip_command.contains("unzip -Z1 './包.zip'"));
        assert!(zip_command.contains("then :; else exit 1; fi"));
        assert!(!zip_command.contains("if ! unzip -Z1"));
        assert!(zip_command.contains("*\\\\**"));
    }

    /// 目标目录命令包含隐藏文件 glob 和同名顶层目录剥离分支。
    #[test]
    fn builds_named_extract_command_with_deduplication() {
        let command = build_extract_to_directory_command(
            "./包.tar.gz",
            "./.staging",
            "./包",
            "包",
            ArchiveKind::TarGz,
        );
        assert!(command.contains("\"$staging\"/*"));
        assert!(command.contains("\"$staging\"/.[!.]*"));
        assert!(command.contains("\"$staging\"/..?*"));
        assert!(command.contains("[ \"$only_name\" = \"$target_name\" ]"));
        assert!(command.contains("cp -a \"$staging/$target_name/.\" \"$target/.\""));
    }

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

    /// 权限值、应用范围和目标路径校验拒绝越界输入。
    #[test]
    fn validates_permission_arguments() {
        assert!(validate_permission_mode(0o755).is_ok());
        assert!(validate_permission_mode(0o1000).is_err());
        assert!(validate_permission_scope("all").is_ok());
        assert!(validate_permission_scope("files").is_ok());
        assert!(validate_permission_scope("directories").is_ok());
        assert!(validate_permission_scope("bad").is_err());
        assert!(validate_permission_application(false, "all").is_ok());
        assert!(validate_permission_application(false, "files").is_err());
        assert!(validate_permission_application(true, "files").is_ok());
        assert!(validate_permission_path("/tmp/item").is_ok());
        assert!(validate_permission_path("/").is_err());
        assert!(validate_permission_path("relative").is_err());
        assert!(validate_permission_path("/tmp/../item").is_err());
        assert!(permission_scope_matches("files", &FileType::File));
        assert!(!permission_scope_matches("files", &FileType::Dir));
        assert!(!permission_scope_matches("files", &FileType::Symlink));
        assert!(!permission_scope_matches("files", &FileType::Other));
        assert!(permission_scope_matches("directories", &FileType::Dir));
        assert!(!permission_scope_matches("directories", &FileType::File));
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
