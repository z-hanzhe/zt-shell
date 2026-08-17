/**
 * 后端 Tauri 命令的前端封装，集中管理 invoke 调用
 */

import { invoke, Channel } from "@tauri-apps/api/core";
import type {
  ConnectionConfig,
  ConnectionExportCredentialSources,
  ConnectOutcome,
  CredentialCopy,
  CredentialKey,
  CredentialMatch,
  CredentialWrite,
  FileEntry,
  HostKeyApproval,
  MonitorData,
  ProxyConfig,
  TransferCreateResult,
  TransferTask,
} from "./types";
import { runTransferCreation } from "./transferClose";

/** 建立 SSH 连接，未知或变化的主机密钥会先返回确认信息 */
export function sshConnect(
  config: ConnectionConfig & { proxy?: ProxyConfig },
  savedConnectionId: string,
  savedProxyId: string | null,
  hostKeyApproval?: HostKeyApproval
): Promise<ConnectOutcome> {
  return invoke("ssh_connect", {
    config,
    savedConnectionId,
    savedProxyId,
    hostKeyApproval: hostKeyApproval ?? null,
  });
}

/** 批量写入系统凭据库 */
export function credentialsSetMany(changes: CredentialWrite[]): Promise<void> {
  return invoke("credentials_set_many", { changes });
}

/** 批量检查系统凭据是否存在，结果顺序与输入一致 */
export function credentialsCheckMany(keys: CredentialKey[]): Promise<boolean[]> {
  return invoke("credentials_check_many", { keys });
}

/** 读取连接编辑器使用的登录密码 */
export function credentialsGetConnectionPassword(id: string): Promise<string | null> {
  return invoke("credentials_get_connection_password", { id });
}

/** 批量比较代理密码，结果顺序与输入一致且不返回已存明文 */
export function credentialsMatchMany(changes: CredentialMatch[]): Promise<boolean[]> {
  return invoke("credentials_match_many", { changes });
}

/** 批量删除系统凭据 */
export function credentialsDeleteMany(keys: CredentialKey[]): Promise<void> {
  return invoke("credentials_delete_many", { keys });
}

/** 批量复制系统凭据 */
export function credentialsCopyMany(changes: CredentialCopy[]): Promise<void> {
  return invoke("credentials_copy_many", { changes });
}

/** 选择并读取一个连接导入文件，取消时返回 null */
export function pickConnectionImportFile(): Promise<string | null> {
  return invoke("pick_connection_import_file");
}

/** 选择保存位置并由原生层注入系统凭据后写入连接导出文件 */
export function saveConnectionExportFile(
  content: string,
  defaultFileName: string,
  credentialSources: ConnectionExportCredentialSources
): Promise<boolean> {
  return invoke("save_connection_export_file", {
    content,
    defaultFileName,
    credentialSources,
  });
}

/** 断开会话；重连前保留传输任务时传入 keepTransfers */
export function sshDisconnect(sessionId: string, keepTransfers = false): Promise<void> {
  return invoke("ssh_disconnect", { sessionId, keepTransfers });
}

/**
 * 开启终端。终端输出通过 ipc::Channel 按序接收 Raw 二进制数据
 */
export function terminalOpen(
  sessionId: string,
  cols: number,
  rows: number,
  onData: Channel<ArrayBuffer>
): Promise<void> {
  return invoke("terminal_open", { sessionId, cols, rows, onData });
}

/** 向终端写入数据 */
export function terminalWrite(sessionId: string, data: number[]): Promise<void> {
  return invoke("terminal_write", { sessionId, data });
}

/** 变更终端尺寸 */
export function terminalResize(
  sessionId: string,
  cols: number,
  rows: number
): Promise<void> {
  return invoke("terminal_resize", { sessionId, cols, rows });
}

/** 判断本地路径是否为目录（终端拖拽上传前校验，仅允许单文件） */
export function pathIsDir(path: string): Promise<boolean> {
  return invoke("path_is_dir", { path });
}

/** 采集监控数据 */
export function monitorCollect(sessionId: string): Promise<MonitorData> {
  return invoke("monitor_collect", { sessionId });
}

/** 列举目录 */
export function sftpList(sessionId: string, path: string): Promise<FileEntry[]> {
  return invoke("sftp_list", { sessionId, path });
}

/** 获取主目录路径 */
export function sftpHome(sessionId: string): Promise<string> {
  return invoke("sftp_home", { sessionId });
}

/** 读取文件 */
export function sftpRead(sessionId: string, path: string): Promise<number[]> {
  return invoke("sftp_read", { sessionId, path });
}

/** 写入文件 */
export function sftpWrite(
  sessionId: string,
  path: string,
  data: number[],
  operationId?: string
): Promise<void> {
  return invoke("sftp_write", { sessionId, path, data, operationId: operationId ?? null });
}

/** 删除文件 */
export function sftpRemoveFile(sessionId: string, path: string): Promise<void> {
  return invoke("sftp_remove_file", { sessionId, path });
}

/** 删除目录 */
export function sftpRemoveDir(sessionId: string, path: string): Promise<void> {
  return invoke("sftp_remove_dir", { sessionId, path });
}

