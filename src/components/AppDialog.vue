<script setup lang="ts">
/**
 * 通用轻量弹窗：支持提示、确认与单输入，避免使用浏览器内置弹窗行为
 */
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { tryBeginModalAction } from "../composables/modalActionGuard";
import { useDialogDrag } from "../composables/useDialogDrag";
import { ESC_MODAL_PRIORITY, useEscClose } from "../composables/useEscClose";

const FOCUSABLE_SELECTOR =
  "button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])";

const props = withDefaults(
  defineProps<{
    /** 是否显示弹窗 */
    open: boolean;
    /** 弹窗标题 */
    title: string;
    /** 主体提示文案 */
    message?: string;
    /** 弹窗类型，loading 为不可关闭的进行中提示，可配置附加操作按钮 */
    type?: "info" | "confirm" | "prompt" | "loading";
    /** 输入框默认值 */
    defaultValue?: string;
    /** 输入框占位文案 */
    placeholder?: string;
    /** 确认按钮文案 */
    confirmText?: string;
    /** 取消按钮文案 */
    cancelText?: string;
    /** 确认按钮是否使用红色警示样式 */
    confirmDanger?: boolean;
    /** 输入提示模板，使用 {value} 表示当前输入值 */
    hintTemplate?: string;
    /** 进行中弹窗的附加操作按钮文案，非空时显示红色警示按钮 */
    loadingActionText?: string;
    /** 进行中弹窗附加操作按钮是否禁用 */
    loadingActionDisabled?: boolean;
    /** 是否为窗口关闭确认，允许遮罩覆盖自绘标题栏 */
    windowModal?: boolean;
  }>(),
  {
    message: "",
    type: "info",
    defaultValue: "",
    placeholder: "",
    confirmText: "确定",
    cancelText: "取消",
    confirmDanger: false,
    hintTemplate: "",
    loadingActionText: "",
    loadingActionDisabled: false,
    windowModal: false,
  }
);

const emit = defineEmits<{
  (e: "confirm", value: string): void;
  (e: "cancel"): void;
  (e: "loading-action"): void;
}>();

const inputValue = ref("");
const inputRef = ref<HTMLInputElement | null>(null);
const confirmButtonRef = ref<HTMLButtonElement | null>(null);
/** 弹窗打开前拥有焦点的控件，用于关闭后恢复 */
let previousFocus: HTMLElement | null = null;
/** 焦点切换序号，避免快速开关时执行过期任务 */
let focusChangeSequence = 0;
/** 弹窗拖动控制器，每次重新显示时恢复居中 */
const { dialogRef, onDialogHeaderPointerDown } = useDialogDrag(
  () => props.open,
  () => props.windowModal
);

/** 根据当前输入内容生成提示文案 */
const hintText = computed(() => {
  if (!props.hintTemplate) return "";
  return props.hintTemplate.replace("{value}", inputValue.value || props.placeholder);
});

const { isTop: isTopModal } = useEscClose(
  () => props.open,
  () => requestCancel(),
  () =>
    props.windowModal ? ESC_MODAL_PRIORITY.WINDOW : ESC_MODAL_PRIORITY.BUSINESS
);

/** 获取弹窗内当前可通过键盘聚焦的控件 */
function focusableControls(): HTMLElement[] {
  const dialog = dialogRef.value;
  if (!dialog) return [];
  return Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
}

/** 将焦点放到当前弹窗的默认操作控件 */
function focusDefaultControl(): void {
  if (!props.open || !isTopModal.value) return;
  const target =
    (props.type === "prompt" ? inputRef.value : confirmButtonRef.value) ??
    focusableControls()[0] ??
    dialogRef.value;
  target?.focus();
}

/** 在当前弹窗关闭前仍持有焦点时，恢复弹窗打开前的键盘焦点 */
function restorePreviousFocus(ownedFocus: boolean): void {
  const target = previousFocus;
  previousFocus = null;
  if (ownedFocus && target?.isConnected) target.focus();
}

/** 同步弹窗开关对应的初始值和焦点 */
async function onOpenChange(open: boolean): Promise<void> {
  const sequence = ++focusChangeSequence;
  if (!open) {
    const ownedFocus = dialogRef.value?.contains(document.activeElement) ?? false;
    await nextTick();
    if (sequence === focusChangeSequence && !props.open) {
      restorePreviousFocus(ownedFocus);
    }
    return;
  }
  previousFocus = document.activeElement instanceof HTMLElement
    ? document.activeElement
    : null;
  inputValue.value = props.defaultValue;
  await nextTick();
  if (sequence === focusChangeSequence) focusDefaultControl();
}

