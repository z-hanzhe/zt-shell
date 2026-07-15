<script setup lang="ts">
/**
 * SSH 终端组件：封装 xterm，负责渲染远端输出并回传用户输入，
 * 提供右键菜单（复制/粘贴/查找/全选/清空缓存）、查找高亮与 clear 软清屏
 */
import { onBeforeUnmount, onMounted, reactive, ref, shallowRef, watch } from "vue";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import "@xterm/xterm/css/xterm.css";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import Icon from "./Icon.vue";
import { terminalOpen, terminalWrite, terminalResize } from "../api";
import { useSettingsStore } from "../stores/settings";

const props = defineProps<{
  /** 会话标识 */
  sessionId: string;
  /** 会话是否已连接（连接成功后再开启终端） */
  connected: boolean;
}>();

const settings = useSettingsStore();

/** 终端挂载容器 */
const container = ref<HTMLDivElement>();
// 终端实例存于 shallowRef，避免被 Vue 响应式代理干扰内部实现
const term = shallowRef<Terminal>();
let fitAddon: FitAddon;
let searchAddon: SearchAddon;
let unlistenData: UnlistenFn | null = null;
let unlistenClose: UnlistenFn | null = null;
let resizeObserver: ResizeObserver | null = null;
let opened = false;
let pwdBuffer = "";
const pendingPwdRequests = new Map<string, (path: string) => void>();

/** 右键菜单状态 */
const contextMenu = reactive({ open: false, x: 0, y: 0 });
/** 查找栏状态 */
const search = reactive({ open: false, keyword: "", current: 0, total: 0 });
/** 查找输入框引用 */
const searchInput = ref<HTMLInputElement>();

/** 查找高亮配色（命中黄色、当前项橙色） */
const searchDecorations = {
  matchBackground: "#3b4a6b",
  matchBorder: "#7aa2f7",
  matchOverviewRuler: "#7aa2f7",
  activeMatchBackground: "#e0af68",
  activeMatchBorder: "#e0af68",
  activeMatchColorOverviewRuler: "#e0af68",
};

/**
 * xterm 主题：Tokyo Night 配色
 * 完整 16 色 ANSI 调色板，使 ls --color、日志高亮等 ANSI 转义色正确渲染
 * 背景用不透明纯色，WebView 下透明会渲染成纯黑
 */
const theme = {
  background: "#1a1b26",
  foreground: "#c0caf5",
  cursor: "#c0caf5",
  cursorAccent: "#1a1b26",
  selectionBackground: "#28344a",
  // 标准色
  black: "#15161e",
  red: "#f7768e",
  green: "#9ece6a",
  yellow: "#e0af68",
  blue: "#7aa2f7",
  magenta: "#bb9af7",
  cyan: "#7dcfff",
  white: "#a9b1d6",
  // 亮色
  brightBlack: "#414868",
  brightRed: "#f7768e",
  brightGreen: "#9ece6a",
  brightYellow: "#e0af68",
  brightBlue: "#7aa2f7",
  brightMagenta: "#bb9af7",
  brightCyan: "#7dcfff",
  brightWhite: "#c0caf5",
};

/** 将路径转为 POSIX shell 单引号参数 */
function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'"'"'`)}'`;
}

/** 写入终端字节 */
function writeToTerminal(data: string): Promise<void> {
  const bytes = Array.from(new TextEncoder().encode(data));
  return terminalWrite(props.sessionId, bytes);
}

/** 从输出中捕获专用 OSC 当前目录标记 */
function capturePwdMarkers(bytes: number[]) {
  const text = new TextDecoder().decode(new Uint8Array(bytes));
  pwdBuffer = (pwdBuffer + text).slice(-4096);
  const marker = /\x1b\]6973;ZTSHELL_PWD=([^:]+):([^\x07]*)\x07/g;
  let match: RegExpExecArray | null;
  while ((match = marker.exec(pwdBuffer))) {
    pendingPwdRequests.get(match[1])?.(match[2]);
    pendingPwdRequests.delete(match[1]);
  }
  if (!pwdBuffer.includes("\x1b]")) pwdBuffer = "";
}

