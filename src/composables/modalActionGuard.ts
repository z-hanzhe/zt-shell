/**
 * 模态弹窗操作门禁：避免窗口级弹窗关闭后，同一次双击继续命中下层弹窗
 */

const WINDOW_MODAL_ACTION_LOCK_MS = 400;
let actionLockedUntil = 0;

/**
 * 尝试开始一次用户触发的弹窗操作
 * @param windowModal 当前操作是否来自窗口级弹窗
 * @param event 触发操作的可选界面事件
 */
export function tryBeginModalAction(windowModal: boolean, event?: Event): boolean {
  const now = Date.now();
  if (now < actionLockedUntil) return false;
  if (event instanceof MouseEvent && event.detail > 1) return false;
  if (windowModal) actionLockedUntil = now + WINDOW_MODAL_ACTION_LOCK_MS;
  return true;
}