/** 将 Tab 导航限制在当前最上层弹窗中 */
function trapModalFocus(event: KeyboardEvent): void {
  if (event.key !== "Tab" || !isTopModal.value) return;
  const controls = focusableControls();
  if (controls.length === 0) {
    event.preventDefault();
    dialogRef.value?.focus();
    return;
  }
  const currentIndex = controls.indexOf(document.activeElement as HTMLElement);
  const shouldWrapBackward = event.shiftKey && currentIndex <= 0;
  const shouldWrapForward = !event.shiftKey && currentIndex === controls.length - 1;
  if (!shouldWrapBackward && !shouldWrapForward && currentIndex >= 0) return;
  event.preventDefault();
  controls[event.shiftKey ? controls.length - 1 : 0].focus();
}

watch(() => props.open, onOpenChange, { immediate: true });
watch(isTopModal, async (isTop) => {
  if (!isTop || !props.open) return;
  await nextTick();
  await nextTick();
  focusDefaultControl();
});
onBeforeUnmount(() => {
  focusChangeSequence += 1;
  const ownedFocus = dialogRef.value?.contains(document.activeElement) ?? false;
  restorePreviousFocus(ownedFocus);
});

/** 确认当前弹窗，并拦截跨层重复操作 */
function submit(event?: Event) {
  if (!tryBeginModalAction(props.windowModal, event)) return;
  emit("confirm", inputValue.value);
}

/** 请求取消：进行中弹窗不可关闭 */
function requestCancel(event?: Event) {
  if (props.type === "loading") return;
  if (!tryBeginModalAction(props.windowModal, event)) return;
  emit("cancel");
}

</script>

<template>
  <div
    v-if="open"
    :class="[
      'modal-mask',
      { 'window-modal-mask': windowModal, 'modal-top-mask': isTopModal },
    ]"
    :inert="!isTopModal"
    :aria-hidden="isTopModal ? undefined : 'true'"
  >
    <div
      ref="dialogRef"
      class="modal dialog-draggable app-dialog"
      role="dialog"
      :aria-modal="isTopModal ? 'true' : 'false'"
      tabindex="-1"
      @keydown="trapModalFocus"
    >
      <div class="modal-header dialog-drag-handle" @pointerdown="onDialogHeaderPointerDown">
        <span>{{ title }}</span>
        <button v-if="type !== 'loading'" class="modal-close" title="关闭" @click="requestCancel">×</button>
      </div>
      <div class="modal-body app-dialog-body">
        <div v-if="type === 'loading'" class="app-dialog-loading">
          <span class="spinner"></span>
          <span>{{ message }}</span>
        </div>
        <div v-else-if="message" class="app-dialog-message">{{ message }}</div>
        <input
          v-if="type === 'prompt'"
          ref="inputRef"
          class="input app-dialog-input"
          v-model="inputValue"
          :placeholder="placeholder"
          @keyup.enter="submit"
        />
        <div v-if="hintText" class="app-dialog-hint">{{ hintText }}</div>
      </div>
      <div v-if="type !== 'loading' || loadingActionText" class="modal-footer">
        <button
          v-if="type === 'loading' && loadingActionText"
          class="btn btn-danger"
          :disabled="loadingActionDisabled"
          @click="emit('loading-action')"
        >
          {{ loadingActionText }}
        </button>
        <button v-if="type === 'confirm' || type === 'prompt'" class="btn" @click="requestCancel">
          {{ cancelText }}
        </button>
        <button
          v-if="type !== 'loading'"
          ref="confirmButtonRef"
          :class="['btn', confirmDanger ? 'btn-danger' : 'btn-primary']"
          @click="submit"
        >
          {{ confirmText }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.app-dialog {
  width: 360px;
}
.app-dialog-body {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.app-dialog-message {
  line-height: 1.6;
  color: var(--text-secondary);
  white-space: pre-line;
  word-break: break-word;
}
.app-dialog-input {
  width: 100%;
}
.app-dialog-hint {
  padding: 6px 8px;
  border: 1px solid var(--border-light);
  border-radius: var(--radius);
  background: #f7f9fb;
  color: var(--text-muted);
  font-family: "Consolas", "Cascadia Mono", monospace;
  font-size: 12px;
  word-break: break-all;
}
/* 进行中提示：转圈图标 + 文案 */
.app-dialog-loading {
  display: flex;
  align-items: center;
  gap: 10px;
  color: var(--text-secondary);
  padding: 4px 0;
}
.spinner {
  width: 16px;
  height: 16px;
  flex: 0 0 auto;
  border: 2px solid #c9d6e4;
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: app-dialog-spin 0.8s linear infinite;
}
@keyframes app-dialog-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
