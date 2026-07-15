<script setup lang="ts">
/**
 * 应用根组件：自绘标题栏 + 三区固定尺寸布局 + 底部状态栏
 *
 * 布局采用手写 flex + 自定义拖拽分隔条：
 * - 左侧监控面板固定像素宽度，可拖拽调整
 * - 右下文件区固定像素高度，可拖拽调整
 * - 窗口缩放时仅右上终端区自适应，左宽与底高保持不变（满足需求）
 */
import { computed, onMounted, onBeforeUnmount, reactive, ref } from "vue";

import TitleBar from "./components/TitleBar.vue";
import MonitorPanel from "./components/MonitorPanel.vue";
import TerminalPanel from "./components/TerminalPanel.vue";
import BottomPanel from "./components/BottomPanel.vue";
import ConnectionManager from "./components/ConnectionManager.vue";
import SettingsDialog from "./components/SettingsDialog.vue";

import { useConnectionsStore } from "./stores/connections";
import { useSessionsStore } from "./stores/sessions";
import { useSettingsStore } from "./stores/settings";
import { useTransfersStore } from "./stores/transfers";
import type { ConnectionConfig } from "./types";
import type { AppSettings } from "./stores/settings";

const connectionsStore = useConnectionsStore();
const sessionsStore = useSessionsStore();
const settingsStore = useSettingsStore();
const transfersStore = useTransfersStore();

/** 连接管理器弹窗可见性 */
const showConnManager = ref(false);
/** 设置弹窗可见性 */
const showSettings = ref(false);
/** 终端面板引用，用于文件区与终端互相同步路径 */
const terminalPanelRef = ref<InstanceType<typeof TerminalPanel>>();
/** 底部面板引用，用于根据终端路径更新文件管理器 */
const bottomPanelRef = ref<InstanceType<typeof BottomPanel>>();

/** 当前激活会话（用于状态栏与子面板） */
const active = computed(() => sessionsStore.activeSession);
const activeConnected = computed(() => active.value?.status === "connected");

/** 面板尺寸（像素），左宽与底高固定，窗口缩放不改变 */
const layout = reactive({ leftWidth: 258, bottomHeight: 300 });

/** 各面板尺寸约束（像素） */
const LIMITS = { leftMin: 240, leftMax: 520, bottomMin: 120, bottomMax: 700 };

/** 当前拖拽状态 */
let dragging: "left" | "bottom" | null = null;
let startPos = 0;
let startSize = 0;

const blockedBrowserKeys = new Set(["F3", "F5", "F7"]);

/** 开始拖拽左侧分隔条 */
function startDragLeft(e: MouseEvent) {
  dragging = "left";
  startPos = e.clientX;
  startSize = layout.leftWidth;
  attachDragListeners();
}

/** 开始拖拽底部分隔条 */
function startDragBottom(e: MouseEvent) {
  dragging = "bottom";
  startPos = e.clientY;
  startSize = layout.bottomHeight;
  attachDragListeners();
}

/** 拖拽过程中更新尺寸 */
function onDragMove(e: MouseEvent) {
  if (dragging === "left") {
    const next = startSize + (e.clientX - startPos);
    layout.leftWidth = Math.min(Math.max(next, LIMITS.leftMin), LIMITS.leftMax);
  } else if (dragging === "bottom") {
    // 底部分隔条向上拖动增高，故取反向
    const next = startSize - (e.clientY - startPos);
    layout.bottomHeight = Math.min(
      Math.max(next, LIMITS.bottomMin),
      LIMITS.bottomMax
    );
  }
}

/** 结束拖拽 */
function endDrag() {
  dragging = null;
  document.body.style.cursor = "";
  document.removeEventListener("mousemove", onDragMove);
  document.removeEventListener("mouseup", endDrag);
}

/** 绑定全局拖拽监听 */
function attachDragListeners() {
  document.body.style.cursor = dragging === "left" ? "col-resize" : "row-resize";
  document.addEventListener("mousemove", onDragMove);
  document.addEventListener("mouseup", endDrag);
}

/** 阻止 WebView 中浏览器自带快捷键，避免覆盖桌面端交互 */
function preventBrowserShortcut(e: KeyboardEvent) {
  const key = e.key.toLowerCase();
  const ctrlOrMeta = e.ctrlKey || e.metaKey;
  const target = e.target as HTMLElement;
  // Monaco 编辑器内部放行查找/替换/跳转行快捷键，交由编辑器处理
  if (
    target?.closest?.(".monaco-editor") &&
    (e.key === "F3" || (ctrlOrMeta && ["f", "h", "g"].includes(key)))
  ) {
    return;
  }
  // 终端内部放行：Ctrl+Shift+C/V/F/A 由终端快捷键处理，其余 Ctrl 组合与 F3/F5/F7
  // 转义后发往远端（vim 翻页、shell 历史搜索等），xterm 会阻止浏览器默认行为
  if (target?.closest?.(".terminal-wrap") && (ctrlOrMeta || blockedBrowserKeys.has(e.key))) {
    return;
  }
  if (
    blockedBrowserKeys.has(e.key) ||
    (ctrlOrMeta && ["f", "g", "r", "p", "s", "u"].includes(key)) ||
    (e.ctrlKey && e.shiftKey && ["i", "j", "c", "r"].includes(key))
  ) {
    e.preventDefault();
    e.stopPropagation();
  }
}

/** 阻止浏览器原生右键菜单 */
function preventNativeContextMenu(e: MouseEvent) {
  e.preventDefault();
}