/** 创建目录 */
export function sftpCreateDir(sessionId: string, path: string): Promise<void> {
  return invoke("sftp_create_dir", { sessionId, path });
}

/** 重命名/移动 */
export function sftpRename(
  sessionId: string,
  from: string,
  to: string
): Promise<void> {
  return invoke("sftp_rename", { sessionId, from, to });
}

/** 上传本地文件到远端 */
export function sftpUpload(
  sessionId: string,
  localPath: string,
  remotePath: string
): Promise<void> {
  return invoke("sftp_upload", { sessionId, localPath, remotePath });
}

/** 下载远端文件到本地 */
export function sftpDownload(
  sessionId: string,
  remotePath: string,
  localPath: string
): Promise<void> {
  return invoke("sftp_download", { sessionId, remotePath, localPath });
}

/** 将远端文件压缩到当前目录 */
export function sftpCreateArchive(
  sessionId: string,
  directory: string,
  names: string[],
  archiveFormat: "zip" | "tarGz",
  archiveName: string,
  operationId: string
): Promise<void> {
  return invoke("sftp_create_archive", { sessionId, directory, names, archiveFormat, archiveName, operationId });
}

/** 将远端压缩包解压到当前目录或指定子目录 */
export function sftpExtractArchive(
  sessionId: string,
  directory: string,
  archiveName: string,
  operationId: string,
  targetDirectory?: string
): Promise<void> {
  return invoke("sftp_extract_archive", {
    sessionId,
    directory,
    archiveName,
    operationId,
    targetDirectory: targetDirectory ?? null,
  });
}

/** 批量删除远端条目，目录递归删除支持中断 */
export function sftpRemoveEntries(
  sessionId: string,
  entries: { path: string; isDir: boolean }[],
  operationId: string
): Promise<void> {
  return invoke("sftp_remove_entries", { sessionId, entries, operationId });
}

/** 请求中断文件管理中的长耗时操作 */
export function sftpCancelOperation(sessionId: string, operationId: string): Promise<boolean> {
  return invoke("sftp_cancel_operation", { sessionId, operationId });
}

/** 切换 sudo 提权文件管理开关 */
export function sftpSetSudo(sessionId: string, enabled: boolean): Promise<void> {
  return invoke("sftp_set_sudo", { sessionId, enabled });
}

/** 检测当前权限模式下对远端文件是否有写入权限 */
export function sftpCheckWritable(sessionId: string, path: string): Promise<boolean> {
  return invoke("sftp_check_writable", { sessionId, path });
}

/** 修改远端文件或目录权限，支持递归应用范围。 */
export function sftpSetPermissions(
  sessionId: string,
  path: string,
  mode: number,
  recursive: boolean,
  scope: "all" | "files" | "directories",
  operationId: string
): Promise<void> {
  return invoke("sftp_set_permissions", {
    sessionId,
    path,
    mode,
    recursive,
    scope,
    operationId,
  });
}

/** 创建上传任务，force 确认超量、overwrite 确认覆盖，未确认时仅返回统计 */
export function transferUpload(
  sessionId: string,
  localPaths: string[],
  remoteDir: string,
  force: boolean,
  overwrite: boolean
): Promise<TransferCreateResult> {
  return runTransferCreation(sessionId, () =>
    invoke("transfer_upload", { sessionId, localPaths, remoteDir, force, overwrite })
  );
}

/** 创建下载任务，force 与 overwrite 含义同上传 */
export function transferDownload(
  sessionId: string,
  items: { path: string; isDir: boolean }[],
  localDir: string,
  force: boolean,
  overwrite: boolean
): Promise<TransferCreateResult> {
  return runTransferCreation(sessionId, () =>
    invoke("transfer_download", { sessionId, items, localDir, force, overwrite })
  );
}

/** 创建打包下载任务（远端 tar 打包后下载） */
export function transferPackDownload(
  sessionId: string,
  remoteDir: string,
  names: string[],
  localPath: string
): Promise<void> {
  return runTransferCreation(sessionId, () =>
    invoke("transfer_pack_download", { sessionId, remoteDir, names, localPath })
  );
}

/** 列出全部传输任务 */
export function transferList(): Promise<TransferTask[]> {
  return invoke("transfer_list");
}

/** 暂停传输任务，不传 ids 表示全部 */
export function transferPause(ids?: string[]): Promise<void> {
  return invoke("transfer_pause", { ids: ids ?? null });
}

/** 继续传输任务，不传 ids 表示全部 */
export function transferResume(ids?: string[]): Promise<void> {
  return invoke("transfer_resume", { ids: ids ?? null });
}

/** 删除传输任务（级联子任务），不传 ids 表示全部 */
export function transferRemove(ids?: string[]): Promise<void> {
  return invoke("transfer_remove", { ids: ids ?? null });
}

/** 重试失败的传输任务，不传 sessionId 表示全部会话 */
export function transferRetryFailed(sessionId?: string): Promise<void> {
  return invoke("transfer_retry_failed", { sessionId: sessionId ?? null });
}
