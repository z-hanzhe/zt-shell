<script setup lang="ts">
/**
 * 右上 SSH 交互区：顶部选项卡栏（文件夹图标打开连接管理器）+ 终端区域
 *
 * 选项卡支持拖拽排序、横向滚动溢出、右键菜单（关闭/关闭其他/关闭全部/重连），
 * 关闭连接中的选项卡会二次确认；未选中会话有新输出时以叹号提示。
 */
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import Icon from "./Icon.vue";
import Terminal from "./Terminal.vue";
import AppDialog from "./AppDialog.vue";
import { useSessionsStore } from "../stores/sessions";

const emit = defineEmits<{
  (e: "open-conn-manager"): void;
}>();

const store = useSessionsStore();

/** 通用确认弹窗状态（复用 AppDialog，样式与文件管理一致） */
const dialog = reactive<{
  open: boolean;
  title: string;
  message: string;
  confirmText: string;
  confirmDanger: boolean;
  resolve?: (value: boolean) => void;
}>({ open: false, title: "", message: "", confirmText: "确定", confirmDanger: false });

/** 显示确认弹窗，返回用户是否确认 */
function showConfirm(opts: {
  title: string;
  message: string;
  confirmText?: string;
  confirmDanger?: boolean;
}): Promise<boolean> {
  return new Promise((resolve) => {
    Object.assign(dialog, {
      open: true,
      title: opts.title,
      message: opts.message,
      confirmText: opts.confirmText ?? "确定",
      confirmDanger: opts.confirmDanger ?? false,
      resolve,
    });
  });
}

/** 确认弹窗 */
function onDialogConfirm() {
  const resolve = dialog.resolve;
  dialog.open = false;
  resolve?.(true);
}

/** 取消弹窗 */
function onDialogCancel() {
  const resolve = dialog.resolve;
  dialog.open = false;
  resolve?.(false);
}

/** 各会话终端组件引用，用于切换选项卡后触发尺寸自适应 */
const termRefs = ref<Record<string, InstanceType<typeof Terminal>>>({});

/** 当前正在拖拽的会话 id（拖拽排序用） */
const draggingId = ref<string>("");

/** 右键菜单状态 */
const menu = reactive({ open: false, x: 0, y: 0, id: "" });

/** 右键菜单目标会话 */
const menuSession = computed(() => store.sessions.find((s) => s.id === menu.id));

/** 将当前激活终端切换到指定目录 */
function cdActiveTerminal(path: string) {
  return termRefs.value[store.activeId]?.cdTo(path);
}

/** 获取当前激活终端的目录 */
function requestActiveTerminalCwd() {
  return termRefs.value[store.activeId]?.requestCwd();
}

// 切换激活会话后，等 DOM 显示再重新适配终端尺寸并刷新视口
watch(
  () => store.activeId,
  (id) => {
    nextTick(() => termRefs.value[id]?.activate());
  }
);

/** 拖拽排序：记录被拖拽的选项卡 */
function onTabDragStart(id: string, e: DragEvent) {
  draggingId.value = id;
  if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
}

/** 拖拽经过其他选项卡时实时调整顺序 */
function onTabDragOver(targetId: string, e: DragEvent) {
  e.preventDefault();
  if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
  if (draggingId.value && draggingId.value !== targetId) {
    store.move(draggingId.value, targetId);
  }
}

/** 结束拖拽 */
function onTabDragEnd() {
  draggingId.value = "";
}

/** 打开选项卡右键菜单 */
function onTabContextMenu(id: string, e: MouseEvent) {
  menu.id = id;
  menu.x = e.clientX;
  menu.y = e.clientY;
  menu.open = true;
}

/** 关闭右键菜单 */
function closeMenu() {
  menu.open = false;
}

/** 关闭单个选项卡（连接中/连接后会二次确认） */
async function closeTab(id: string) {
  const s = store.sessions.find((x) => x.id === id);
  if (!s) return;
  if (s.status === "connecting" || s.status === "connected") {
    const ok = await showConfirm({
      title: "关闭会话",
      message: `会话「${s.name}」仍处于连接中，确定要关闭吗？`,
      confirmText: "关闭",
      confirmDanger: true,
    });
    if (!ok) return;
  }
  await store.close(id);
}

/** 关闭其他选项卡 */
async function closeOthers(id: string) {
  const others = store.sessions.filter((s) => s.id !== id);
  if (others.length === 0) return;
  const activeCount = others.filter(
    (s) => s.status === "connecting" || s.status === "connected"
  ).length;
  const ok = await showConfirm({
    title: "关闭其他会话",
    message: activeCount
      ? `将关闭其他 ${others.length} 个会话，其中 ${activeCount} 个仍在连接中，确定吗？`
      : `将关闭其他 ${others.length} 个会话，确定吗？`,
    confirmText: "关闭",
    confirmDanger: true,
  });
  if (!ok) return;
  for (const s of others) await store.close(s.id);
}

