<script setup lang="ts">
/**
 * 自绘标题栏：应用图标 + 标题 + 拖拽区 + 最小化/最大化/关闭窗口按钮
 * 已在 tauri.conf.json 关闭系统原生标题栏（decorations:false），此处统一风格
 *
 * 拖拽与双击最大化交给 Tauri 原生 data-tauri-drag-region 属性处理：
 * 相比手动 mousedown+startDragging，原生属性不会吞掉双击事件，双击标题栏可切换最大化/还原
 */
import { onMounted, onBeforeUnmount, ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";
import Icon from "./Icon.vue";

/** 当前窗口是否最大化 */
const maximized = ref(false);
/** 当前窗口句柄（非 Tauri 环境下为 null） */
let appWindow: ReturnType<typeof getCurrentWindow> | null = null;
let unlisten: UnlistenFn | null = null;

/** 最小化窗口 */
function onMinimize() {
  appWindow?.minimize();
}

/** 最大化 / 还原窗口 */
async function onToggleMaximize() {
  if (!appWindow) return;
  await appWindow.toggleMaximize();
  maximized.value = await appWindow.isMaximized();
}

/** 关闭窗口 */
function onClose() {
  appWindow?.close();
}

onMounted(async () => {
  try {
    appWindow = getCurrentWindow();
    maximized.value = await appWindow.isMaximized();
    // 监听窗口尺寸变化，同步最大化状态（拖拽或双击标题栏触发时）
    unlisten = await appWindow.onResized(async () => {
      maximized.value = (await appWindow?.isMaximized()) ?? false;
    });
  } catch {
    // 非 Tauri 环境（浏览器预览）忽略
    appWindow = null;
  }
});

onBeforeUnmount(() => {
  unlisten?.();
});
</script>

<template>
  <!-- data-tauri-drag-region：整条标题栏可拖拽窗口，双击切换最大化/还原 -->
  <div class="titlebar" data-tauri-drag-region>
    <img class="logo" src="/app-icon.png" alt="logo" draggable="false" />
    <div class="app-title">ZTShell</div>

    <!-- 窗口控制按钮（不含 drag-region，保持可点击） -->
    <div class="win-btns">
      <button class="win-btn" title="最小化" @click="onMinimize">
        <Icon name="winMin" :size="15" />
      </button>
      <button class="win-btn" :title="maximized ? '还原' : '最大化'" @click="onToggleMaximize">
        <Icon :name="maximized ? 'winRestore' : 'winMax'" :size="13" />
      </button>
      <button class="win-btn close" title="关闭" @click="onClose">
        <Icon name="close" :size="15" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.titlebar {
  display: flex;
  align-items: center;
  height: 34px;
  padding: 0 0 0 10px;
  background: var(--bg-window);
  border-bottom: 1px solid var(--border-light);
  flex: 0 0 auto;
  user-select: none;
}
/* logo 与标题不响应指针，使其区域的拖拽事件回落到带 drag-region 的标题栏 */
.logo {
  width: 18px;
  height: 18px;
  margin-right: 8px;
  pointer-events: none;
  -webkit-user-drag: none;
}
.app-title {
  font-size: 12px;
  color: #444;
  flex: 1;
  pointer-events: none;
}
.win-btns {
  display: flex;
  align-items: center;
  height: 100%;
}
.win-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 44px;
  height: 34px;
  border: none;
  background: transparent;
  color: #555;
  cursor: pointer;
  transition: background 0.12s, color 0.12s;
}
.win-btn:hover {
  background: var(--row-hover);
}
.win-btn.close:hover {
  background: #e05555;
  color: #fff;
}
</style>
