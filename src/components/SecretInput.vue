<script setup lang="ts">
/**
 * 固定长度凭据输入框：已有凭据仅显示固定掩码，不接收或展示真实值。
 */
import { computed, ref, watch } from "vue";
import Icon from "./Icon.vue";

type SecretChangeAction = "keep" | "set" | "clear";

const props = withDefaults(
  defineProps<{
    /** 当前编辑对象是否已保存凭据 */
    hasSecret: boolean;
    /** 未保存凭据或进入编辑状态时的占位文案 */
    placeholder?: string;
    /** 上层编辑对象变化时用于重置输入状态的稳定标识 */
    resetKey?: string | number;
  }>(),
  {
    placeholder: "",
    resetKey: "",
  }
);

const emit = defineEmits<{
  (event: "change", action: "keep"): void;
  (event: "change", action: "set", value: string): void;
  (event: "change", action: "clear"): void;
}>();

/** 固定八位掩码，仅表示已有凭据 */
const SECRET_MASK = "********";

/** 输入框当前显示值，掩码不代表真实凭据 */
const inputValue = ref("");
/** 当前凭据变更意图 */
const action = ref<SecretChangeAction>("keep");
/** 是否已经通过清除按钮明确要求删除凭据 */
const explicitlyCleared = ref(false);

/** 当前是否存在可清除的已保存或新输入凭据 */
const canClear = computed(
  () => action.value === "set" || (props.hasSecret && action.value !== "clear")
);

/** 按当前编辑对象重置为保留状态 */
function resetInput() {
  inputValue.value = props.hasSecret ? SECRET_MASK : "";
  action.value = "keep";
  explicitlyCleared.value = false;
  emit("change", "keep");
}

watch(
  [() => props.hasSecret, () => props.resetKey],
  resetInput,
  { immediate: true }
);

/** 聚焦时移除展示掩码，允许直接输入替换值 */
function onFocus() {
  inputValue.value = "";
}

/** 输入新凭据；清空普通输入表示撤销本次修改，显式清除状态除外 */
function onInput(event: Event) {
  const value = (event.target as HTMLInputElement).value;
  inputValue.value = value;
  if (value) {
    action.value = "set";
    emit("change", "set", value);
    return;
  }
  if (explicitlyCleared.value) {
    action.value = "clear";
    emit("change", "clear");
    return;
  }
  action.value = "keep";
  emit("change", "keep");
}

/** 失焦时隐藏新输入长度；没有输入时恢复已有凭据的固定掩码 */
function onBlur() {
  if (action.value === "set" || (action.value === "keep" && props.hasSecret)) {
    inputValue.value = SECRET_MASK;
    return;
  }
  inputValue.value = "";
}

/** 明确清除当前凭据 */
function clearSecret() {
  inputValue.value = "";
  action.value = "clear";
  explicitlyCleared.value = true;
  emit("change", "clear");
}
</script>

<template>
  <div class="secret-input-field">
    <input
      class="input secret-input"
      type="password"
      :value="inputValue"
      :placeholder="placeholder"
      :aria-label="placeholder || '凭据'"
      autocomplete="new-password"
      @focus="onFocus"
      @input="onInput"
      @blur="onBlur"
    />
    <button
      class="secret-clear"
      :class="{ hidden: !canClear }"
      type="button"
      title="清除凭据"
      aria-label="清除凭据"
      :aria-hidden="!canClear"
      :tabindex="canClear ? 0 : -1"
      :disabled="!canClear"
      @mousedown.prevent
      @click="clearSecret"
    >
      <Icon name="close" :size="13" />
    </button>
  </div>
</template>

<style scoped>
.secret-input-field {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 30px;
  gap: 6px;
  width: 100%;
  min-width: 0;
}
.secret-input {
  width: 100%;
  min-width: 0;
}
.secret-clear {
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
.secret-clear:hover:not(:disabled) {
  border-color: var(--danger);
  background: #fff0f0;
  color: var(--danger);
}
.secret-clear.hidden {
  visibility: hidden;
  pointer-events: none;
}
</style>