/** 关闭全部选项卡 */
async function closeAll() {
  if (store.sessions.length === 0) return;
  const activeCount = store.sessions.filter(
    (s) => s.status === "connecting" || s.status === "connected"
  ).length;
  const ok = await showConfirm({
    title: "关闭全部会话",
    message: activeCount
      ? `将关闭全部 ${store.sessions.length} 个会话，其中 ${activeCount} 个仍在连接中，确定吗？`
      : `将关闭全部 ${store.sessions.length} 个会话，确定吗？`,
    confirmText: "关闭",
    confirmDanger: true,
  });
  if (!ok) return;
  for (const s of [...store.sessions]) await store.close(s.id);
}

/** 重连指定会话：复用同一会话原地重连以保留历史输出 */
async function reconnect(id: string) {
  const reopenInPlace = await store.reconnect(id);
  if (reopenInPlace) {
    nextTick(() => termRefs.value[id]?.reopen());
  }
}

/** 菜单动作分发 */
async function onMenuAction(action: "close" | "closeOthers" | "closeAll" | "reconnect") {
  const id = menu.id;
  closeMenu();
  if (!id) return;
  switch (action) {
    case "close":
      await closeTab(id);
      break;
    case "closeOthers":
      await closeOthers(id);
      break;
    case "closeAll":
      await closeAll();
      break;
    case "reconnect":
      await reconnect(id);
      break;
  }
}

/** 连接状态对应的绿点提示文案 */
function statusTitle(status: string): string {
  if (status === "connected") return "已连接";
  if (status === "connecting") return "连接中";
  if (status === "error") return "连接失败";
  return "已断开";
}

/** 终端连接被关闭时更新对应选项卡状态（掉线/退出） */
function onTerminalClosed(id: string) {
  store.markDisconnected(id);
}

/** 未激活会话有新输出时标记提示 */
function onTerminalActivity(id: string) {
  store.markActivity(id);
}

/** 点击任意非菜单区域关闭右键菜单 */
function onGlobalPointerDown(e: PointerEvent) {
  if (!menu.open) return;
  const target = e.target as HTMLElement;
  if (target.closest(".tab-context-menu")) return;
  closeMenu();
}

/**
 * 是否存在处于连接中/已连接的会话，供关闭软件前判定是否需要确认
 * 供 App 层通过 ref 调用
 */
function hasLiveSessions(): boolean {
  return store.sessions.some(
    (s) => s.status === "connecting" || s.status === "connected"
  );
}

onMounted(() => {
  window.addEventListener("pointerdown", onGlobalPointerDown);
});

onBeforeUnmount(() => {
  window.removeEventListener("pointerdown", onGlobalPointerDown);
});

defineExpose({ cdActiveTerminal, requestActiveTerminalCwd, hasLiveSessions });
</script>

<template>
  <div class="term-panel">
    <!-- 选项卡栏 -->
    <div class="tabbar">
      <button
        class="tb-icon"
        title="连接管理器"
        @click="emit('open-conn-manager')"
      >
        <Icon name="folder" :size="16" />
      </button>

      <div class="tabs">
        <div
          v-for="s in store.sessions"
          :key="s.id"
          :class="['tab', { active: store.activeId === s.id, dragging: draggingId === s.id }]"
          draggable="true"
          @click="store.activate(s.id)"
          @contextmenu.prevent="onTabContextMenu(s.id, $event)"
          @dragstart="onTabDragStart(s.id, $event)"
          @dragover="onTabDragOver(s.id, $event)"
          @dragend="onTabDragEnd"
        >
          <!-- 未激活且有新输出时显示叹号提示（xshell 风格），否则显示连接状态绿点 -->
          <span
            v-if="s.activity && store.activeId !== s.id"
            class="live activity"
            title="有新输出"
          >!</span>
          <span v-else :class="['live', s.status]" :title="statusTitle(s.status)"></span>
          <span class="tab-name">{{ s.name }}</span>
          <button class="tab-close" title="关闭" @click.stop="closeTab(s.id)">
            <Icon name="close" :size="11" />
          </button>
        </div>
      </div>
    </div>

    <!-- 选项卡右键菜单 -->
    <div
      v-if="menu.open"
      class="tab-context-menu"
      :style="{ left: menu.x + 'px', top: menu.y + 'px' }"
    >
      <div class="tcm-item" @click="onMenuAction('close')">关闭</div>
      <div
        :class="['tcm-item', { disabled: store.sessions.length <= 1 }]"
        @click="onMenuAction('closeOthers')"
      >
        关闭其他
      </div>
      <div class="tcm-item" @click="onMenuAction('closeAll')">关闭全部</div>
      <div class="tcm-sep"></div>
      <div
        :class="['tcm-item', { disabled: !menuSession || menuSession.status === 'connecting' }]"
        @click="onMenuAction('reconnect')"
      >
        重连
      </div>
    </div>

    <!-- 终端区域 -->
    <div class="term-area">
      <template v-for="s in store.sessions" :key="s.id">
        <div v-show="store.activeId === s.id" class="term-slot">
          <div v-if="s.status === 'connecting'" class="term-status">
            正在连接 {{ s.config.host }} ...
          </div>
          <div v-else-if="s.status === 'error'" class="term-status error">
            连接失败：{{ s.error }}
          </div>
          <Terminal
            v-else
            :ref="(el) => { if (el) termRefs[s.id] = el as any }"
            :session-id="s.id"
            :connected="s.status === 'connected'"
            :active="store.activeId === s.id"
            @closed="onTerminalClosed(s.id)"
            @activity="onTerminalActivity(s.id)"
          />
        </div>
      </template>

      <!-- 无会话时的欢迎页 -->
      <div v-if="store.sessions.length === 0" class="welcome">
        <Icon name="terminal" :size="44" />
        <p>点击左上角文件夹图标打开连接管理器，开始一个 SSH 会话</p>
      </div>
    </div>

    <!-- 通用确认弹窗（关闭连接中会话等场景） -->
    <AppDialog
      :open="dialog.open"
      type="confirm"
      :title="dialog.title"
      :message="dialog.message"
      :confirm-text="dialog.confirmText"
      :confirm-danger="dialog.confirmDanger"
      @confirm="onDialogConfirm"
      @cancel="onDialogCancel"
    />
  </div>
