<script setup lang="ts">
/**
 * 远端 Unix 权限编辑弹窗：按属主、属组和其他用户分别设置读写执行权限，
 * 可选递归应用到子目录，并通过范围单选项决定文件与目录的覆盖范围。
 */
import { nextTick, reactive, ref, watch } from "vue";
import { tryBeginModalAction } from "../composables/modalActionGuard";
import { useDialogDrag } from "../composables/useDialogDrag";
import { ESC_MODAL_PRIORITY, useEscClose } from "../composables/useEscClose";

export type PermissionScope = "all" | "files" | "directories";

export type PermissionDialogValue = {
  /** Unix 九位权限值，仅包含 000-777 */
  mode: number;
  /** 是否递归处理目录内容 */
  recursive: boolean;
  /** 递归时应用到全部条目、文件或目录 */
  scope: PermissionScope;
};

const props = withDefaults(
  defineProps<{
    /** 是否显示弹窗 */
    open: boolean;
    /** 当前选中目标的展示名称 */
    targetName?: string;
    /** 多选目标数量，数量大于一时用于标题提示 */
    targetCount?: number;
    /** 多选目标权限不一致时显示明确提示 */
    mixedMode?: boolean;
    /** 当前权限值，用于打开弹窗时初始化复选框 */
    initialMode?: number;
    /** 是否正在提交权限修改 */
    saving?: boolean;
  }>(),
  {
    targetName: "",
    targetCount: 1,
    mixedMode: false,
    initialMode: 0o644,
    saving: false,
  }
);

const emit = defineEmits<{
  (e: "confirm", value: PermissionDialogValue): void;
  (e: "cancel"): void;
}>();

type PermissionGroup = "owner" | "group" | "other";
type PermissionBit = "read" | "write" | "exec";

const permissionBits: Record<PermissionGroup, Record<PermissionBit, number>> = {
  owner: { read: 0o400, write: 0o200, exec: 0o100 },
  group: { read: 0o040, write: 0o020, exec: 0o010 },
  other: { read: 0o004, write: 0o002, exec: 0o001 },
};

const groupLabels: Record<PermissionGroup, string> = {
  owner: "所有者",
  group: "组",
  other: "其他",
};

const state = reactive<Record<PermissionGroup, Record<PermissionBit, boolean>>>({
  owner: { read: true, write: true, exec: false },
  group: { read: true, write: false, exec: false },
  other: { read: true, write: false, exec: false },
});
/** 多选权限不一致时，要求用户显式调整权限后才能应用。 */
const modeEdited = ref(true);
const recursive = ref(false);
const scope = ref<PermissionScope>("all");
const confirmButtonRef = ref<HTMLButtonElement | null>(null);
let previousFocus: HTMLElement | null = null;

const { dialogRef, onDialogHeaderPointerDown } = useDialogDrag(() => props.open);
const { isTop: isTopModal } = useEscClose(
  () => props.open,
  () => cancel(),
  () => ESC_MODAL_PRIORITY.BUSINESS
);

/** 将 Unix 权限值转换为复选框状态。*/
function loadMode(mode: number): void {
  const normalized = Number.isFinite(mode) ? mode & 0o777 : 0o644;
  for (const group of Object.keys(permissionBits) as PermissionGroup[]) {
    for (const bit of Object.keys(permissionBits[group]) as PermissionBit[]) {
      state[group][bit] = (normalized & permissionBits[group][bit]) !== 0;
    }
  }
}

/** 计算复选框状态对应的 Unix 九位权限值。*/
function currentMode(): number {
  let mode = 0;
  for (const group of Object.keys(permissionBits) as PermissionGroup[]) {
    for (const bit of Object.keys(permissionBits[group]) as PermissionBit[]) {
      if (state[group][bit]) mode |= permissionBits[group][bit];
    }
  }
  return mode;
}

/** 将权限值格式化为三组八进制数字，方便用户核对结果。*/
function modeText(): string {
  return currentMode().toString(8).padStart(3, "0");
}

/** 标记用户已明确确认当前权限模式。 */
function markModeEdited(): void {
  modeEdited.value = true;
}

/** 获取弹窗内可聚焦控件并把焦点限制在当前弹窗。*/
function trapFocus(event: KeyboardEvent): void {
  if (event.key !== "Tab" || !isTopModal.value) return;
  const controls = Array.from(
    dialogRef.value?.querySelectorAll<HTMLElement>(
      "button:not([disabled]), input:not([disabled])"
    ) ?? []
  );
  if (controls.length === 0) {
    event.preventDefault();
    dialogRef.value?.focus();
    return;
  }
  const current = controls.indexOf(document.activeElement as HTMLElement);
  const next = event.shiftKey ? current <= 0 : current === controls.length - 1;
  if (!next && current >= 0) return;
  event.preventDefault();
  controls[event.shiftKey ? controls.length - 1 : 0].focus();
}

/** 同步弹窗打开状态、初始权限和默认焦点。*/
watch(
  () => props.open,
  async (open) => {
    if (!open) {
      if (dialogRef.value?.contains(document.activeElement)) {
        await nextTick();
        previousFocus?.focus();
      }
      previousFocus = null;
      return;
    }
    previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    loadMode(props.initialMode);
    modeEdited.value = !props.mixedMode;
    recursive.value = false;
    scope.value = "all";
    await nextTick();
    if (isTopModal.value) confirmButtonRef.value?.focus();
  },
  { immediate: true }
);

