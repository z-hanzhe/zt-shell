<script setup lang="ts">
/**
 * 设置弹窗：终端与界面相关的基础设置
 */
import { reactive, watch } from "vue";
import type { AppSettings } from "../stores/settings";
import { useDialogDrag } from "../composables/useDialogDrag";
import { useEscClose } from "../composables/useEscClose";

const props = defineProps<{
  /** 当前设置 */
  settings: AppSettings;
}>();

const emit = defineEmits<{
  (e: "save", settings: AppSettings): void;
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

/** 保存设置 */
function submit() {
  emit("save", { ...form });
  emit("close");
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
      class="modal dialog-draggable"
      style="width: 420px"
      role="dialog"
      :aria-modal="isTopModal ? 'true' : 'false'"
    >
      <div class="modal-header dialog-drag-handle" @pointerdown="onDialogHeaderPointerDown">
        <span>设置</span>
        <button class="modal-close" title="关闭" @click="emit('close')">×</button>
      </div>
      <div class="modal-body">
        <div class="set-grid">
          <label>终端字号</label>
          <input class="input" type="number" min="8" max="32" v-model.number="form.fontSize" />

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
.switch {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--text-primary);
  cursor: pointer;
}
</style>
