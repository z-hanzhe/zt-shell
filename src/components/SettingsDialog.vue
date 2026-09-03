<script setup lang="ts">
/**
 * 设置弹窗：终端与界面相关的基础设置
 */
import { reactive, watch } from "vue";
import type { AppSettings } from "../stores/settings";
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

watch(
  () => props.settings,
  (s) => Object.assign(form, s),
  { immediate: true }
);

watch(
  () => form.uiScale,
  (scale) => emit("preview-ui-scale", scale)
);

/** 保存设置 */
function submit() {
  emit("save", { ...form });
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
        <button class="btn" @click="emit('close')">取消</button>
        <button class="btn btn-primary" @click="submit">保存</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.settings-modal {
  width: min(420px, calc(100vw - 24px));
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
