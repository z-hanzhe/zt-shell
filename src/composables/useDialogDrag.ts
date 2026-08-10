/**
 * 弹窗标题栏拖动：统一管理指针捕获、视口边界与重新打开时的位置重置
 */
import {
  nextTick,
  onBeforeUnmount,
  onMounted,
  ref,
  watch,
  type Ref,
  type WatchSource,
} from "vue";

const VIEWPORT_MARGIN = 8;
const INTERACTIVE_SELECTOR =
  "button, a, input, select, textarea, label, [contenteditable='true'], [role='button'], [data-dialog-no-drag]";

type DragStart = {
  pointerX: number;
  pointerY: number;
  dialogLeft: number;
  dialogTop: number;
  offsetX: number;
  offsetY: number;
};

export type DialogDragController = {
  /** 弹窗容器引用 */
  dialogRef: Ref<HTMLElement | null>;
  /** 标题栏按下事件 */
  onDialogHeaderPointerDown: (event: PointerEvent) => void;
};

/** 将数值限制在指定闭区间内 */
function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

/** 计算弹窗单轴位置，正常尺寸完整留在视口内，超大尺寸允许在两侧边界间移动 */
function clampAxisPosition(position: number, size: number, viewportSize: number): number {
  const oppositeEdge = viewportSize - VIEWPORT_MARGIN - size;
  const min = Math.min(VIEWPORT_MARGIN, oppositeEdge);
  const max = Math.max(VIEWPORT_MARGIN, oppositeEdge);
  return clamp(position, min, max);
}

/**
 * 创建弹窗拖动控制器
 * @param isOpen 可选的弹窗显示状态，用于每次重新打开时恢复居中
 */
export function useDialogDrag(isOpen?: WatchSource<boolean>): DialogDragController {
  const dialogRef = ref<HTMLElement | null>(null);
  let offsetX = 0;
  let offsetY = 0;
  let activePointerId: number | null = null;
  let dragHeader: HTMLElement | null = null;
  let dragStart: DragStart | null = null;
  let resizeObserver: ResizeObserver | null = null;

  /** 将当前偏移写入弹窗定位样式 */
  function applyDialogOffset() {
    const dialog = dialogRef.value;
    if (!dialog) return;
    dialog.style.left = `${offsetX}px`;
    dialog.style.top = `${offsetY}px`;
  }

  /** 恢复弹窗到遮罩层的默认居中位置 */
  function resetDialogPosition() {
    offsetX = 0;
    offsetY = 0;
    applyDialogOffset();
  }

  /** 将弹窗当前矩形夹取到视口允许范围 */
  function clampDialogToViewport() {
    const dialog = dialogRef.value;
    if (!dialog) return;
    const rect = dialog.getBoundingClientRect();
    const nextLeft = clampAxisPosition(rect.left, rect.width, window.innerWidth);
    const nextTop = clampAxisPosition(rect.top, rect.height, window.innerHeight);
    offsetX += nextLeft - rect.left;
    offsetY += nextTop - rect.top;
    applyDialogOffset();
  }

  /** 移除一次拖动绑定并释放指针捕获 */
  function stopDragging() {
    const pointerId = activePointerId;
    const header = dragHeader;
    activePointerId = null;
    dragHeader = null;
    dragStart = null;

    window.removeEventListener("pointermove", onPointerMove);
    window.removeEventListener("pointerup", onPointerEnd);
    window.removeEventListener("pointercancel", onPointerEnd);
    window.removeEventListener("blur", stopDragging);

    if (!header) return;
    header.classList.remove("dialog-header-dragging");
    header.removeEventListener("lostpointercapture", onLostPointerCapture);
    if (pointerId !== null && header.hasPointerCapture(pointerId)) {
      header.releasePointerCapture(pointerId);
    }
  }

  /** 根据当前指针位置更新弹窗偏移 */
  function onPointerMove(event: PointerEvent) {
    if (event.pointerId !== activePointerId || !dragStart) return;
    const dialog = dialogRef.value;
    if (!dialog) {
      stopDragging();
      return;
    }

    const rect = dialog.getBoundingClientRect();
    const nextLeft = clampAxisPosition(
      dragStart.dialogLeft + event.clientX - dragStart.pointerX,
      rect.width,
      window.innerWidth
    );
    const nextTop = clampAxisPosition(
      dragStart.dialogTop + event.clientY - dragStart.pointerY,
      rect.height,
      window.innerHeight
    );
    offsetX = dragStart.offsetX + nextLeft - dragStart.dialogLeft;
    offsetY = dragStart.offsetY + nextTop - dragStart.dialogTop;
    applyDialogOffset();
  }

  /** 在对应指针抬起或取消时结束拖动 */
  function onPointerEnd(event: PointerEvent) {
    if (event.pointerId === activePointerId) stopDragging();
  }

  /** 指针捕获意外丢失时结束拖动 */
  function onLostPointerCapture() {
    stopDragging();
  }

  /** 从非交互标题栏区域开始拖动弹窗 */
  function onDialogHeaderPointerDown(event: PointerEvent) {
    if (!event.isPrimary || event.button !== 0) return;
    const target = event.target;
    if (target instanceof Element && target.closest(INTERACTIVE_SELECTOR)) return;

    const dialog = dialogRef.value;
    const header = event.currentTarget;
    if (!dialog || !(header instanceof HTMLElement)) return;

    stopDragging();
    const rect = dialog.getBoundingClientRect();
    activePointerId = event.pointerId;
    dragHeader = header;
    dragStart = {
      pointerX: event.clientX,
      pointerY: event.clientY,
      dialogLeft: rect.left,
      dialogTop: rect.top,
      offsetX,
      offsetY,
    };

    event.preventDefault();
    header.classList.add("dialog-header-dragging");
    header.addEventListener("lostpointercapture", onLostPointerCapture);
    header.setPointerCapture(event.pointerId);
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerEnd);
    window.addEventListener("pointercancel", onPointerEnd);
    window.addEventListener("blur", stopDragging);
  }

  /** 弹窗 DOM 实例变化时重置位置并重新观察尺寸 */
  function onDialogElementChange(element: HTMLElement | null, previous: HTMLElement | null) {
    stopDragging();
    if (previous) resizeObserver?.unobserve(previous);
    resetDialogPosition();
    if (!element) return;
    resizeObserver ??= new ResizeObserver(clampDialogToViewport);
    resizeObserver.observe(element);
  }

  /** 弹窗重新打开后等待 DOM 更新并恢复居中 */
  async function onOpenChange(open: boolean) {
    if (!open) {
      stopDragging();
      return;
    }
    resetDialogPosition();
    await nextTick();
    resetDialogPosition();
    clampDialogToViewport();
  }

  watch(dialogRef, onDialogElementChange);
  if (isOpen) watch(isOpen, onOpenChange);

  onMounted(() => window.addEventListener("resize", clampDialogToViewport));
  onBeforeUnmount(() => {
    stopDragging();
    resizeObserver?.disconnect();
    window.removeEventListener("resize", clampDialogToViewport);
  });

  return { dialogRef, onDialogHeaderPointerDown };
}
