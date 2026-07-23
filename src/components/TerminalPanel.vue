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

/** 选项卡滚动容器 */
const tabsRef = ref<HTMLElement | null>(null);
/** 是否显示左右滚动按钮（选项卡溢出时） */
const canScrollLeft = ref(false);
const canScrollRight = ref(false);

/** 当前正在拖拽的会话 id（拖拽排序用） */
const draggingId = ref<string>("");
/** 被按住选项卡跟随光标的水平位移（像素） */
const dragOffset = ref(0);
/** 拖拽起始下标（原始顺序中的位置） */
const dragFrom = ref(-1);
/** 拖拽目标下标（实时判定的落点位置） */
const dragTo = ref(-1);
/** 让位平移步长：被按住选项卡占位宽度（含左外边距） */
const dragStep = ref(0);
/** 松手提交那一帧临时关闭选项卡过渡，避免重排后 transform 归零被动画化造成回弹闪动 */
const suppressTabAnim = ref(false);
/** 指针拖拽运行时状态 */
let tabDrag: TabDragState | null = null;

type TabDragState = {
  id: string;
  /** 按下时的指针 X */
  startX: number;
  /** 是否已越过阈值真正进入拖拽 */
  moved: boolean;
  /** 拖拽开始时各选项卡中心点（相对 tabs 内容坐标，含滚动偏移，顺序固定不变） */
  centers: number[];
};

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

// 选项卡增减后重新计算左右滚动按钮的可用状态
watch(
  () => store.sessions.length,
  () => nextTick(updateScrollState)
);

/**
 * 选项卡指针按下：准备拖拽排序（超过阈值才真正开始，避免影响点击）
 * 用 pointer 事件自行实现而非 HTML5 拖放，WebView2 下拖放放置不稳定，
 * 且自绘可实现按住项跟随光标、被跨过项平移让位的动画效果
 */
function onTabPointerDown(id: string, e: PointerEvent) {
  // 仅左键，且点在关闭按钮上时不启动拖拽
  if (e.button !== 0) return;
  if ((e.target as HTMLElement).closest(".tab-close")) return;
  const tabsEl = tabsRef.value;
  const tabEl = e.currentTarget as HTMLElement;
  if (!tabsEl) return;
  // 拖拽全程冻结这份布局快照：各选项卡中心点用视口客户端坐标（与 e.clientX 同一坐标系），
  // 避免 offsetLeft 相对 offsetParent 造成的坐标错位；顺序与占位宽度不随让位动画变化，
  // 从根本上消除“换位后重新测量 -> 判定漂移 -> 来回闪”的反馈回路
  const centers = Array.from(tabsEl.querySelectorAll<HTMLElement>(".tab")).map((el) => {
    const r = el.getBoundingClientRect();
    return r.left + r.width / 2;
  });
  const from = store.sessions.findIndex((s) => s.id === id);
  tabDrag = { id, startX: e.clientX, moved: false, centers };
  dragFrom.value = from;
  dragTo.value = from;
  // 让位步长取被按住项自身的占位宽度（含 4px 左外边距），邻居整体平移该距离
  dragStep.value = tabEl.offsetWidth + 4;
  window.addEventListener("pointermove", onTabPointerMove);
  window.addEventListener("pointerup", onTabPointerUp, { once: true });
}

/** 拖拽移动：被按住项跟随光标，其余选项卡按目标下标整体让位（不改动数组） */
function onTabPointerMove(e: PointerEvent) {
  if (!tabDrag) return;
  const dx = e.clientX - tabDrag.startX;
  // 超过 4px 才判定为拖拽，规避轻微抖动误触
  if (!tabDrag.moved) {
    if (Math.abs(dx) < 4) return;
    tabDrag.moved = true;
    draggingId.value = tabDrag.id;
  }
  // 被按住项严格跟随光标（相对原始槽位的位移即光标位移），无换位跳变
  dragOffset.value = dx;
  // 落点下标 = 除被按住项外、中心点在光标左侧的选项卡数量
  // 该值同时等于最终应处的下标与 moveToIndex 的实参（先移除再插入语义，各方向皆准）
  let target = 0;
  for (let i = 0; i < tabDrag.centers.length; i++) {
    if (i !== dragFrom.value && tabDrag.centers[i] < e.clientX) target++;
  }
  dragTo.value = target;
}

/** 结束拖拽：提交一次排序并清理状态 */
function onTabPointerUp() {
  window.removeEventListener("pointermove", onTabPointerMove);
  const committed = !!tabDrag?.moved && dragTo.value >= 0 && dragTo.value !== dragFrom.value;
  if (committed) {
    // 提交这一帧关闭过渡：数组重排后各选项卡真实槽位跳到让位位置，
    // 若保留 transition 会把 translateX(-step) -> 0 动画化，造成松手后回弹闪一下。
    // 先禁用过渡让 transform 与新槽位同帧归零，下一帧再恢复动画。
    suppressTabAnim.value = true;
    store.moveToIndex(tabDrag!.id, dragTo.value);
  }
  draggingId.value = "";
  dragOffset.value = 0;
  dragFrom.value = -1;
  dragTo.value = -1;
  dragStep.value = 0;
  tabDrag = null;
  if (committed) {
    // 等 Vue 刷新重排后的 DOM，再等一帧渲染完成后恢复过渡，确保归零无过渡地落定
    nextTick(() => requestAnimationFrame(() => (suppressTabAnim.value = false)));
  }
}

