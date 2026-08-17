<script setup lang="ts">
/**
 * 系统信息工作区：按会话读取监控数据，并在主工作区内展示完整系统信息。
 */
import { computed } from "vue";
import Icon from "./Icon.vue";
import SystemInfoDialog from "./SystemInfoDialog.vue";
import { useMonitorStore } from "../stores/monitor";

const props = defineProps<{
  /** 提供系统信息的 SSH 会话标识 */
  sessionId: string;
}>();

const monitor = useMonitorStore();

/** 当前会话最新一次监控数据 */
const data = computed(() => monitor.state(props.sessionId)?.data ?? null);
</script>

<template>
  <div class="system-info-workspace">
    <SystemInfoDialog v-if="data" :data="data" embedded />
    <div v-else class="system-info-unavailable">
      <Icon name="server" :size="40" />
      <p>当前会话暂无可用的系统信息</p>
    </div>
  </div>
</template>

<style scoped>
.system-info-workspace {
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  background: var(--bg-window);
}

.system-info-unavailable {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100%;
  gap: 14px;
  color: var(--text-muted);
}

.system-info-unavailable p {
  margin: 0;
}
</style>
