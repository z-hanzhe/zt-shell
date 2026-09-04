<script setup lang="ts">
/**
 * 设置弹窗：界面、终端、下载与监控相关的基础设置
 */
import { reactive, ref, watch } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import Icon from "./Icon.vue";
import AppDialog from "./AppDialog.vue";
import { pathIsDir } from "../api";
import {
  createDefaultAppSettings,
  type AppSettings,
} from "../stores/settings";
import { UI_SCALE_OPTIONS } from "../uiScale";
import {
  MAX_EDITOR_FONT_SIZE,
  MIN_EDITOR_FONT_SIZE,
} from "../editorProtocol";
import { useDialogDrag } from "../composables/useDialogDrag";
import { useEscClose } from "../composables/useEscClose";

const props = defineProps<{
  /** 当前设置 */
  settings: AppSettings;
}>();

const emit = defineEmits<{
  (e: "save", settings: AppSettings): void;
  (e: "preview-ui-scale", scale: number): void;
  (e: "close"): void;
}>();

/** 设置弹窗拖动控制器 */
const { dialogRef, onDialogHeaderPointerDown } = useDialogDrag();

const form = reactive<AppSettings>({ ...props.settings });

/** 是否正在校验并保存设置 */
const submitting = ref(false);
/** 是否正在读取默认设置 */
const resetting = ref(false);
/** 是否显示恢复默认设置确认 */
const resetConfirmOpen = ref(false);
/** 重置表单时抑制一次缩放预览，确保保存前不应用默认值 */
let suppressNextUiScalePreview = false;

watch(
  () => props.settings,
  (s) => Object.assign(form, s),
  { immediate: true }
);

watch(
  () => form.uiScale,
  (scale) => {
    if (suppressNextUiScalePreview) {
      suppressNextUiScalePreview = false;
      return;
    }
    emit("preview-ui-scale", scale);
  }
);

/** 请求确认是否恢复默认设置 */
function requestReset(): void {
  if (submitting.value || resetting.value) return;
  resetConfirmOpen.value = true;
}

/** 将默认值写入表单，等待用户手动保存 */
async function confirmReset(): Promise<void> {
  resetConfirmOpen.value = false;
  resetting.value = true;
  try {
    const defaultSettings = await createDefaultAppSettings();
    suppressNextUiScalePreview = form.uiScale !== defaultSettings.uiScale;
    Object.assign(form, defaultSettings);
  } catch (error) {
    alert(`读取默认设置失败：${String(error)}`);
  } finally {
    resetting.value = false;
  }
}

/** 打开系统目录选择框并回填下载路径 */
async function selectDownloadPath(): Promise<void> {
  try {
    const picked = await openDialog({
      directory: true,
      title: "选择下载路径",
      defaultPath: form.downloadPath.trim() || undefined,
    });
    if (typeof picked === "string") form.downloadPath = picked;
  } catch (error) {
    alert(`选择下载路径失败：${String(error)}`);
  }
}

/** 校验下载路径后保存设置 */
async function submit(): Promise<void> {
  if (submitting.value) return;
  const downloadPath = form.downloadPath.trim();
  if (!downloadPath) {
    alert("请填写下载路径");
    return;
  }
  submitting.value = true;
  try {
    if (isTauri() && !(await pathIsDir(downloadPath))) {
      alert("下载路径不存在或不是文件夹，请重新选择");
      return;
    }
    form.downloadPath = downloadPath;
    emit("save", { ...form });
  } catch (error) {
    alert(`检查下载路径失败：${String(error)}`);
  } finally {
    submitting.value = false;
  }
}

// 组件挂载即为打开状态，ESC 关闭
const { isTop: isTopModal } = useEscClose(
  () => true,
  () => emit("close")
);
</script>

