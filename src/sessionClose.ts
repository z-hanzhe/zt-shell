import type { TransferTask } from "./types";

/** 会触发关闭保护的会话状态 */
const LIVE_SESSION_STATUSES = new Set(["connecting", "verifying", "connected"]);

/** 判断会话是否仍处于连接生命周期中 */
export function isLiveSessionStatus(status: string): boolean {
  return LIVE_SESSION_STATUSES.has(status);
}

/** 统计目标会话内尚未完成的顶层传输任务 */
export function countUnfinishedTransferTasks(
  tasks: TransferTask[],
  sessionIds: readonly string[]
): number {
  const targets = new Set(sessionIds);
  return tasks.filter(
    (task) =>
      targets.has(task.sessionId) &&
      task.parentId === null &&
      task.status !== "completed" &&
      task.status !== "cancelled"
  ).length;
}

/** 会话关闭保护所需的外部操作 */
export interface SessionCloseGuardOptions {
  /** 每个检查阶段开始前重新取得仍在线的目标会话 */
  getLiveSessionIds: () => string[];
  /** 现有的连接中会话关闭确认；省略表示调用方已确认 */
  confirmLiveSessions?: (sessionIds: string[]) => Promise<boolean>;
  /** 查询目标会话的未保存文档数量 */
  queryUnsavedDocuments: (sessionIds: string[]) => Promise<number>;
  /** 询问是否放弃未保存文档 */
  confirmUnsavedDocuments: (
    count: number,
    sessionIds: string[]
  ) => Promise<boolean>;
  /** 锁定目标会话的传输创建入口后取得当前任务快照 */
  queryTransfers: (sessionIds: string[]) => Promise<TransferTask[]>;
  /** 询问是否终止未完成传输 */
  confirmUnfinishedTransfers: (
    count: number,
    sessionIds: string[]
  ) => Promise<boolean>;
}

/**
 * 依次执行会话关闭保护；任一确认取消时立即停止，且不执行任何资源清理。
 */
export async function confirmSessionClose(
  options: SessionCloseGuardOptions
): Promise<boolean> {
  let liveSessionIds = options.getLiveSessionIds();
  if (liveSessionIds.length === 0) return true;

  if (
    options.confirmLiveSessions &&
    !(await options.confirmLiveSessions(liveSessionIds))
  ) {
    return false;
  }

  liveSessionIds = options.getLiveSessionIds();
  if (liveSessionIds.length === 0) return true;
  const unsavedCount = await options.queryUnsavedDocuments(liveSessionIds);
  if (
    unsavedCount > 0 &&
    !(await options.confirmUnsavedDocuments(unsavedCount, liveSessionIds))
  ) {
    return false;
  }

  liveSessionIds = options.getLiveSessionIds();
  if (liveSessionIds.length === 0) return true;
  const transferCount = countUnfinishedTransferTasks(
    await options.queryTransfers(liveSessionIds),
    liveSessionIds
  );
  if (
    transferCount > 0 &&
    !(await options.confirmUnfinishedTransfers(transferCount, liveSessionIds))
  ) {
    return false;
  }

  return true;
}