/** 初始化终端并绑定事件 */
async function setup() {
  if (!container.value || opened) return;
  opened = true;

  const t = new Terminal({
    fontFamily: settings.settings.fontFamily,
    fontSize: settings.settings.fontSize,
    lineHeight: 1.25,
    letterSpacing: 0,
    cursorBlink: settings.settings.cursorBlink,
    theme,
    scrollback: 5000,
  });
  fitAddon = new FitAddon();
  searchAddon = new SearchAddon();
  t.loadAddon(fitAddon);
  t.loadAddon(searchAddon);
  // 拦截 clear 的清屏序列，改为软清屏（内容顶到滚动区外，保留历史）
  installSoftClear(t);
  t.open(container.value);
  fitAddon.fit();
  term.value = t;

  // 查找结果变化时更新计数
  searchAddon.onDidChangeResults((e) => {
    search.total = e.resultCount;
    search.current = e.resultCount === 0 ? 0 : e.resultIndex + 1;
  });

  // 快捷键：Ctrl+Shift+C/V/F/A，preventDefault 阻止 WebView 默认行为并返回 false 跳过终端输入
  t.attachCustomKeyEventHandler((event) => {
    if (event.type !== "keydown") return true;
    if (event.ctrlKey && event.shiftKey) {
      const key = event.key.toLowerCase();
      if (key === "c") {
        event.preventDefault();
        copySelection();
        return false;
      }
      if (key === "v") {
        event.preventDefault();
        pasteClipboard();
        return false;
      }
      if (key === "f") {
        event.preventDefault();
        openSearch();
        return false;
      }
      if (key === "a") {
        event.preventDefault();
        t.selectAll();
        return false;
      }
    }
    // 查找栏打开时 Esc 关闭查找
    if (event.key === "Escape" && search.open) {
      closeSearch();
      return false;
    }
    return true;
  });

  // 监听后端推送的远端输出
  const dataEvent = `terminal://data//${props.sessionId}`;
  const closeEvent = `terminal://close//${props.sessionId}`;
  unlistenData = await listen<number[]>(dataEvent, (e) => {
    capturePwdMarkers(e.payload);
    t.write(new Uint8Array(e.payload));
  });
  unlistenClose = await listen(closeEvent, () => {
    t.write("\r\n\x1b[33m[连接已关闭]\x1b[0m\r\n");
  });

  // 用户输入回传后端
  t.onData((data) => {
    writeToTerminal(data).catch(() => {});
  });

  // 开启后端终端通道
  await terminalOpen(props.sessionId, t.cols, t.rows);

  // 尺寸自适应
  resizeObserver = new ResizeObserver(() => doFit());
  resizeObserver.observe(container.value);
}

/**
 * 安装软清屏：拦截 clear 发出的擦除全屏序列（CSI 2J），
 * 改为把当前视口内容滚入回滚区（保留历史，可向上翻查），
 * 光标行移动到视口顶部；真正清空由右键"清空屏幕缓存区"执行 term.clear()。
 * CSI 3J（清回滚区）忽略，避免 clear 连带清掉历史。
 */
function installSoftClear(t: Terminal) {
  // 拦截 ED（Erase in Display）：参数 2=擦除全屏，3=擦除回滚区
  t.parser.registerCsiHandler({ final: "J" }, (params) => {
    const raw = params[0];
    const mode = typeof raw === "number" ? raw : 0;
    if (mode === 2) {
      softClear(t);
      return true; // 已处理，阻止默认擦除
    }
    if (mode === 3) {
      return true; // 忽略清回滚区，保留历史
    }
    return false; // 其余（0/1 局部擦除）交由 xterm 默认处理
  });
}

/**
 * 软清屏：clear 触发时（光标已被 ESC[H 移到视口左上角）把视口现有内容
 * 滚入回滚区，新提示符出现在视口第一行，历史仍可向上翻查
 */
function softClear(t: Terminal) {
  const buffer = t.buffer.active;
  // 找视口内最后一个非空行
  let lastRow = -1;
  for (let i = t.rows - 1; i >= 0; i--) {
    const line = buffer.getLine(buffer.baseY + i);
    if (line && line.translateToString(true).length > 0) {
      lastRow = i;
      break;
    }
  }
  // 视口本就为空则无需滚动
  if (lastRow < 0) return;
  // 光标移到底行后写入换行逐行滚入回滚区，再把光标移回视口顶部
  t.write(`\x1b[${t.rows};1H${"\n".repeat(lastRow + 1)}\x1b[H`);
}

/** 适配容器尺寸并同步到后端 */
function doFit() {
  if (!term.value || !fitAddon || !container.value) return;
  // 容器不可见（display:none）时尺寸为 0，跳过以免 fit 成 0 行导致视口错乱
  if (container.value.offsetHeight === 0 || container.value.offsetWidth === 0) {
    return;
  }
  try {
    fitAddon.fit();
    terminalResize(props.sessionId, term.value.cols, term.value.rows).catch(
      () => {}
    );
  } catch {
    // 容器不可见时忽略
  }
}

