<script setup lang="ts">
/**
 * 设置弹窗：终端与界面相关的基础设置
 */
import { reactive, watch } from "vue";
import Icon from "./Icon.vue";
import type { AppSettings } from "../stores/settings";

const props = defineProps<{
  /** 当前设置 */
  settings: AppSettings;
}>();

const emit = defineEmits<{
  (e: "save", settings: AppSettings): void;
  (e: "close"): void;
}>();

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
</script>

<template>
  <div class="modal-mask" @mousedown.self="emit('close')">
    <div class="modal" style="width: 420px">
      <div class="modal-header">
        <span>设置</span>
        <button class="btn btn-icon" @click="emit('close')">
          <Icon name="close" :size="15" />
        </button>
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
