<script setup lang="ts">
/**
 * 应用根组件：自绘标题栏 + 三区固定尺寸布局 + 底部状态栏
 *
 * 布局采用手写 flex + 自定义拖拽分隔条：
 * - 左侧监控面板固定像素宽度，可拖拽调整
 * - 右下文件区固定像素高度，可拖拽调整
 * - 窗口缩放时仅右上终端区自适应，左宽与底高保持不变（满足需求）
 */
import { computed, nextTick, onMounted, onBeforeUnmount, reactive, ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { isTauri } from "@tauri-apps/api/core";

import TitleBar from "./components/TitleBar.vue";
import MonitorPanel from "./components/MonitorPanel.vue";
import TerminalPanel from "./components/TerminalPanel.vue";
import BottomPanel from "./components/BottomPanel.vue";
import ConnectionManager from "./components/ConnectionManager.vue";
import AppDialog from "./components/AppDialog.vue";
import HostKeyDialog from "./components/HostKeyDialog.vue";
import Icon from "./components/Icon.vue";

import { useConnectionsStore } from "./stores/connections";
import { useProxiesStore } from "./stores/proxies";
import { useSessionsStore } from "./stores/sessions";
import { useSettingsStore } from "./stores/settings";
import { useTransfersStore } from "./stores/transfers";
import { useWorkspacesStore } from "./stores/workspaces";
import { useUpdatesStore } from "./stores/updates";
import type { ConnectionConfig } from "./types";
import type { AppSettings } from "./stores/settings";
import {
  closeAllTextEditorWindows,
  syncTextEditorFontSize,
  syncTextEditorUiScale,
} from "./editorWindows";
import { isLiveSessionStatus } from "./sessionClose";
import { applyUiScale } from "./uiScale";

const connectionsStore = useConnectionsStore();
const proxiesStore = useProxiesStore();
const sessionsStore = useSessionsStore();
const settingsStore = useSettingsStore();
const transfersStore = useTransfersStore();
const workspacesStore = useWorkspacesStore();
const updatesStore = useUpdatesStore();

/** 连接管理器弹窗可见性 */
const showConnManager = ref(false);
/** 连接、代理和系统凭据是否已完成初始化 */
const connectionDataReady = ref(false);
/** 初始化期间是否收到打开连接管理器的请求 */
const pendingConnectionManagerOpen = ref(false);
/** 安装更新前的退出确认 */
const showInstallConfirm = ref(false);
/** 关闭软件前的确认弹窗可见性（存在连接中的会话时） */
const showCloseConfirm = ref(false);
/** 打开退出确认时仍处于连接中的会话数量快照 */
const closeConfirmSessionCount = ref(0);
/** 本地凭据或持久化初始化错误 */
const storageError = ref("");
/** 主机密钥确认后的二次握手是否正在进行 */
const hostKeyBusy = ref(false);
/** 终端面板引用，用于文件区与终端互相同步路径 */
const terminalPanelRef = ref<InstanceType<typeof TerminalPanel>>();
/** 底部面板引用，用于根据终端路径更新文件管理器 */
const bottomPanelRef = ref<InstanceType<typeof BottomPanel>>();

/** 当前激活会话（用于状态栏与子面板） */
const active = computed(() => sessionsStore.activeSession);
const activeConnected = computed(() => active.value?.status === "connected");
/** 面板可见性独立于会话功能开关，收起时保留内部状态 */
const panels = reactive({ left: true, bottom: true, right: false });
/** 连接工具页独占右侧纵向空间，会话页和欢迎页保留底部文件与传输区域 */
const showBottomRegion = computed(
  () => panels.bottom && (!workspacesStore.activeTab || workspacesStore.activeTab.type === "session")
);
/** 当前最早等待确认主机密钥的会话，多个连接按会话顺序逐个处理 */
const pendingHostKeySession = computed(() =>
  sessionsStore.sessions.find((session) => session.hostKeyChallenge)
);

/** 面板尺寸（像素），左宽与底高固定，窗口缩放不改变 */
const layout = reactive({ leftWidth: 258, bottomHeight: 300, rightWidth: 258 });

/** 各面板尺寸约束（像素） */
const LIMITS = { leftMin: 240, leftMax: 520, bottomMin: 120, bottomMax: 700, rightMin: 180, rightMax: 520 };

/** 当前拖拽状态 */
let dragging: "left" | "bottom" | "right" | null = null;
let startPos = 0;
let startSize = 0;

const blockedBrowserKeys = new Set(["F3", "F5", "F7"]);

/** 切换面板；在工具页展开底部区域时先回到其来源会话 */
function togglePanel(panel: "left" | "bottom" | "right") {
  if (panel === "bottom") {
    panels.bottom = !showBottomRegion.value;
    if (panels.bottom && active.value) sessionsStore.activate(active.value.id);
  } else {
    panels[panel] = !panels[panel];
  }
  terminalPanelRef.value?.focusActiveWorkspace();
}

/** 开始拖拽右侧分隔条 */
function startDragRight(e: MouseEvent) {
  dragging = "right";
  startPos = e.clientX;
  startSize = ((e.currentTarget as HTMLElement).nextElementSibling as HTMLElement).offsetWidth;
  attachDragListeners();
}

/** 开始拖拽左侧分隔条 */
function startDragLeft(e: MouseEvent) {
  dragging = "left";
  startPos = e.clientX;
  startSize = ((e.currentTarget as HTMLElement).previousElementSibling as HTMLElement).offsetWidth;
  attachDragListeners();
}

/** 开始拖拽底部分隔条 */
function startDragBottom(e: MouseEvent) {
  dragging = "bottom";
  startPos = e.clientY;
  startSize = ((e.currentTarget as HTMLElement).nextElementSibling as HTMLElement).offsetHeight;
  attachDragListeners();
}

/** 拖拽过程中更新尺寸 */
function onDragMove(e: MouseEvent) {
  if (dragging === "left") {
    const next = startSize + (e.clientX - startPos);
    layout.leftWidth = Math.min(Math.max(next, LIMITS.leftMin), LIMITS.leftMax);
  } else if (dragging === "right") {
    const next = startSize - (e.clientX - startPos);
    layout.rightWidth = Math.min(Math.max(next, LIMITS.rightMin), LIMITS.rightMax);
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
  document.body.style.cursor = dragging === "bottom" ? "row-resize" : "col-resize";
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
  // 终端内部放行：Ctrl+Shift+C/V/F/A、Alt+Insert 由终端处理，其余 Ctrl 组合与 F3/F5/F7
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
  if (checkingAppClose) return;
  sessionsStore.open(config);
}

/** 按连接配置复用系统信息选项卡，并切换到指定来源会话 */
function openSystemInfo(sessionId: string) {
  const session = sessionsStore.sessions.find((item) => item.id === sessionId);
  if (!session) return;
  sessionsStore.setActiveContext(sessionId);
  workspacesStore.openSystemInfo(
    sessionId,
    session.config.id,
    session.config.name || session.name
  );
  terminalPanelRef.value?.focusActiveWorkspace();
}

/** 按连接配置复用进程列表选项卡，并切换到指定来源会话 */
function openProcessList(sessionId: string) {
  const session = sessionsStore.sessions.find((item) => item.id === sessionId);
  if (!session) return;
  sessionsStore.setActiveContext(sessionId);
  workspacesStore.openProcessList(
    sessionId,
    session.config.id,
    session.config.name || session.name
  );
  terminalPanelRef.value?.focusActiveWorkspace();
}

/** 在连接数据就绪后打开管理器，启动期间的点击会延迟执行 */
function openConnectionManager() {
  if (!connectionDataReady.value) {
    pendingConnectionManagerOpen.value = true;
    return;
  }
  showConnManager.value = true;
}

/** 标记连接数据可用并执行启动期间排队的打开请求 */
function markConnectionDataReady() {
  connectionDataReady.value = true;
  if (!pendingConnectionManagerOpen.value) return;
  pendingConnectionManagerOpen.value = false;
  showConnManager.value = true;
}

/** 完成主机密钥确认；persist 为 false 时仅信任当前连接 */
async function onApproveHostKey(persist = true) {
  const session = pendingHostKeySession.value;
  if (!session || hostKeyBusy.value) return;
  hostKeyBusy.value = true;
  try {
    const reopenInPlace = await sessionsStore.approveHostKey(session.id, persist);
    if (reopenInPlace) {
      try {
        await nextTick();
        await terminalPanelRef.value?.reopenSession(session.id);
      } finally {
        sessionsStore.finishReconnect(session.id);
      }
    }
  } finally {
    hostKeyBusy.value = false;
  }
}

/** 仅本次信任未知服务器，不保存主机密钥记录 */
async function onTrustHostKeyOnce() {
  await onApproveHostKey(false);
}

/** 取消当前主机密钥确认并终止对应连接 */
function onRejectHostKey() {
  const session = pendingHostKeySession.value;
  if (!session || hostKeyBusy.value) return;
  sessionsStore.rejectHostKey(session.id);
}

/** 界面缩放串行队列，避免实时预览的异步调用发生乱序 */
let uiScaleUpdateQueue: Promise<void> = Promise.resolve();

/** 将界面缩放应用到主窗口并同步当前编辑器窗口 */
function applyAppUiScale(uiScale: number): Promise<void> {
  const task = uiScaleUpdateQueue.then(async () => {
    const appliedScale = await applyUiScale(uiScale);
    if (isTauri()) await syncTextEditorUiScale(appliedScale);
  });
  uiScaleUpdateQueue = task.catch((error) => {
    console.warn("应用界面缩放失败", error);
  });
  return uiScaleUpdateQueue;
}

/** 实时预览设置页中选择的界面缩放 */
function onPreviewUiScale(uiScale: number) {
  void applyAppUiScale(uiScale);
}

/** 打开唯一的设置选项卡并转移焦点 */
function openSettings() {
  workspacesStore.openSettings();
  terminalPanelRef.value?.focusActiveWorkspace();
}

/** 应用编辑器基础字号并同步当前独立编辑器窗口 */
async function applyTextEditorFontSize(fontSize: number): Promise<void> {
  if (!isTauri()) return;
  try {
    await syncTextEditorFontSize(fontSize);
  } catch (error) {
    console.warn("应用文本编辑器字号失败", error);
  }
}

/** 保存设置 */
async function onSaveSettings(settings: AppSettings) {
  await settingsStore.update(settings);
  await applyTextEditorFontSize(settingsStore.settings.editorFontSize);
  await applyAppUiScale(settingsStore.settings.uiScale);
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

/** 窗口关闭事件解绑函数 */
let unlistenCloseRequested: UnlistenFn | null = null;
/** 是否正在执行受控窗口销毁，避免重复拦截关闭事件 */
let destroyingWindow = false;
/** 是否正在执行程序退出风险检查，期间禁止创建新会话 */
let checkingAppClose = false;

/** 程序退出失败时恢复窗口状态并释放资源关闭准备 */
async function recoverFailedAppClose(): Promise<void> {
  destroyingWindow = false;
  try {
    await terminalPanelRef.value?.releaseCloseRisks();
  } catch (releaseError) {
    console.warn("释放程序退出关闭准备失败", releaseError);
  }
}

/** 关闭全部编辑窗口后销毁主窗口 */
async function destroyAppWindows() {
  destroyingWindow = true;
  try {
    await closeAllTextEditorWindows();
  } catch (e) {
    console.warn("关闭文本编辑窗口失败", e);
    await recoverFailedAppClose();
    throw e;
  }
  try {
    await getCurrentWindow().destroy();
  } catch (error) {
    await recoverFailedAppClose();
    throw error;
  }
}

/** 确认关闭软件：销毁窗口退出 */
async function onConfirmClose() {
  if (checkingAppClose) return;
  checkingAppClose = true;
  showCloseConfirm.value = false;
  try {
    const targetIds = sessionsStore.sessions.map((session) => session.id);
    const canClose = await terminalPanelRef.value?.confirmCloseRisks(targetIds);
    if (canClose === false) return;
    await destroyAppWindows();
  } catch (e) {
    console.warn("关闭窗口失败", e);
  } finally {
    if (!destroyingWindow) checkingAppClose = false;
  }
}

/** 取消关闭软件 */
function onCancelClose() {
  showCloseConfirm.value = false;
}

/** 请求安装时锁定更新状态，直到用户确认或取消 */
function requestInstallUpdate(): void {
  if (checkingAppClose || updatesStore.busy || updatesStore.phase !== "ready") return;
  updatesStore.preparingInstall = true;
  showInstallConfirm.value = true;
}

/** 取消安装，保留已经下载且验证通过的更新包 */
function cancelInstallUpdate(): void {
  showInstallConfirm.value = false;
  updatesStore.preparingInstall = false;
}

/** 复用应用退出保护，检查通过后才启动官方安装器 */
async function confirmInstallUpdate(): Promise<void> {
  if (checkingAppClose) return;
  checkingAppClose = true;
  showInstallConfirm.value = false;
  try {
    const targetIds = sessionsStore.sessions.map((session) => session.id);
    const canClose = await terminalPanelRef.value?.confirmCloseRisks(targetIds);
    if (canClose !== true) return;
    await closeAllTextEditorWindows();
    destroyingWindow = true;
    await updatesStore.install();
  } catch (error) {
    updatesStore.error = `无法完成更新安装：${String(error)}`;
  } finally {
    if (updatesStore.phase !== "installing") {
      await recoverFailedAppClose();
      checkingAppClose = false;
      updatesStore.preparingInstall = false;
    }
  }
}

onMounted(async () => {
  attachBrowserGuards();
  void updatesStore.init();
  const settingsInitTask = settingsStore.init();
  // 加载本地持久化的连接与设置、初始化传输事件监听（浏览器预览环境下会失败，忽略即可）
  try {
    await Promise.all([
      (async () => {
        // 凭据迁移串行执行，避免多个系统凭据库操作互相争用
        await connectionsStore.init();
        await proxiesStore.init();
        markConnectionDataReady();
      })(),
      settingsInitTask,
      transfersStore.init(),
    ]);
  } catch (e) {
    console.warn("本地存储不可用（可能非 Tauri 环境）", e);
    if (isTauri()) storageError.value = `无法加载本地连接数据：${String(e)}`;
    else markConnectionDataReady();
  }
  await settingsInitTask.catch(() => undefined);
  await applyTextEditorFontSize(settingsStore.settings.editorFontSize);
  await applyAppUiScale(settingsStore.settings.uiScale);
  // 拦截窗口关闭：存在连接中的会话时先二次确认（非 Tauri 环境忽略）
  try {
    unlistenCloseRequested = await getCurrentWindow().onCloseRequested(async (event) => {
      if (destroyingWindow) return;
      event.preventDefault();
      if (checkingAppClose) return;
      if (terminalPanelRef.value?.hasLiveSessions()) {
        closeConfirmSessionCount.value = sessionsStore.sessions.filter((session) =>
          isLiveSessionStatus(session.status)
        ).length;
        showCloseConfirm.value = true;
        return;
      }
      checkingAppClose = true;
      try {
        await destroyAppWindows();
      } catch (e) {
        checkingAppClose = false;
        console.warn("关闭窗口失败", e);
      }
    });
  } catch (e) {
    console.warn("窗口关闭事件监听失败（可能非 Tauri 环境）", e);
  }
});

onBeforeUnmount(() => {
  endDrag();
  detachBrowserGuards();
  unlistenCloseRequested?.();
});
</script>

<template>
  <div class="app-root">
    <!-- 顶部自绘标题栏 -->
    <TitleBar :update-available="updatesStore.hasUpdate" @open-settings="openSettings" />

    <!-- 主体：左固定宽 + 右自适应 -->
    <div class="app-body">
      <!-- 左侧监控面板（固定宽） -->
      <div id="monitor-pane" v-show="panels.left" class="left-pane" :style="{ width: layout.leftWidth + 'px' }">
        <MonitorPanel
          :session-id="active?.id ?? ''"
          :connected="activeConnected"
          :config="active?.config"
          @open-system-info="openSystemInfo"
          @open-process-list="openProcessList"
        />
      </div>

      <!-- 左右分隔条 -->
      <div v-show="panels.left" class="splitter splitter-v" @mousedown.prevent="startDragLeft"></div>

      <!-- 右侧：上终端（自适应） + 下文件区（固定高） -->
      <div class="right-pane">
        <div class="terminal-region">
          <TerminalPanel
            ref="terminalPanelRef"
            :save-settings="onSaveSettings"
            @preview-ui-scale="onPreviewUiScale"
            @install-update="requestInstallUpdate"
            @open-conn-manager="openConnectionManager"
          />
        </div>

        <!-- 上下分隔条 -->
        <div
          v-show="showBottomRegion"
          class="splitter splitter-h"
          @mousedown.prevent="startDragBottom"
        ></div>

        <!-- 底部文件区（固定高） -->
        <div
          id="bottom-pane"
          v-show="showBottomRegion"
          class="bottom-region"
          :style="{ height: layout.bottomHeight + 'px' }"
        >
          <BottomPanel
            ref="bottomPanelRef"
            :session-id="active?.id ?? ''"
            :connected="activeConnected"
            :sftp-enabled="active?.config.sftpEnabled !== false"
            :active="showBottomRegion"
            @sync-terminal-path="syncTerminalPath"
            @sync-file-path="syncFilePath"
          />
        </div>
      </div>

      <div v-show="panels.right" class="splitter splitter-v" @mousedown.prevent="startDragRight"></div>
      <aside
        id="auxiliary-pane"
        v-show="panels.right"
        class="auxiliary-pane"
        :style="{ width: layout.rightWidth + 'px' }"
        aria-label="右侧面板"
      >
        <div class="auxiliary-title">右侧面板</div>
        <div class="auxiliary-empty">功能暂未实现</div>
      </aside>
    </div>

    <!-- 底部状态栏 -->
    <div class="statusbar">
      <span>就绪</span>
      <span v-if="active" class="status-connection" :title="`${active.config.name} (${active.config.host}:${active.config.port})`">
        连接：{{ active.config.name }} ({{ active.config.host }}:{{
          active.config.port
        }})
      </span>
      <span v-else>未连接</span>
      <span>UTF-8</span>
      <div class="status-right" role="group" aria-label="面板布局">
        <button
          type="button"
          class="panel-toggle"
          :title="panels.left ? '收起左侧监控面板' : '展开左侧监控面板'"
          :aria-label="panels.left ? '收起左侧监控面板' : '展开左侧监控面板'"
          :aria-pressed="panels.left"
          aria-controls="monitor-pane"
          @click="togglePanel('left')"
        >
          <Icon name="panelLeft" :size="16" :filled="panels.left" />
        </button>
        <button
          type="button"
          class="panel-toggle"
          :title="showBottomRegion ? '收起底部文件管理器' : '展开底部文件管理器'"
          :aria-label="showBottomRegion ? '收起底部文件管理器' : '展开底部文件管理器'"
          :aria-pressed="showBottomRegion"
          aria-controls="bottom-pane"
          @click="togglePanel('bottom')"
        >
          <Icon name="panelBottom" :size="16" :filled="showBottomRegion" />
        </button>
        <button
          type="button"
          class="panel-toggle"
          :title="panels.right ? '收起右侧面板' : '展开右侧面板'"
          :aria-label="panels.right ? '收起右侧面板' : '展开右侧面板'"
          :aria-pressed="panels.right"
          aria-controls="auxiliary-pane"
          @click="togglePanel('right')"
        >
          <Icon name="panelRight" :size="16" :filled="panels.right" />
        </button>
      </div>
    </div>

    <!-- 连接管理器 -->
    <ConnectionManager
      v-if="showConnManager"
      @connect="onConnect"
      @close="showConnManager = false"
    />

    <AppDialog
      :open="showInstallConfirm"
      type="confirm"
      title="安装更新"
      message="安装更新需要退出程序，所有 SSH 会话将断开。接下来会检查未保存文件和未完成传输，是否继续？"
      confirm-text="继续安装"
      window-modal
      @confirm="confirmInstallUpdate"
      @cancel="cancelInstallUpdate"
    />

    <!-- 关闭软件前确认（存在连接中的会话时） -->
    <AppDialog
      :open="showCloseConfirm"
      type="confirm"
      title="退出程序"
      :message="`有 ${closeConfirmSessionCount} 个会话正处于连接中，确定要退出吗？`"
      confirm-text="退出"
      :confirm-danger="true"
      window-modal
      @confirm="onConfirmClose"
      @cancel="onCancelClose"
    />

    <AppDialog
      :open="Boolean(storageError)"
      type="info"
      title="连接数据加载失败"
      :message="storageError"
      @confirm="storageError = ''"
      @cancel="storageError = ''"
    />

    <!-- SSH 主机身份确认始终位于其他业务弹窗之上 -->
    <HostKeyDialog
      :open="!!pendingHostKeySession"
      :challenge="pendingHostKeySession?.hostKeyChallenge ?? null"
      :busy="hostKeyBusy"
      @confirm="onApproveHostKey(true)"
      @trust-once="onTrustHostKeyOnce"
      @cancel="onRejectHostKey"
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
.left-pane,
.auxiliary-pane {
  flex: 0 0 auto;
  min-width: 0;
  max-width: 30%;
  overflow: hidden;
}
.auxiliary-pane {
  display: flex;
  flex-direction: column;
  background: var(--bg-panel);
}
.auxiliary-title {
  display: flex;
  align-items: center;
  min-height: var(--tab-height);
  padding: 0 12px;
  border-bottom: 1px solid var(--border);
  color: var(--text-secondary);
  font-size: 12px;
}
.auxiliary-empty {
  flex: 1;
  display: grid;
  place-content: center;
  padding: 12px;
  color: var(--text-muted);
  font-size: 12px;
  overflow-wrap: anywhere;
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
  max-height: calc(100% - 100px);
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
.statusbar > span {
  flex-shrink: 0;
  white-space: nowrap;
}
.statusbar .status-connection {
  flex-shrink: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}
.statusbar .status-right {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: 2px;
  margin-left: auto;
}
.panel-toggle {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 20px;
  padding: 0;
  border: none;
  border-radius: 2px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
}
.panel-toggle:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.panel-toggle:focus-visible {
  outline: 1px solid var(--accent);
  outline-offset: -1px;
}
</style>