/**
 * 选项卡重新激活时调用：重新适配尺寸并刷新 xterm 视口
 * 修复隐藏（display:none）后再显示时滚动条错位、需手动滚到顶部才恢复的问题
 */
function activate() {
  if (!term.value) return;
  // 等布局生效后再刷新，确保容器已有真实尺寸
  requestAnimationFrame(() => {
    doFit();
    const t = term.value;
    if (!t) return;
    // 强制重绘全部可见行并滚动到底部，同步视口滚动状态
    t.refresh(0, t.rows - 1);
    t.scrollToBottom();
  });
}

/** 复制当前选中内容到剪贴板 */
async function copySelection() {
  const text = term.value?.getSelection();
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // 剪贴板不可用时忽略
  }
}

/** 从剪贴板粘贴到终端 */
async function pasteClipboard() {
  try {
    const text = await navigator.clipboard.readText();
    if (text) await writeToTerminal(text);
  } catch {
    // 剪贴板不可用时忽略
  }
}

/** 全选终端内容 */
function selectAll() {
  term.value?.selectAll();
}

/** 清空终端屏幕与回滚缓存区 */
function clearBuffer() {
  term.value?.clear();
}

/** 打开查找栏 */
function openSearch() {
  search.open = true;
  requestAnimationFrame(() => {
    searchInput.value?.focus();
    searchInput.value?.select();
  });
}

/** 关闭查找栏并清除高亮 */
function closeSearch() {
  search.open = false;
  search.keyword = "";
  search.current = 0;
  search.total = 0;
  searchAddon?.clearDecorations();
  term.value?.focus();
}

/** 执行查找：向后查找下一处 */
function findNext() {
  if (!search.keyword) {
    searchAddon?.clearDecorations();
    return;
  }
  searchAddon?.findNext(search.keyword, { decorations: searchDecorations });
}

/** 执行查找：向前查找上一处 */
function findPrevious() {
  if (!search.keyword) return;
  searchAddon?.findPrevious(search.keyword, { decorations: searchDecorations });
}

/** 关键字变化时从头增量查找 */
function onSearchInput() {
  if (!search.keyword) {
    searchAddon?.clearDecorations();
    search.current = 0;
    search.total = 0;
    return;
  }
  searchAddon?.findNext(search.keyword, { decorations: searchDecorations, incremental: true });
}

/** 打开右键菜单（边缘收敛不超出视口） */
function onContextMenu(event: MouseEvent) {
  event.preventDefault();
  const MENU_W = 208;
  const MENU_H = 200;
  contextMenu.open = true;
  contextMenu.x = Math.min(event.clientX, window.innerWidth - MENU_W - 8);
  contextMenu.y = Math.min(event.clientY, window.innerHeight - MENU_H - 8);
}

/** 关闭右键菜单 */
function closeContextMenu() {
  contextMenu.open = false;
}

/** 右键菜单项 */
const menuItems = [
  { action: "copy", label: "复制", shortcut: "Ctrl+Shift+C" },
  { action: "paste", label: "粘贴", shortcut: "Ctrl+Shift+V" },
  { action: "find", label: "查找", shortcut: "Ctrl+Shift+F" },
  { action: "selectAll", label: "全选", shortcut: "Ctrl+Shift+A" },
  { action: "clear", label: "清空屏幕缓存区", shortcut: "" },
] as const;

/** 执行右键菜单动作 */
function runMenuAction(action: (typeof menuItems)[number]["action"]) {
  closeContextMenu();
  switch (action) {
    case "copy":
      copySelection();
      break;
    case "paste":
      pasteClipboard();
      break;
    case "find":
      openSearch();
      break;
    case "selectAll":
      selectAll();
      break;
    case "clear":
      clearBuffer();
      break;
  }
}

/** 点击任意非菜单区域关闭右键菜单 */
function onGlobalPointerDown(event: PointerEvent) {
  if (!contextMenu.open) return;
  const target = event.target as HTMLElement;
  if (target.closest(".term-context-menu")) return;
  closeContextMenu();
}

/** 将交互终端当前目录切换到指定路径 */
function cdTo(path: string) {
  if (!term.value || !props.connected) return Promise.resolve();
  return writeToTerminal(`cd -- ${shellQuote(path)}\r`);
}

