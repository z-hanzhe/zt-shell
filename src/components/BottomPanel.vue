<script setup lang="ts">
/**
 * 右下面板：顶部选项卡栏（默认文件管理器选项卡）+ 内容区
 */
import { ref } from "vue";
import FileManager from "./FileManager.vue";

defineProps<{
  /** 当前会话标识 */
  sessionId: string;
  /** 会话是否已连接 */
  connected: boolean;
}>();

/** 选项卡定义，当前内置文件管理器，后续可扩展 */
const tabs = [{ key: "files", label: "文件" }];
/** 当前激活选项卡 */
const activeTab = ref("files");
</script>

<template>
  <div class="bottom-panel">
    <div class="file-tabs">
      <div
        v-for="t in tabs"
        :key="t.key"
        :class="['ftab', { active: activeTab === t.key }]"
        @click="activeTab = t.key"
      >
        {{ t.label }}
      </div>
    </div>
    <div class="tab-content">
      <FileManager
        v-show="activeTab === 'files'"
        :session-id="sessionId"
        :connected="connected"
      />
    </div>
  </div>
</template>

<style scoped>
.bottom-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg-window);
}
.file-tabs {
  display: flex;
  align-items: center;
  height: 26px;
  background: var(--bg-panel-alt);
  border-bottom: 1px solid var(--border);
  padding: 0 4px;
  flex: 0 0 auto;
}
.ftab {
  padding: 4px 16px;
  font-size: 12px;
  color: #555;
  border: 1px solid transparent;
  border-bottom: none;
  cursor: pointer;
}
.ftab.active {
  background: #fff;
  border-color: var(--border);
  color: #222;
  border-radius: 3px 3px 0 0;
}
.tab-content {
  flex: 1;
  overflow: hidden;
}
</style>
