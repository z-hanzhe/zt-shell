<script setup lang="ts">
/**
 * 右下面板：顶部选项卡栏（文件管理器 + 传输列表）+ 内容区，
 * 传输选项卡在有执行中任务时显示数量角标（上限 99）
 */
import { computed, ref } from "vue";
import FileManager from "./FileManager.vue";
import TransferPanel from "./TransferPanel.vue";
import { useTransfersStore } from "../stores/transfers";

const props = defineProps<{
  /** 当前会话标识 */
  sessionId: string;
  /** 会话是否已连接 */
  connected: boolean;
}>();

const emit = defineEmits<{
  (e: "sync-terminal-path", path: string): void;
  (e: "sync-file-path"): void;
}>();

const transfersStore = useTransfersStore();

/** 文件管理器组件引用，用于外部同步路径 */
const fileManagerRef = ref<InstanceType<typeof FileManager>>();

/** 选项卡定义 */
const tabs = [
  { key: "files", label: "文件" },
  { key: "transfers", label: "传输" },
];
/** 当前激活选项卡 */
const activeTab = ref("files");

/** 传输角标数量：当前会话执行中的文件任务数（上限 99） */
const transferBadge = computed(() => {
  const count = transfersStore.tasks.filter(
    (t) =>
      t.sessionId === props.sessionId &&
      !t.isDir &&
      (t.status === "pending" || t.status === "running" || t.status === "packing")
  ).length;
  return Math.min(count, 99);
});

/** 根据终端当前目录更新文件管理器路径 */
function setFilePath(path: string) {
  activeTab.value = "files";
  return fileManagerRef.value?.setPathFromTerminal(path);
}

defineExpose({ setFilePath });
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
        <span v-if="t.key === 'transfers' && transferBadge > 0" class="tab-badge">
          {{ transferBadge }}
        </span>
      </div>
    </div>
    <div class="tab-content">
      <FileManager
        ref="fileManagerRef"
        v-show="activeTab === 'files'"
        :session-id="sessionId"
        :connected="connected"
        @sync-terminal-path="emit('sync-terminal-path', $event)"
        @sync-file-path="emit('sync-file-path')"
      />
      <TransferPanel v-show="activeTab === 'transfers'" :session-id="sessionId" />
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
  position: relative;
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
/* 传输任务数量角标 */
.tab-badge {
  position: absolute;
  top: 0;
  right: 0;
  min-width: 14px;
  height: 13px;
  padding: 0 3px;
  border-radius: 7px;
  background: var(--danger);
  color: #fff;
  font-size: 10px;
  line-height: 13px;
  text-align: center;
}
.tab-content {
  flex: 1;
  overflow: hidden;
}
</style>
