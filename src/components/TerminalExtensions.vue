<script setup lang="ts">
/**
 * 终端扩展信息浮层：会话使用了代理或隧道时在终端右侧显示入口按钮
 *
 * 按钮默认位于右上角，可在终端区域内垂直拖拽（禁止左右移动），
 * 点击展开面板列出各条目的使用明细与成功失败状态；
 * 存在失败条目时按钮闪烁提示，可在面板内手动关闭闪烁。
 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import Icon from "./Icon.vue";
import { useEscClose } from "../composables/useEscClose";
import type { ExtensionEntry } from "../types";

const props = defineProps<{
  /** 本次连接的扩展功能条目 */
  entries: ExtensionEntry[];
  /** 按钮相对终端区域可用范围顶部的垂直偏移（像素） */
  offsetY: number;
  /** 是否已手动关闭异常闪烁 */
  blinkMuted: boolean;
}>();

const emit = defineEmits<{
  (e: "update:offsetY", value: number): void;
  (e: "update:blinkMuted", value: boolean): void;
}>();

/** 按钮边长 */
const BUTTON_SIZE = 24;
/** 按钮与终端区域四边的安全间距 */
const EDGE = 8;
/** 判定为拖拽而非点击的位移阈值 */
const DRAG_THRESHOLD = 4;

/** 覆盖整个终端区域的定位容器，同时用于测量可拖拽范围 */
const rootRef = ref<HTMLElement | null>(null);
/** 详情面板，用于测量高度以避免超出终端区域下边界 */
const panelRef = ref<HTMLElement | null>(null);
/** 详情面板是否展开 */
const open = ref(false);
/** 详情面板顶部坐标（相对终端区域） */
const panelTop = ref(EDGE);

/** 拖拽运行时状态，null 表示未按下 */
let drag: { startY: number; startOffset: number; moved: boolean } | null = null;

/** 启动失败的条目数量 */
const failedCount = computed(() => props.entries.filter((entry) => !entry.ok).length);

/** 是否需要闪烁提示异常 */
const blinking = computed(() => failedCount.value > 0 && !props.blinkMuted);

/** 按钮悬浮提示 */
const buttonTitle = computed(() =>
  failedCount.value
    ? `扩展功能 ${props.entries.length} 项，其中 ${failedCount.value} 项未启用`
    : `扩展功能 ${props.entries.length} 项，全部正常`
);

/** 将垂直偏移限制在终端区域内 */
function clampOffset(value: number): number {
  const height = rootRef.value?.clientHeight ?? 0;
  const max = Math.max(0, height - BUTTON_SIZE - EDGE * 2);
  return Math.min(Math.max(value, 0), max);
}

/** 计算详情面板顶部坐标：跟随按钮，超出下边界时上移 */
function updatePanelTop() {
  const height = rootRef.value?.clientHeight ?? 0;
  const panelHeight = panelRef.value?.offsetHeight ?? 0;
  const bottomLimit = Math.max(EDGE, height - panelHeight - EDGE);
  panelTop.value = Math.min(Math.max(EDGE + props.offsetY, EDGE), bottomLimit);
}

/**
 * 按钮按下：进入拖拽准备状态
 * 位移未越过阈值时松手视为点击，用于切换详情面板
 */
function onButtonPointerDown(e: PointerEvent) {
  if (e.button !== 0) return;
  drag = { startY: e.clientY, startOffset: props.offsetY, moved: false };
  window.addEventListener("pointermove", onPointerMove);
  window.addEventListener("pointerup", onPointerUp, { once: true });
}

/** 拖拽移动：仅取垂直位移，水平方向恒定贴靠右侧 */
function onPointerMove(e: PointerEvent) {
  if (!drag) return;
  const dy = e.clientY - drag.startY;
  if (!drag.moved) {
    if (Math.abs(dy) < DRAG_THRESHOLD) return;
    drag.moved = true;
  }
  emit("update:offsetY", clampOffset(drag.startOffset + dy));
}

/** 松手：拖拽结束或按点击处理 */
function onPointerUp() {
  window.removeEventListener("pointermove", onPointerMove);
  const clicked = !drag?.moved;
  drag = null;
  if (clicked) open.value = !open.value;
}

/** 切换闪烁提示开关 */
function toggleBlinkMuted() {
  emit("update:blinkMuted", !props.blinkMuted);
}

/** 点击浮层以外区域关闭详情面板 */
function onGlobalPointerDown(e: PointerEvent) {
  if (!open.value) return;
  if (rootRef.value?.contains(e.target as Node)) return;
  open.value = false;
}

/** 条目类别的中文名称 */
function kindLabel(entry: ExtensionEntry): string {
  return entry.kind === "proxy" ? "代理" : "隧道";
}

// 终端区域尺寸变化后重新约束按钮位置与面板坐标
let resizeObserver: ResizeObserver | null = null;

// 面板展开或按钮位置变化后重新计算面板坐标
watch([open, () => props.offsetY], () => {
  if (open.value) nextTick(updatePanelTop);
});

useEscClose(open, () => (open.value = false));

onMounted(() => {
  window.addEventListener("pointerdown", onGlobalPointerDown);
  if (rootRef.value) {
    resizeObserver = new ResizeObserver(() => {
      const clamped = clampOffset(props.offsetY);
      if (clamped !== props.offsetY) emit("update:offsetY", clamped);
      if (open.value) updatePanelTop();
    });
    resizeObserver.observe(rootRef.value);
  }
});

