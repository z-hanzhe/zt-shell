/**
 * 后端 Tauri 命令的前端封装，集中管理 invoke 调用
 */

import { invoke } from "@tauri-apps/api/core";
import type {
  ConnectionConfig,
  FileEntry,
  MonitorData,
  TransferCreateResult,
  TransferTask,
} from "./types";

/** 建立 SSH 连接，返回会话标识 */
export function sshConnect(config: ConnectionConfig): Promise<string> {
  return invoke("ssh_connect", { config });
}

/** 断开会话 */
export function sshDisconnect(sessionId: string): Promise<void> {
  return invoke("ssh_disconnect", { sessionId });
}

/** 开启终端 */
export function terminalOpen(
  sessionId: string,
  cols: number,
  rows: number
): Promise<void> {
  return invoke("terminal_open", { sessionId, cols, rows });
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
  data: number[]
): Promise<void> {
  return invoke("sftp_write", { sessionId, path, data });
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

/** 切换 sudo 提权文件管理开关 */
export function sftpSetSudo(sessionId: string, enabled: boolean): Promise<void> {
  return invoke("sftp_set_sudo", { sessionId, enabled });
}

/** 检测当前权限模式下对远端文件是否有写入权限 */
export function sftpCheckWritable(sessionId: string, path: string): Promise<boolean> {
  return invoke("sftp_check_writable", { sessionId, path });
}

/** 创建上传任务，force 确认超量、overwrite 确认覆盖，未确认时仅返回统计 */
export function transferUpload(
  sessionId: string,
  localPaths: string[],
  remoteDir: string,
  force: boolean,
  overwrite: boolean
): Promise<TransferCreateResult> {
  return invoke("transfer_upload", { sessionId, localPaths, remoteDir, force, overwrite });
}

/** 创建下载任务，force 与 overwrite 含义同上传 */
export function transferDownload(
  sessionId: string,
  items: { path: string; isDir: boolean }[],
  localDir: string,
  force: boolean,
  overwrite: boolean
): Promise<TransferCreateResult> {
  return invoke("transfer_download", { sessionId, items, localDir, force, overwrite });
}

/** 创建打包下载任务（远端 tar 打包后下载） */
export function transferPackDownload(
  sessionId: string,
  remoteDir: string,
  names: string[],
  localPath: string
): Promise<void> {
  return invoke("transfer_pack_download", { sessionId, remoteDir, names, localPath });
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
