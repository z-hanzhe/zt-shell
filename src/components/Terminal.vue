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

/** xterm 主题，与 FinalShell 终端风格保持一致（用不透明纯色背景，WebView 下透明会渲染成纯黑） */
const theme = {
  background: "#12303d",
  foreground: "#cfe3ea",
  cursor: "#57d977",
  cursorAccent: "#12303d",
  selectionBackground: "#2f5866",
  black: "#0f2833",
  brightBlack: "#5a7683",
};

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
    t.write(new Uint8Array(e.payload));
  });
  unlistenClose = await listen(closeEvent, () => {
    t.write("\r\n\x1b[33m[连接已关闭]\x1b[0m\r\n");
  });

  // 用户输入回传后端
  t.onData((data) => {
    const bytes = Array.from(new TextEncoder().encode(data));
    terminalWrite(props.sessionId, bytes).catch(() => {});
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
});

// 暴露刷新方法供父组件在切换选项卡后调用
defineExpose({ fit: doFit, activate });
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
  background: #12303d;
  overflow: hidden;
}
.terminal-body {
  width: 100%;
  height: 100%;
  padding: 4px 6px;
}
</style>
