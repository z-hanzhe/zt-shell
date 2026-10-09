<script setup lang="ts" generic="T extends string | number">
/** 与应用主题一致的单选下拉框，浮层避开滚动容器裁切并保留键盘操作。 */
import { computed, nextTick, ref, watch, type CSSProperties } from "vue";
import Icon from "./Icon.vue";

const props = withDefaults(defineProps<{
  id: string;
  modelValue: T;
  options: ReadonlyArray<{ value: T; label: string }>;
  labelledby?: string;
  disabled?: boolean;
}>(), { disabled: false });
const emit = defineEmits<{ (event: "update:modelValue", value: T): void }>();
const trigger = ref<HTMLButtonElement | null>(null);
const menu = ref<HTMLElement | null>(null);
const opened = ref(false);
const activeIndex = ref(0);
const menuStyle = ref<CSSProperties>({});
const selectedLabel = computed(() => props.options.find((option) => option.value === props.modelValue)?.label ?? "请选择");

/** 根据可用空间展开浮层，控件离开可见区域时收起。 */
function positionMenu(): void {
  if (!trigger.value || !opened.value) return;
  const bounds = trigger.value.getBoundingClientRect();
  // 浏览器预览使用根节点 CSS 缩放，浮层坐标需换算回同一布局单位。
  const zoom = Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue("zoom")) || 1;
  const rect = {
    top: bounds.top / zoom,
    bottom: bounds.bottom / zoom,
    right: bounds.right / zoom,
    width: bounds.width / zoom,
  };
  const viewportWidth = document.documentElement.clientWidth / zoom;
  const viewportHeight = document.documentElement.clientHeight / zoom;
  if (!trigger.value.getClientRects().length || rect.bottom <= 0 || rect.top >= viewportHeight) {
    closeMenu();
    return;
  }
  const width = Math.min(rect.width, viewportWidth - 16);
  const below = viewportHeight - rect.bottom - 14;
  const above = rect.top - 14;
  const upwards = below < Math.min(props.options.length * 36 + 12, 248) && above > below;
  menuStyle.value = {
    width: `${width}px`,
    left: `${Math.max(8, Math.min(rect.right - width, viewportWidth - width - 8))}px`,
    top: upwards ? undefined : `${rect.bottom + 6}px`,
    bottom: upwards ? `${viewportHeight - rect.top + 6}px` : undefined,
    maxHeight: `${Math.max(0, Math.min(248, upwards ? above : below))}px`,
  };
}

/** 打开时定位到当前值，方向键仅移动候选项。 */
function openMenu(index = props.options.findIndex((option) => option.value === props.modelValue)): void {
  if (props.disabled || !props.options.length) return;
  activeIndex.value = Math.max(0, index);
  opened.value = true;
  positionMenu();
}

/** 收起浮层，确认或取消键盘选择时保留控件焦点。 */
function closeMenu(restoreFocus = false): void {
  opened.value = false;
  if (restoreFocus) trigger.value?.focus({ preventScroll: true });
}

/** 提交候选值，由调用方决定直接更新还是等待保存成功。 */
function selectOption(value: T): void {
  if (props.disabled) return;
  closeMenu(true);
  if (value !== props.modelValue) emit("update:modelValue", value);
}

/** 点击或焦点移到控件之外时收起，不干扰外部交互。 */
function onOutsideInteraction(event: Event): void {
  const target = event.target;
  if (!(target instanceof Node) || trigger.value?.contains(target) || menu.value?.contains(target)) return;
  closeMenu();
}

/** 方向键切换候选项，回车确认，Escape 和 Tab 取消展开。 */
function onKeydown(event: KeyboardEvent): void {
  if (props.disabled) return;
  if (event.key === "Tab") {
    closeMenu();
    return;
  }
  if (event.key === "Escape") {
    if (opened.value) {
      event.preventDefault();
      event.stopPropagation();
      closeMenu(true);
    }
    return;
  }
  if (!["ArrowDown", "ArrowUp", "Home", "End", "Enter", " "].includes(event.key)) return;
  event.preventDefault();
  event.stopPropagation();
  if (!opened.value) {
    openMenu(event.key === "Home" ? 0 : event.key === "End" ? props.options.length - 1 : undefined);
    return;
  }
  if (event.key === "Enter" || event.key === " ") {
    const option = props.options[activeIndex.value];
    if (option) selectOption(option.value);
  } else if (event.key === "Home") {
    activeIndex.value = 0;
  } else if (event.key === "End") {
    activeIndex.value = props.options.length - 1;
  } else {
    activeIndex.value = (activeIndex.value + (event.key === "ArrowDown" ? 1 : -1) + props.options.length) % props.options.length;
  }
}