/** 提交权限修改设置。*/
function submit(event: Event): void {
  if (props.saving || (props.mixedMode && !modeEdited.value) || !tryBeginModalAction(false, event)) return;
  // 非递归模式不区分文件和目录，避免用户先选范围再取消递归后提交非法组合。
  emit("confirm", {
    mode: currentMode(),
    recursive: recursive.value,
    scope: recursive.value ? scope.value : "all",
  });
}

/** 请求关闭弹窗。*/
function cancel(event?: Event): void {
  if (props.saving || (event && !tryBeginModalAction(false, event))) return;
  emit("cancel");
}
</script>

<template>
  <div
    v-if="open"
    :class="['modal-mask', { 'modal-top-mask': isTopModal }]"
    :inert="!isTopModal"
    :aria-hidden="isTopModal ? undefined : 'true'"
  >
    <div
      ref="dialogRef"
      class="modal dialog-draggable permission-dialog"
      role="dialog"
      :aria-modal="isTopModal ? 'true' : 'false'"
      tabindex="-1"
      @keydown="trapFocus"
    >
      <div class="modal-header dialog-drag-handle" @pointerdown="onDialogHeaderPointerDown">
        <span>修改文件权限</span>
        <button class="modal-close" title="关闭" :disabled="saving" @click="cancel">×</button>
      </div>
      <div class="modal-body permission-dialog-body">
        <div class="permission-target" :title="targetName">
          {{ targetCount > 1 ? `已选择 ${targetCount} 个项目` : targetName || "当前项目" }}
        </div>
        <div v-if="mixedMode" class="permission-mixed-hint">
          所选项目的当前权限不一致，已按 644（八进制）初始化，请先调整权限后再应用。
        </div>
        <div class="permission-grid" role="group" aria-label="文件权限">
          <div class="permission-grid-head">
            <span></span><span>读取</span><span>写入</span><span>执行</span>
          </div>
          <div v-for="group in (['owner', 'group', 'other'] as PermissionGroup[])" :key="group" class="permission-row">
            <span class="permission-group-label">{{ groupLabels[group] }}</span>
            <label v-for="bit in (['read', 'write', 'exec'] as PermissionBit[])" :key="bit" class="permission-check">
              <input v-model="state[group][bit]" type="checkbox" :aria-label="`${groupLabels[group]}${bit === 'read' ? '读取' : bit === 'write' ? '写入' : '执行'}`" @change="markModeEdited" />
              <span class="permission-mark"></span>
            </label>
          </div>
        </div>
        <div class="permission-mode-line">权限：<code>{{ modeText() }}</code></div>
        <label class="permission-recursive">
          <input v-model="recursive" type="checkbox" />
          <span>递归设置子目录</span>
        </label>
        <fieldset class="permission-scope" :disabled="!recursive">
          <legend>应用范围</legend>
          <label><input v-model="scope" type="radio" value="all" />应用到文件和目录</label>
          <label><input v-model="scope" type="radio" value="files" />仅应用到文件</label>
          <label><input v-model="scope" type="radio" value="directories" />仅应用到目录</label>
        </fieldset>
      </div>
      <div class="modal-footer">
        <button class="btn" :disabled="saving" @click="cancel">取消</button>
        <button ref="confirmButtonRef" class="btn btn-primary" :disabled="saving || (mixedMode && !modeEdited)" @click="submit">
          {{ saving ? "应用中" : "确定" }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.permission-dialog {
  width: min(340px, calc(100vw - 24px));
}
.permission-dialog-body {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.permission-target {
  min-height: 24px;
  padding: 4px 6px;
  border: 1px solid var(--border-light);
  background: var(--bg-panel);
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.permission-mixed-hint {
  padding: 6px 8px;
  border: 1px solid #e2c47a;
  background: #fff8e1;
  color: #765b16;
  line-height: 1.4;
}
.permission-grid {
  border: 1px solid var(--border);
  background: #fff;
}
.permission-grid-head,
.permission-row {
  display: grid;
  grid-template-columns: minmax(76px, 1fr) repeat(3, 48px);
  align-items: center;
  min-height: 30px;
  border-bottom: 1px solid var(--border-light);
}
.permission-row:last-child {
  border-bottom: 0;
}
.permission-grid-head {
  min-height: 26px;
  color: var(--text-muted);
  font-size: 11px;
  text-align: center;
  background: var(--bg-panel);
}
.permission-group-label {
  padding-left: 10px;
  color: var(--text-secondary);
}
.permission-check {
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 30px;
  cursor: pointer;
}
.permission-check input,
.permission-recursive input,
.permission-scope input {
  accent-color: var(--accent);
}
.permission-check input {
  width: 14px;
  height: 14px;
  margin: 0;
}
.permission-mode-line {
  color: var(--text-muted);
}
.permission-mode-line code {
  color: var(--text);
  font-family: "Cascadia Mono", Consolas, monospace;
  font-weight: 600;
}
.permission-recursive {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--text-secondary);
}
.permission-recursive input {
  width: 14px;
  height: 14px;
  margin: 0;
}
.permission-scope {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin: 0;
  padding: 8px 10px;
  border: 1px solid var(--border);
  color: var(--text-secondary);
}
.permission-scope legend {
  padding: 0 4px;
  color: var(--text-muted);
  font-size: 11px;
}
.permission-scope label {
  display: flex;
  align-items: center;
  gap: 6px;
}
</style>
