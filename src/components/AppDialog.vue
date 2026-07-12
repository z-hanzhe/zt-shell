<script setup lang="ts">
/**
 * 通用轻量弹窗：支持提示、确认与单输入，避免使用浏览器内置弹窗行为
 */
import { computed, nextTick, ref, watch } from "vue";

const props = withDefaults(
  defineProps<{
    /** 是否显示弹窗 */
    open: boolean;
    /** 弹窗标题 */
    title: string;
    /** 主体提示文案 */
    message?: string;
    /** 弹窗类型 */
    type?: "info" | "confirm" | "prompt";
    /** 输入框默认值 */
    defaultValue?: string;
    /** 输入框占位文案 */
    placeholder?: string;
    /** 确认按钮文案 */
    confirmText?: string;
    /** 取消按钮文案 */
    cancelText?: string;
    /** 输入提示模板，使用 {value} 表示当前输入值 */
    hintTemplate?: string;
  }>(),
  {
    message: "",
    type: "info",
    defaultValue: "",
    placeholder: "",
    confirmText: "确定",
    cancelText: "取消",
    hintTemplate: "",
  }
);

const emit = defineEmits<{
  (e: "confirm", value: string): void;
  (e: "cancel"): void;
}>();

const inputValue = ref("");
const inputRef = ref<HTMLInputElement | null>(null);

/** 根据当前输入内容生成提示文案 */
const hintText = computed(() => {
  if (!props.hintTemplate) return "";
  return props.hintTemplate.replace("{value}", inputValue.value || props.placeholder);
});

watch(
  () => props.open,
  async (open) => {
    if (!open) return;
    inputValue.value = props.defaultValue;
    await nextTick();
    if (props.type === "prompt") inputRef.value?.focus();
  }
);

/** 确认当前弹窗 */
function submit() {
  emit("confirm", inputValue.value);
}
</script>

<template>
  <div v-if="open" class="modal-mask" @mousedown.self="emit('cancel')" @keydown.esc="emit('cancel')">
    <div class="modal app-dialog" role="dialog" aria-modal="true">
      <div class="modal-header">
        <span>{{ title }}</span>
      </div>
      <div class="modal-body app-dialog-body">
        <div v-if="message" class="app-dialog-message">{{ message }}</div>
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
      <div class="modal-footer">
        <button v-if="type !== 'info'" class="btn" @click="emit('cancel')">{{ cancelText }}</button>
        <button class="btn btn-primary" @click="submit">{{ confirmText }}</button>
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
</style>
