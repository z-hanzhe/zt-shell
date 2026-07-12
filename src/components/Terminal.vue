<script setup lang="ts">
/**
 * SSH 终端组件：封装 xterm，负责渲染远端输出并回传用户输入
 */
import { onBeforeUnmount, onMounted, ref, shallowRef, watch } from "vue";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
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
let unlistenData: UnlistenFn | null = null;
let unlistenClose: UnlistenFn | null = null;
let resizeObserver: ResizeObserver | null = null;
let opened = false;
let pwdBuffer = "";
const pendingPwdRequests = new Map<string, (path: string) => void>();

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
  t.loadAddon(fitAddon);
  t.open(container.value);
  fitAddon.fit();
  term.value = t;

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
  if (props.connected) setup();
});

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
  unlistenData?.();
  unlistenClose?.();
  term.value?.dispose();
  pendingPwdRequests.clear();
});

// 暴露刷新方法供父组件在切换选项卡后调用
defineExpose({ fit: doFit, activate, cdTo, requestCwd });
</script>

<template>
  <div class="terminal-wrap">
    <div ref="container" class="terminal-body"></div>
  </div>
</template>

<style scoped>
.terminal-wrap {
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
</style>