<template>
  <div
    :class="['modal-mask', { 'modal-top-mask': isTopModal }]"
    :inert="!isTopModal"
    :aria-hidden="isTopModal ? undefined : 'true'"
  >
    <div
      ref="dialogRef"
      class="modal dialog-draggable settings-modal"
      role="dialog"
      :aria-modal="isTopModal ? 'true' : 'false'"
    >
      <div class="modal-header dialog-drag-handle" @pointerdown="onDialogHeaderPointerDown">
        <span>设置</span>
        <button class="modal-close" title="关闭" @click="emit('close')">×</button>
      </div>
      <div class="modal-body">
        <div class="set-grid">
          <label>界面缩放</label>
          <select v-model.number="form.uiScale" class="input settings-select">
            <option
              v-for="option in UI_SCALE_OPTIONS"
              :key="option.value"
              :value="option.value"
            >
              {{ option.label }}
            </option>
          </select>

          <label>终端字号</label>
          <input class="input" type="number" min="8" max="32" v-model.number="form.fontSize" />

          <label>编辑器字号</label>
          <input
            v-model.number="form.editorFontSize"
            class="input"
            type="number"
            :min="MIN_EDITOR_FONT_SIZE"
            :max="MAX_EDITOR_FONT_SIZE"
          />

          <label>字体</label>
          <input class="input" v-model="form.fontFamily" />

          <label>下载路径</label>
          <div class="path-field">
            <input
              v-model="form.downloadPath"
              class="input"
              placeholder="请选择本地下载目录"
            />
            <button
              class="path-picker"
              type="button"
              title="选择下载路径"
              aria-label="选择下载路径"
              @click="selectDownloadPath"
            >
              <Icon name="folder" :size="15" />
            </button>
          </div>

          <label>光标闪烁</label>
          <label class="switch">
            <input type="checkbox" v-model="form.cursorBlink" />
            <span>启用</span>
          </label>

          <label>监控间隔(秒)</label>
          <input class="input" type="number" min="1" max="30" v-model.number="form.monitorInterval" />
        </div>
      </div>
      <div class="modal-footer">
        <button
          class="btn"
          type="button"
          :disabled="submitting || resetting"
          @click="requestReset"
        >
          重置
        </button>
        <button
          class="btn"
          type="button"
          :disabled="submitting || resetting"
          @click="emit('close')"
        >
          取消
        </button>
        <button
          class="btn btn-primary"
          type="button"
          :disabled="submitting || resetting"
          @click="submit"
        >
          {{ submitting ? "保存中" : "保存" }}
        </button>
      </div>
    </div>
  </div>

  <AppDialog
    :open="resetConfirmOpen"
    type="confirm"
    title="恢复默认设置"
    message="确定要恢复为默认设置吗？确认后仍需点击保存才会生效。"
    confirm-text="恢复默认"
    @confirm="confirmReset"
    @cancel="resetConfirmOpen = false"
  />
</template>

<style scoped>
.settings-modal {
  width: min(520px, calc(100vw - 24px));
}
.set-grid {
  display: grid;
  grid-template-columns: 100px 1fr;
  gap: 12px;
  align-items: center;
}
.set-grid > label:nth-child(odd) {
  color: var(--text-secondary);
  text-align: right;
}
.settings-select {
  width: 100%;
  cursor: pointer;
}
.path-field {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 30px;
  gap: 6px;
  min-width: 0;
}
.path-field .input {
  box-sizing: border-box;
  width: 100%;
  min-width: 0;
}
.path-picker {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 28px;
  padding: 0;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-panel);
  color: var(--text-secondary);
  cursor: pointer;
}
.path-picker:hover {
  border-color: var(--accent);
  background: var(--bg-hover);
  color: var(--accent);
}
.switch {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--text-primary);
  cursor: pointer;
}
@media (max-height: 700px) {
  .modal-body {
    padding: 8px 14px;
  }
  .set-grid {
    gap: 4px 12px;
  }
  .settings-modal .input {
    height: 24px;
  }
  .modal-footer {
    padding: 6px 14px;
  }
}
</style>