/** 绑定全局浏览器默认行为拦截 */
function attachBrowserGuards() {
  window.addEventListener("keydown", preventBrowserShortcut, true);
  window.addEventListener("contextmenu", preventNativeContextMenu, true);
}

/** 解绑全局浏览器默认行为拦截 */
function detachBrowserGuards() {
  window.removeEventListener("keydown", preventBrowserShortcut, true);
  window.removeEventListener("contextmenu", preventNativeContextMenu, true);
}

/** 打开连接管理器发起的连接 */
function onConnect(config: ConnectionConfig) {
  sessionsStore.open(config);
}

/** 保存设置 */
async function onSaveSettings(settings: AppSettings) {
  await settingsStore.update(settings);
}

/** 将文件管理器地址栏路径同步到当前终端 */
function syncTerminalPath(path: string) {
  terminalPanelRef.value?.cdActiveTerminal(path)?.catch((e: unknown) => {
    console.warn("同步路径到终端失败", e);
  });
}

/** 将当前终端路径同步到文件管理器地址栏 */
async function syncFilePath() {
  try {
    const path = await terminalPanelRef.value?.requestActiveTerminalCwd();
    if (path) await bottomPanelRef.value?.setFilePath(path);
  } catch (e) {
    console.warn("同步终端路径到文件管理器失败", e);
  }
}

onMounted(async () => {
  attachBrowserGuards();
  // 加载本地持久化的连接与设置、初始化传输事件监听（浏览器预览环境下会失败，忽略即可）
  try {
    await Promise.all([connectionsStore.init(), settingsStore.init(), transfersStore.init()]);
  } catch (e) {
    console.warn("本地存储不可用（可能非 Tauri 环境）", e);
  }
});

onBeforeUnmount(() => {
  endDrag();
  detachBrowserGuards();
});
</script>

<template>
  <div class="app-root">
    <!-- 顶部自绘标题栏 -->
    <TitleBar />

    <!-- 主体：左固定宽 + 右自适应 -->
    <div class="app-body">
      <!-- 左侧监控面板（固定宽） -->
      <div class="left-pane" :style="{ width: layout.leftWidth + 'px' }">
        <MonitorPanel
          :session-id="active?.id ?? ''"
          :connected="activeConnected"
          :config="active?.config"
        />
      </div>

      <!-- 左右分隔条 -->
      <div class="splitter splitter-v" @mousedown.prevent="startDragLeft"></div>

      <!-- 右侧：上终端（自适应） + 下文件区（固定高） -->
      <div class="right-pane">
        <div class="terminal-region">
          <TerminalPanel
            ref="terminalPanelRef"
            @open-conn-manager="showConnManager = true"
            @open-settings="showSettings = true"
          />
        </div>

        <!-- 上下分隔条 -->
        <div class="splitter splitter-h" @mousedown.prevent="startDragBottom"></div>

        <!-- 底部文件区（固定高） -->
        <div class="bottom-region" :style="{ height: layout.bottomHeight + 'px' }">
          <BottomPanel
            ref="bottomPanelRef"
            :session-id="active?.id ?? ''"
            :connected="activeConnected"
            @sync-terminal-path="syncTerminalPath"
            @sync-file-path="syncFilePath"
          />
        </div>
      </div>
    </div>

    <!-- 底部状态栏 -->
    <div class="statusbar">
      <span>就绪</span>
      <span v-if="active">
        连接：{{ active.config.name }} ({{ active.config.host }}:{{
          active.config.port
        }})
      </span>
      <span v-else>未连接</span>
      <span>UTF-8</span>
      <span class="status-right">
        {{ activeConnected ? "SFTP 已连接" : "SFTP 未连接" }}
      </span>
    </div>

    <!-- 连接管理器 -->
    <ConnectionManager
      v-if="showConnManager"
      @connect="onConnect"
      @close="showConnManager = false"
    />

    <!-- 设置 -->
    <SettingsDialog
      v-if="showSettings"
      :settings="settingsStore.settings"
      @save="onSaveSettings"
      @close="showSettings = false"
    />
  </div>
</template>

<style scoped>
.app-root {
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  background: var(--bg-window);
  overflow: hidden;
}

/* 主体：横向布局 */
.app-body {
  flex: 1 1 auto;
  min-height: 0;
  display: flex;
  overflow: hidden;
}

/* 左侧面板固定宽度 */
.left-pane {
  flex: 0 0 auto;
  min-width: 0;
  overflow: hidden;
}

/* 右侧面板占据剩余空间，纵向布局 */
.right-pane {
  flex: 1 1 auto;
  min-width: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

/* 终端区自适应剩余高度 */
.terminal-region {
  flex: 1 1 auto;
  min-height: 0;
  overflow: hidden;
}

/* 底部文件区固定高度 */
.bottom-region {
  flex: 0 0 auto;
  overflow: hidden;
}

/* 分隔条 */
.splitter {
  background: var(--border);
  flex: 0 0 auto;
  transition: background 0.15s;
}
.splitter:hover {
  background: var(--accent);
}
.splitter-v {
  width: 3px;
  cursor: col-resize;
}
.splitter-h {
  height: 3px;
  cursor: row-resize;
}

/* 状态栏 */
.statusbar {
  flex: 0 0 auto;
  height: 22px;
  display: flex;
  align-items: center;
  padding: 0 12px;
  gap: 18px;
  background: var(--bg-panel-alt);
  border-top: 1px solid var(--border);
  font-size: 11px;
  color: var(--text-muted);
}
.statusbar .status-right {
  margin-left: auto;
}
</style>