/**
 * 计算某下标选项卡在拖拽期间的让位平移量：
 * 被按住项由 dragOffset 单独跟随光标，其余项在 from 与 to 之间的区段整体平移一个步长让位
 */
function tabShift(index: number): number {
  if (draggingId.value === "" || index === dragFrom.value) return 0;
  const from = dragFrom.value;
  const to = dragTo.value;
  // 向右拖：(from, to] 区间的项左移一格；向左拖：[to, from) 区间的项右移一格
  if (to > from && index > from && index <= to) return -dragStep.value;
  if (to < from && index >= to && index < from) return dragStep.value;
  return 0;
}

/** 滚动选项卡栏：dir 为 -1 左移、1 右移，每次滚动约一屏的 50% */
function scrollTabs(dir: -1 | 1) {
  const el = tabsRef.value;
  if (!el) return;
  el.scrollBy({ left: dir * el.clientWidth * 0.5, behavior: "smooth" });
}

/** 更新左右滚动按钮的可用状态 */
function updateScrollState() {
  const el = tabsRef.value;
  if (!el) {
    canScrollLeft.value = false;
    canScrollRight.value = false;
    return;
  }
  canScrollLeft.value = el.scrollLeft > 1;
  canScrollRight.value = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
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
      message: `会话 [ ${s.name} ] 仍处于连接中，确定要关闭吗？`,
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

/** 监听选项卡栏尺寸变化，动态更新滚动按钮状态 */
let tabsResizeObserver: ResizeObserver | null = null;

// 选项卡数量变化后，等 DOM 更新再刷新滚动按钮状态
watch(
  () => store.sessions.length,
  () => nextTick(updateScrollState)
);

onMounted(() => {
  window.addEventListener("pointerdown", onGlobalPointerDown);
  if (tabsRef.value) {
    tabsResizeObserver = new ResizeObserver(() => updateScrollState());
    tabsResizeObserver.observe(tabsRef.value);
  }
  nextTick(updateScrollState);
});

onBeforeUnmount(() => {
  window.removeEventListener("pointerdown", onGlobalPointerDown);
  window.removeEventListener("pointermove", onTabPointerMove);
  tabsResizeObserver?.disconnect();
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

      <!-- 左滚动按钮：选项卡溢出且非最左时显示 -->
      <button
        v-show="canScrollLeft"
        class="tb-scroll"
        title="向左滚动"
        @click="scrollTabs(-1)"
      >
        <Icon name="chevronLeft" :size="14" />
      </button>

      <div ref="tabsRef" class="tabs" @scroll="updateScrollState">
        <div
          v-for="(s, i) in store.sessions"
          :key="s.id"
          :class="['tab', { active: store.activeId === s.id, dragging: draggingId === s.id, 'no-anim': suppressTabAnim }]"
          :style="{ transform: `translateX(${draggingId === s.id ? dragOffset : tabShift(i)}px)` }"
          @pointerdown="onTabPointerDown(s.id, $event)"
          @click="store.activate(s.id)"
          @contextmenu.prevent="onTabContextMenu(s.id, $event)"
        >
          <!-- 状态指示位固定尺寸，绿点/叹号切换时不改变宽度避免选项卡抖动 -->
          <span class="tab-indicator">
            <span
              v-if="s.activity && store.activeId !== s.id"
              class="ind-badge"
              title="有新输出"
            >!</span>
            <span v-else :class="['ind-dot', s.status]" :title="statusTitle(s.status)"></span>
          </span>
          <span class="tab-name">{{ s.name }}</span>
          <button class="tab-close" title="关闭" @click.stop="closeTab(s.id)">
            <Icon name="close" :size="11" />
          </button>
        </div>
      </div>

      <!-- 右滚动按钮：选项卡溢出且非最右时显示 -->
      <button
        v-show="canScrollRight"
        class="tb-scroll"
        title="向右滚动"
        @click="scrollTabs(1)"
      >
        <Icon name="chevronRight" :size="14" />
      </button>
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
/* 选项卡栏左右滚动按钮 */
.tb-scroll {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 22px;
  border: none;
  background: transparent;
  color: #888;
  cursor: pointer;
  flex-shrink: 0;
}
.tb-scroll:hover {
  color: var(--accent);
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
  /* 被跨过的选项卡平移让位时的过渡动画 */
  transition: transform 0.18s ease;
}
.tab:hover {
  background: #edf1f5;
}
.tab.active {
  background: #fff;
  color: #222;
}
/* 拖拽中的选项卡：跟随光标、置顶、去除过渡以免跟手延迟 */
.tab.dragging {
  transition: none;
  position: relative;
  z-index: 5;
  opacity: 0.9;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.2);
  cursor: grabbing;
}
/* 松手提交那一帧禁用过渡：让 transform 归零与数组重排同帧落定，避免回弹闪动 */
.tab.no-anim {
  transition: none;
}
/* 状态指示位：固定尺寸容器，内部绿点/叹号切换不改变选项卡宽度 */
.tab-indicator {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 10px;
  height: 10px;
  flex-shrink: 0;
}
.ind-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #9aa2aa;
}
.ind-dot.connecting {
  background: var(--warning);
}
.ind-dot.connected {
  background: var(--terminal-green);
}
.ind-dot.error,
.ind-dot.disconnected {
  background: var(--danger);
}
/* 未读输出叹号提示（仿 xshell）：占满指示位，尺寸与绿点容器一致不抖动 */
.ind-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
  border-radius: 50%;
  background: var(--warning);
  color: #fff;
  font-size: 9px;
  font-weight: 700;
  line-height: 1;
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
