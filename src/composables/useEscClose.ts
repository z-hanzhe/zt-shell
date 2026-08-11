/**
 * 弹窗 ESC 关闭：为所有模态弹窗提供统一的 ESC 关闭行为
 *
 * 维护一个全局关闭回调栈，ESC 仅触发当前最顶层弹窗的关闭回调，
 * 天然支持嵌套弹窗（如连接管理器内的编辑弹窗、文本编辑器内的二次确认）。
 */
import {
  computed,
  onBeforeUnmount,
  shallowReactive,
  watch,
  type ComputedRef,
  type WatchSource,
} from "vue";

/** 模态弹窗 Escape 响应优先级 */
export const ESC_MODAL_PRIORITY = {
  BUSINESS: 0,
  ELEVATED: 1,
  WINDOW: 2,
} as const;

type EscEntry = {
  close: () => void;
  priority: () => number;
};

export type EscCloseController = {
  /** 当前弹窗是否为实际响应键盘操作的最上层弹窗 */
  isTop: ComputedRef<boolean>;
};

/** 打开中的弹窗关闭回调栈，同优先级时后打开的弹窗位于上层 */
const escStack = shallowReactive<EscEntry[]>([]);
/** 全局监听是否已绑定，首个弹窗打开时惰性绑定 */
let listenerBound = false;

/** 按优先级与打开顺序查找当前最上层弹窗 */
function findTopEntry(): EscEntry | undefined {
  let target = escStack[0];
  for (const entry of escStack.slice(1)) {
    if (entry.priority() >= target.priority()) target = entry;
  }
  return target;
}

/** 全局 ESC 监听：命中时触发栈顶弹窗的关闭回调并阻止继续传播 */
function onGlobalEsc(event: KeyboardEvent) {
  if (event.key !== "Escape" || escStack.length === 0) return;
  event.preventDefault();
  event.stopImmediatePropagation();
  if (event.repeat) return;
  findTopEntry()?.close();
}

/** 是否存在打开中的模态弹窗，供其他 ESC 处理逻辑避让 */
export function hasOpenModal(): boolean {
  return escStack.length > 0;
}

/**
 * 注册弹窗的 ESC 关闭行为
 * @param isOpen 弹窗是否显示的响应式来源
 * @param close 关闭弹窗的回调
 * @param priority 弹窗层级优先级，数值更大的弹窗优先响应 ESC
 * @returns 当前弹窗的全局层级状态
 */
export function useEscClose(
  isOpen: WatchSource<boolean>,
  close: () => void,
  priority: () => number = () => ESC_MODAL_PRIORITY.BUSINESS
): EscCloseController {
  const entry: EscEntry = { close, priority };
  const isTop = computed(
    () => escStack.includes(entry) && findTopEntry() === entry
  );

  /** 入栈并确保全局监听已绑定 */
  function push() {
    if (!escStack.includes(entry)) escStack.push(entry);
    if (!listenerBound) {
      window.addEventListener("keydown", onGlobalEsc, true);
      listenerBound = true;
    }
  }

  /** 出栈 */
  function pop() {
    const index = escStack.indexOf(entry);
    if (index >= 0) escStack.splice(index, 1);
  }

  watch(isOpen, (open) => (open ? push() : pop()), { immediate: true });
  onBeforeUnmount(pop);
  return { isTop };
}
