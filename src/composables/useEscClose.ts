/**
 * 弹窗 ESC 关闭：为所有模态弹窗提供统一的 ESC 关闭行为
 *
 * 维护一个全局关闭回调栈，ESC 仅触发当前最顶层弹窗的关闭回调，
 * 天然支持嵌套弹窗（如连接管理器内的编辑弹窗、文本编辑器内的二次确认）。
 */
import { onBeforeUnmount, watch, type WatchSource } from "vue";

/** 打开中的弹窗关闭回调栈，栈顶为最上层弹窗 */
const escStack: Array<() => void> = [];
/** 全局监听是否已绑定，首个弹窗打开时惰性绑定 */
let listenerBound = false;

/** 全局 ESC 监听：命中时触发栈顶弹窗的关闭回调并阻止继续传播 */
function onGlobalEsc(event: KeyboardEvent) {
  if (event.key !== "Escape" || escStack.length === 0) return;
  event.preventDefault();
  event.stopPropagation();
  escStack[escStack.length - 1]();
}

/** 是否存在打开中的模态弹窗，供其他 ESC 处理逻辑避让 */
export function hasOpenModal(): boolean {
  return escStack.length > 0;
}

/**
 * 注册弹窗的 ESC 关闭行为
 * @param isOpen 弹窗是否显示的响应式来源
 * @param close 关闭弹窗的回调
 */
export function useEscClose(isOpen: WatchSource<boolean>, close: () => void) {
  const handler = () => close();

  /** 入栈并确保全局监听已绑定 */
  function push() {
    if (!escStack.includes(handler)) escStack.push(handler);
    if (!listenerBound) {
      window.addEventListener("keydown", onGlobalEsc, true);
      listenerBound = true;
    }
  }

  /** 出栈 */
  function pop() {
    const index = escStack.indexOf(handler);
    if (index >= 0) escStack.splice(index, 1);
  }

  watch(isOpen, (open) => (open ? push() : pop()), { immediate: true });
  onBeforeUnmount(pop);
}