onBeforeUnmount(() => {
  window.removeEventListener("pointerdown", onGlobalPointerDown);
  window.removeEventListener("pointermove", onPointerMove);
  resizeObserver?.disconnect();
});
</script>

<template>
  <div ref="rootRef" class="ext-root">
    <button
      :class="['ext-btn', { failed: failedCount > 0, blinking }]"
      :style="{ top: `${EDGE + offsetY}px` }"
      :title="buttonTitle"
      @pointerdown="onButtonPointerDown"
    >
      <Icon name="network" :size="14" />
    </button>

    <div v-if="open" ref="panelRef" class="ext-panel" :style="{ top: `${panelTop}px` }">
      <div class="ext-head">
        <span class="ext-head-title">扩展信息</span>
        <button
          v-if="failedCount > 0"
          class="ext-mute"
          :title="blinkMuted ? '恢复异常闪烁提示' : '关闭异常闪烁提示'"
          @click="toggleBlinkMuted"
        >
          {{ blinkMuted ? "恢复闪烁" : "关闭闪烁" }}
        </button>
      </div>

      <div class="ext-list">
        <div
          v-for="(entry, index) in entries"
          :key="index"
          :class="['ext-item', { failed: !entry.ok }]"
        >
          <span class="ext-dot"></span>
          <div class="ext-body">
            <div class="ext-title">
              <span class="ext-kind">{{ kindLabel(entry) }}</span>
              <span class="ext-name">{{ entry.name }}</span>
              <span class="ext-category">{{ entry.category }}</span>
            </div>
            <div class="ext-detail">{{ entry.detail }}</div>
            <div v-if="!entry.ok" class="ext-error">{{ entry.error }}</div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 覆盖终端区域的定位层，自身不拦截指针事件，避免影响终端选中与输入 */
.ext-root {
  position: absolute;
  inset: 0;
  z-index: 10;
  pointer-events: none;
}
.ext-btn {
  position: absolute;
  right: 8px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  border: 1px solid rgba(192, 202, 245, 0.35);
  border-radius: 50%;
  background: rgba(36, 40, 59, 0.85);
  color: var(--terminal-text);
  cursor: grab;
  pointer-events: auto;
}
.ext-btn:hover {
  border-color: rgba(192, 202, 245, 0.7);
  background: rgba(52, 58, 82, 0.95);
}
.ext-btn:active {
  cursor: grabbing;
}
/* 存在未启用条目时改用警示配色 */
.ext-btn.failed {
  border-color: rgba(224, 168, 56, 0.8);
  color: var(--warning);
}
.ext-btn.blinking {
  animation: ext-blink 1s ease-in-out infinite;
}
@keyframes ext-blink {
  0%,
  100% {
    opacity: 1;
    box-shadow: 0 0 0 0 rgba(224, 168, 56, 0);
  }
  50% {
    opacity: 0.45;
    box-shadow: 0 0 0 4px rgba(224, 168, 56, 0.25);
  }
}
/* 详情面板：贴按钮左侧展开，随按钮垂直位置浮动 */
.ext-panel {
  position: absolute;
  right: 40px;
  width: 300px;
  max-height: calc(100% - 16px);
  display: flex;
  flex-direction: column;
  border: 1px solid rgba(192, 202, 245, 0.25);
  border-radius: 4px;
  background: rgba(30, 33, 48, 0.98);
  box-shadow: 0 4px 18px rgba(0, 0, 0, 0.45);
  color: var(--terminal-text);
  pointer-events: auto;
}
.ext-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 7px 10px;
  border-bottom: 1px solid rgba(192, 202, 245, 0.16);
  font-size: 12px;
  font-weight: 600;
}
.ext-head-title {
  white-space: nowrap;
}
.ext-mute {
  padding: 2px 8px;
  border: 1px solid rgba(224, 168, 56, 0.55);
  border-radius: 3px;
  background: transparent;
  color: var(--warning);
  font-size: 11px;
  cursor: pointer;
  white-space: nowrap;
}
.ext-mute:hover {
  background: rgba(224, 168, 56, 0.16);
}
.ext-list {
  overflow-y: auto;
  padding: 4px 0;
}
.ext-item {
  display: flex;
  gap: 8px;
  padding: 6px 10px;
}
.ext-item + .ext-item {
  border-top: 1px solid rgba(192, 202, 245, 0.1);
}
/* 状态点：默认成功绿，失败转红 */
.ext-dot {
  flex-shrink: 0;
  width: 7px;
  height: 7px;
  margin-top: 5px;
  border-radius: 50%;
  background: var(--terminal-green);
}
.ext-item.failed .ext-dot {
  background: var(--danger);
}
.ext-body {
  min-width: 0;
  flex: 1;
}
.ext-title {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
}
.ext-kind {
  flex-shrink: 0;
  padding: 0 5px;
  border-radius: 2px;
  background: rgba(192, 202, 245, 0.16);
  font-size: 11px;
}
.ext-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ext-category {
  flex-shrink: 0;
  color: #8b93b8;
  font-size: 11px;
}
.ext-detail {
  margin-top: 2px;
  color: #9aa3c7;
  font-size: 11px;
  word-break: break-all;
}
.ext-error {
  margin-top: 2px;
  color: #f7768e;
  font-size: 11px;
  word-break: break-all;
}
</style>
