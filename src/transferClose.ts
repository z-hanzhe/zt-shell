/** 正在创建的传输任务，按所属会话登记 */
const pendingCreations = new Map<string, Set<Promise<unknown>>>();
/** 会话关闭准备引用计数，存在引用时禁止创建新的传输任务 */
const closePreparationCounts = new Map<string, number>();

/** 传输关闭准备；释放前目标会话不能创建新的传输任务 */
export interface TransferClosePreparation {
  /** 取消关闭或完成会话清理后释放创建门闩 */
  release: () => Promise<void>;
}

/** 增加指定会话的关闭准备引用 */
function retainClosePreparation(sessionIds: readonly string[]): void {
  for (const sessionId of sessionIds) {
    closePreparationCounts.set(
      sessionId,
      (closePreparationCounts.get(sessionId) ?? 0) + 1
    );
  }
}

/** 释放指定会话的关闭准备引用 */
function dropClosePreparation(sessionIds: readonly string[]): void {
  for (const sessionId of sessionIds) {
    const next = (closePreparationCounts.get(sessionId) ?? 0) - 1;
    if (next > 0) closePreparationCounts.set(sessionId, next);
    else closePreparationCounts.delete(sessionId);
  }
}

/** 从会话登记中移除已经结束的传输创建请求 */
function removePendingCreation(
  sessionId: string,
  operation: Promise<unknown>
): void {
  const operations = pendingCreations.get(sessionId);
  if (!operations) return;
  operations.delete(operation);
  if (operations.size === 0) pendingCreations.delete(sessionId);
}

/**
 * 在统一边界执行并登记传输创建请求；关闭准备期间的新请求会被拒绝。
 */
export function runTransferCreation<T>(
  sessionId: string,
  create: () => Promise<T>
): Promise<T> {
  if (closePreparationCounts.has(sessionId)) {
    return Promise.reject(new Error("会话正在关闭，无法创建新的传输任务"));
  }

  let operation: Promise<T>;
  try {
    operation = create();
  } catch (error) {
    return Promise.reject(error);
  }

  let operations = pendingCreations.get(sessionId);
  if (!operations) {
    operations = new Set();
    pendingCreations.set(sessionId, operations);
  }
  operations.add(operation);
  void operation.then(
    () => removePendingCreation(sessionId, operation),
    () => removePendingCreation(sessionId, operation)
  );
  return operation;
}

/**
 * 阻止目标会话创建新传输，并等待已经进入统一 API 边界的创建请求全部落定。
 */
export async function prepareTransferClose(
  sessionIds: readonly string[]
): Promise<TransferClosePreparation> {
  const targetIds = [...new Set(sessionIds)].filter(Boolean);
  let released = false;
  retainClosePreparation(targetIds);

  /** 释放本次关闭准备持有的创建门闩 */
  async function release(): Promise<void> {
    if (released) return;
    dropClosePreparation(targetIds);
    released = true;
  }

  const pending = targetIds.flatMap((sessionId) => [
    ...(pendingCreations.get(sessionId) ?? []),
  ]);
  await Promise.allSettled(pending);
  return { release };
}