/** 请求交互终端回传当前目录 */
function requestCwd(): Promise<string> {
  if (!term.value || !props.connected) return Promise.reject(new Error("终端未连接"));
  const token = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return new Promise((resolve, reject) => {
    const timer = window.setTimeout(() => {
      pendingPwdRequests.delete(token);
      reject(new Error("获取终端路径超时"));
    }, 3000);
    pendingPwdRequests.set(token, (path) => {
      window.clearTimeout(timer);
      resolve(path || "/");
    });
    writeToTerminal(`printf '\\033]6973;ZTSHELL_PWD=${token}:%s\\007' "$PWD"\r`).catch((e) => {
      window.clearTimeout(timer);
      pendingPwdRequests.delete(token);
      reject(e);
    });
  });
}

// 连接成功后再初始化终端
watch(
  () => props.connected,
  (v) => {
    if (v) setup();
  },
  { immediate: true }
);

onMounted(() => {
  window.addEventListener("pointerdown", onGlobalPointerDown);
  if (props.connected) setup();
});

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
  unlistenData?.();
  unlistenClose?.();
  window.removeEventListener("pointerdown", onGlobalPointerDown);
  term.value?.dispose();
  pendingPwdRequests.clear();
});

// 暴露刷新方法供父组件在切换选项卡后调用
defineExpose({ fit: doFit, activate, cdTo, requestCwd });
</script>

<template>
  <div class="terminal-wrap">
    <div ref="container" class="terminal-body" @contextmenu="onContextMenu"></div>

    <!-- 查找栏 -->
    <div v-if="search.open" class="term-search" @keydown.stop>
      <input
        ref="searchInput"
        v-model="search.keyword"
        class="ts-input"
        placeholder="查找"
        @input="onSearchInput"
        @keydown.enter.prevent="$event.shiftKey ? findPrevious() : findNext()"
        @keydown.esc.prevent="closeSearch"
      />
      <span class="ts-count">{{ search.total ? `${search.current}/${search.total}` : "无结果" }}</span>
      <button class="ts-btn" title="上一个 (Shift+Enter)" @click="findPrevious">
        <Icon name="arrowUp" :size="13" />
      </button>
      <button class="ts-btn" title="下一个 (Enter)" @click="findNext">
        <Icon name="arrowDown" :size="13" />
      </button>
      <button class="ts-btn" title="关闭 (Esc)" @click="closeSearch">
        <Icon name="close" :size="13" />
      </button>
    </div>

    <!-- 右键菜单 -->
    <div
      v-if="contextMenu.open"
      class="term-context-menu"
      :style="{ left: `${contextMenu.x}px`, top: `${contextMenu.y}px` }"
      @click.stop
    >
      <button v-for="item in menuItems" :key="item.action" @click="runMenuAction(item.action)">
        <span>{{ item.label }}</span>
        <span v-if="item.shortcut" class="tm-shortcut">{{ item.shortcut }}</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.terminal-wrap {
  position: relative;
  width: 100%;
  height: 100%;
  background: #1a1b26;
  overflow: hidden;
}
.terminal-body {
  width: 100%;
  height: 100%;
  padding: 6px 12px;
}

/* 查找栏：右上角浮层 */
.term-search {
  position: absolute;
  top: 8px;
  right: 16px;
  z-index: 20;
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 5px 8px;
  border: 1px solid #2a2f45;
  border-radius: 6px;
  background: #1f2335;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.4);
}
.ts-input {
  width: 160px;
  height: 24px;
  padding: 0 8px;
  border: 1px solid #33395a;
  border-radius: 4px;
  background: #16161e;
  color: #c0caf5;
  font-size: 12px;
  outline: none;
}
.ts-input:focus {
  border-color: #7aa2f7;
}
.ts-count {
  min-width: 44px;
  text-align: center;
  color: #8a94b8;
  font-size: 12px;
}
.ts-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: #a9b1d6;
  cursor: pointer;
}
.ts-btn:hover {
  background: #2a2f45;
  color: #c0caf5;
}

/* 右键菜单：深色贴合终端 */
.term-context-menu {
  position: fixed;
  z-index: 30;
  min-width: 200px;
  padding: 4px;
  border: 1px solid #2a2f45;
  border-radius: 6px;
  background: #1f2335;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.45);
}
.term-context-menu button {
  display: flex;
  align-items: center;
  justify-content: space-between;
  width: 100%;
  height: 26px;
  padding: 0 10px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: #c0caf5;
  font-size: 12px;
  cursor: pointer;
}
.term-context-menu button:hover {
  background: #2a2f45;
}
.tm-shortcut {
  color: #6a729a;
  font-size: 11px;
  margin-left: 24px;
}
</style>
