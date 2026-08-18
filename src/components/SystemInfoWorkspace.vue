<script setup lang="ts">
/**
 * 系统信息工作区：按会话读取监控数据，并在主工作区内展示完整系统信息。
 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import Icon from "./Icon.vue";
import SystemInfoDialog from "./SystemInfoDialog.vue";
import { hasOpenModal } from "../composables/useEscClose";
import { useMonitorStore } from "../stores/monitor";

const props = defineProps<{
  /** 提供系统信息的 SSH 会话标识 */
  sessionId: string;
  /** 当前系统信息工作区是否处于激活状态 */
  active: boolean;
}>();

const monitor = useMonitorStore();
/** 手动刷新动画的最短展示时间 */
const MANUAL_REFRESH_MIN_DURATION = 200;
/** 是否正在执行 F5 手动刷新 */
const manualRefreshing = ref(false);

/** 当前会话最新一次监控数据 */
const data = computed(() => monitor.state(props.sessionId)?.data ?? null);

/** 手动采集一次系统信息，并保证刷新动画可被用户感知 */
async function refreshManually(): Promise<void> {
  if (manualRefreshing.value) return;
  manualRefreshing.value = true;
  await nextTick();
  const startedAt = performance.now();
  try {
    await monitor.refresh(props.sessionId);
  } finally {
    const remaining = Math.ceil(
      MANUAL_REFRESH_MIN_DURATION - (performance.now() - startedAt)
    );
    if (remaining > 0) {
      await new Promise<void>((resolve) => window.setTimeout(resolve, remaining));
    }
    manualRefreshing.value = false;
  }
}

/** 在激活的系统信息页通过 F5 触发一次监控采集 */
function onGlobalKeyDown(event: KeyboardEvent): void {
  if (
    event.key !== "F5" ||
    event.repeat ||
    !props.active ||
    hasOpenModal()
  ) {
    return;
  }
  event.preventDefault();
  event.stopImmediatePropagation();
  void refreshManually();
}

onMounted(() => window.addEventListener("keydown", onGlobalKeyDown, true));
onBeforeUnmount(() => window.removeEventListener("keydown", onGlobalKeyDown, true));
</script>

<template>
  <div class="system-info-workspace">
    <span
      v-if="manualRefreshing"
      class="manual-refresh-indicator"
      role="status"
      aria-label="正在刷新"
      title="正在刷新"
    >
      <Icon class="manual-refresh-icon" name="refresh" :size="15" />
    </span>
    <SystemInfoDialog v-if="data" :data="data" embedded />
    <div v-else class="system-info-unavailable">
      <Icon name="server" :size="40" />
      <p>当前会话暂无可用的系统信息</p>
    </div>
  </div>
</template>

<style scoped>
.system-info-workspace {
  position: relative;
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  background: var(--bg-window);
}

.manual-refresh-indicator {
  position: absolute;
  top: 8px;
  right: 18px;
  z-index: 4;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  color: var(--accent);
  pointer-events: none;
}

.manual-refresh-icon {
  animation: manual-refresh-spin 0.7s linear infinite;
}

@keyframes manual-refresh-spin {
  to {
    transform: rotate(360deg);
  }
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