</template>

<style scoped>
.term-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--terminal-bg);
}

/* 标签栏（浅色） */
.tabbar {
  display: flex;
  align-items: center;
  height: var(--tab-height);
  background: var(--bg-panel-alt);
  border-bottom: 1px solid var(--border);
  padding: 0 6px;
  flex: 0 0 auto;
}
.tb-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 22px;
  border: none;
  background: transparent;
  color: var(--accent);
  cursor: pointer;
  flex-shrink: 0;
}
.tb-icon:hover {
  color: var(--accent-hover);
}
.tabs {
  display: flex;
  flex: 1;
  overflow-x: auto;
  height: 100%;
  align-items: flex-end;
  gap: 0;
}
.tabs::-webkit-scrollbar {
  height: 0;
}
.tab {
  display: flex;
  align-items: center;
  gap: 7px;
  height: 26px;
  padding: 0 10px;
  margin-left: 4px;
  max-width: 180px;
  background: #e4e9ee;
  border: 1px solid var(--border);
  border-bottom: none;
  border-radius: 4px 4px 0 0;
  font-size: 12px;
  color: #555;
  cursor: pointer;
  white-space: nowrap;
}
.tab:hover {
  background: #edf1f5;
}
.tab.active {
  background: #fff;
  color: #222;
}
.live {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
  background: #9aa2aa;
}
.live.connecting {
  background: var(--warning);
}
.live.connected {
  background: var(--terminal-green);
}
.live.error,
.live.disconnected {
  background: var(--danger);
}
/* 未读输出叹号提示（仿 xshell）：橙色小圆点内显示叹号 */
.live.activity {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 10px;
  height: 10px;
  background: var(--warning);
  color: #fff;
  font-size: 9px;
  font-weight: 700;
  line-height: 1;
}
/* 拖拽中的选项卡半透明提示 */
.tab.dragging {
  opacity: 0.5;
}
.tab-name {
  overflow: hidden;
  text-overflow: ellipsis;
}
.tab-close {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 15px;
  height: 15px;
  border: none;
  border-radius: 3px;
  background: transparent;
  color: #999;
  cursor: pointer;
}
.tab-close:hover {
  background: var(--danger);
  color: #fff;
}
/* 终端区域 */
.term-area {
  flex: 1;
  position: relative;
  overflow: hidden;
  background: linear-gradient(180deg, var(--terminal-bg) 0%, var(--terminal-bg2) 100%);
}
.term-slot {
  position: absolute;
  inset: 0;
}
.term-status {
  padding: 20px;
  color: var(--terminal-text);
  font-family: "Consolas", monospace;
}
.term-status.error {
  color: #f7768e;
}
.welcome {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100%;
  gap: 16px;
  color: #565f89;
}
/* 选项卡右键菜单 */
.tab-context-menu {
  position: fixed;
  z-index: 1200;
  min-width: 120px;
  padding: 4px 0;
  background: #fff;
  border: 1px solid var(--border);
  border-radius: 4px;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.18);
  font-size: 12px;
  color: #333;
}
.tcm-item {
  padding: 6px 14px;
  cursor: pointer;
  white-space: nowrap;
}
.tcm-item:hover {
  background: var(--row-hover);
}
.tcm-item.disabled {
  color: #bbb;
  pointer-events: none;
}
.tcm-sep {
  height: 1px;
  margin: 4px 0;
  background: var(--border-light);
}
</style>