watch(() => props.disabled, (disabled) => { if (disabled) closeMenu(); });
watch(activeIndex, async () => {
  await nextTick();
  menu.value?.querySelector<HTMLElement>(".is-active")?.scrollIntoView({ block: "nearest" });
});
watch(opened, (open, _previous, onCleanup) => {
  if (!open) return;
  void nextTick(() => {
    menu.value?.querySelector<HTMLElement>(".is-active")?.scrollIntoView({ block: "nearest" });
  });
  /** 窗口失去焦点后收起临时浮层。 */
  const closeOnBlur = () => closeMenu();
  const observer = new ResizeObserver(positionMenu);
  const visibility = new IntersectionObserver(([entry]) => {
    if (!entry?.isIntersecting) closeMenu();
  });
  if (trigger.value) {
    observer.observe(trigger.value);
    visibility.observe(trigger.value);
  }
  document.addEventListener("pointerdown", onOutsideInteraction, true);
  document.addEventListener("focusin", onOutsideInteraction, true);
  window.addEventListener("scroll", positionMenu, true);
  window.addEventListener("resize", positionMenu);
  window.addEventListener("blur", closeOnBlur);
  onCleanup(() => {
    observer.disconnect();
    visibility.disconnect();
    document.removeEventListener("pointerdown", onOutsideInteraction, true);
    document.removeEventListener("focusin", onOutsideInteraction, true);
    window.removeEventListener("scroll", positionMenu, true);
    window.removeEventListener("resize", positionMenu);
    window.removeEventListener("blur", closeOnBlur);
  });
});
</script>

<template>
  <div class="app-select">
    <button
      :id="id"
      ref="trigger"
      class="app-select-trigger"
      :class="{ 'is-open': opened }"
      type="button"
      role="combobox"
      aria-haspopup="listbox"
      :aria-labelledby="labelledby"
      :aria-expanded="opened"
      :aria-controls="opened ? `${id}-listbox` : undefined"
      :aria-activedescendant="opened ? `${id}-option-${activeIndex}` : undefined"
      :disabled="disabled"
      @click="opened ? closeMenu() : openMenu()"
      @keydown="onKeydown"
    >
      <span class="app-select-value">{{ selectedLabel }}</span>
      <Icon class="app-select-chevron" name="chevronRight" :size="14" aria-hidden="true" />
    </button>
    <Teleport to="body">
      <div
        v-if="opened"
        :id="`${id}-listbox`"
        ref="menu"
        class="app-select-menu"
        role="listbox"
        :aria-labelledby="labelledby"
        :style="menuStyle"
      >
        <button
          v-for="(option, index) in options"
          :id="`${id}-option-${index}`"
          :key="option.value"
          class="app-select-option"
          :class="{ 'is-selected': option.value === modelValue, 'is-active': index === activeIndex }"
          type="button"
          role="option"
          :aria-selected="option.value === modelValue"
          tabindex="-1"
          @pointermove="activeIndex = index"
          @pointerdown.prevent
          @click="selectOption(option.value)"
        >
          <span>{{ option.label }}</span>
          <Icon v-if="option.value === modelValue" name="check" :size="14" aria-hidden="true" />
        </button>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.app-select { min-width: 0; }
.app-select-trigger {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  width: 100%;
  height: 34px;
  padding: 0 11px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg-window);
  color: var(--text);
  font: inherit;
  text-align: left;
  cursor: pointer;
  transition: border-color .15s, background .15s, box-shadow .15s;
}
.app-select-trigger:hover:not(:disabled) { border-color: var(--accent); background: var(--bg-panel); }
.app-select-trigger.is-open { border-color: var(--accent); box-shadow: 0 0 0 3px var(--row-hover); }
.app-select-trigger:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.app-select-trigger:disabled { opacity: .5; cursor: not-allowed; }
.app-select-value { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.app-select-chevron { color: var(--text-muted); transform: rotate(90deg); transition: transform .15s; }
.is-open .app-select-chevron { transform: rotate(270deg); }
.app-select-menu {
  position: fixed;
  z-index: 900;
  overflow-y: auto;
  padding: 5px;
  border: 1px solid var(--border-light);
  border-radius: 8px;
  background: var(--bg-window);
  box-shadow: 0 8px 24px #2030401a, 0 2px 6px #2030400d;
}
.app-select-option {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  width: 100%;
  min-height: 36px;
  padding: 8px 10px;
  border: 0;
  border-radius: 5px;
  background: transparent;
  color: var(--text-secondary);
  font: inherit;
  text-align: left;
  cursor: pointer;
}
.app-select-option span { min-width: 0; overflow-wrap: anywhere; }
.app-select-option.is-selected { color: var(--accent); font-weight: 600; }
.app-select-option.is-active { background: var(--row-hover); color: var(--accent); }
.app-select-option:active { background: var(--bg-active); }
</style>
