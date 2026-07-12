<script setup lang="ts">
/**
 * 右上 SSH 交互区：顶部选项卡栏（文件夹图标打开连接管理器、设置按钮）+ 终端区域
 */
import { nextTick, ref, watch } from "vue";
import Icon from "./Icon.vue";
import Terminal from "./Terminal.vue";
import { useSessionsStore } from "../stores/sessions";

const emit = defineEmits<{
  (e: "open-conn-manager"): void;
  (e: "open-settings"): void;
}>();

const store = useSessionsStore();

/** 各会话终端组件引用，用于切换选项卡后触发尺寸自适应 */
const termRefs = ref<Record<string, InstanceType<typeof Terminal>>>({});

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

defineExpose({ cdActiveTerminal, requestActiveTerminalCwd });
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
          :class="['tab', { active: store.activeId === s.id }]"
          @click="store.activate(s.id)"
        >
          <span :class="['live', s.status]"></span>
          <span class="tab-name">{{ s.name }}</span>
          <button class="tab-close" @click.stop="store.close(s.id)">
            <Icon name="close" :size="11" />
          </button>
        </div>
      </div>

      <div class="tb-right">
        <button class="tb-icon" title="设置" @click="emit('open-settings')">
          <Icon name="settings" :size="15" />
        </button>
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
          />
        </div>
      </template>

      <!-- 无会话时的欢迎页 -->
      <div v-if="store.sessions.length === 0" class="welcome">
        <Icon name="terminal" :size="44" />
        <p>点击左上角文件夹图标打开连接管理器，开始一个 SSH 会话</p>
      </div>
    </div>
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
.live.error {
  background: var(--danger);
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
.tb-right {
  margin-left: auto;
  display: flex;
  align-items: center;
  padding-right: 4px;
}
.tb-right .tb-icon {
  color: #666;
}
.tb-right .tb-icon:hover {
  color: var(--accent);
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
</style>
