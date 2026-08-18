<script setup lang="ts">
/**
 * 远端进程列表工作区：展示完整列表、排序、右键操作与选中进程详情。
 */
import {
  computed,
  nextTick,
  onBeforeUnmount,
  onMounted,
  reactive,
  ref,
  watch,
} from "vue";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";

import AppDialog from "./AppDialog.vue";
import Icon from "./Icon.vue";
import {
  processDetail as fetchProcessDetail,
  processList,
  processTerminate,
} from "../api";
import { hasOpenModal } from "../composables/useEscClose";
import { useMonitorStore } from "../stores/monitor";
import { useSettingsStore } from "../stores/settings";
import type { ProcessDetail, ProcessListItem } from "../types";
import { formatShort } from "../utils";

const props = defineProps<{
  /** 提供进程数据的 SSH 会话标识 */
  sessionId: string;
  /** 当前进程工作区是否处于激活状态 */
  active: boolean;
}>();

type SortKey = "pid" | "user" | "memBytes" | "cpu" | "name" | "executable";
type SortDirection = "asc" | "desc";
type CopyField = "pid" | "name" | "command" | "executable";

const settingsStore = useSettingsStore();
const monitorStore = useMonitorStore();
/** 手动刷新动画的最短展示时间 */
const MANUAL_REFRESH_MIN_DURATION = 200;

/** 最近一次读取到的完整进程列表 */
const processes = ref<ProcessListItem[]>([]);
/** 进程表格滚动容器 */
const tableWrap = ref<HTMLDivElement | null>(null);
/** 进程列表是否正在刷新 */
const loading = ref(false);
/** 是否正在执行 F5 手动刷新 */
const manualRefreshing = ref(false);
/** 进程列表读取错误 */
const error = ref("");
/** 当前选中进程的稳定身份键 */
const selectedKey = ref("");
/** 当前选中进程的详细信息 */
const detail = ref<ProcessDetail | null>(null);
/** 进程详情是否正在读取 */
const detailLoading = ref(false);
/** 进程详情读取错误 */
const detailError = ref("");
/** 工具栏短暂状态提示 */
const notice = ref("");
/** 当前排序状态，默认与参考界面一致按内存降序 */
const sortState = reactive<{ key: SortKey; direction: SortDirection }>({
  key: "memBytes",
  direction: "desc",
});

/** 右键菜单状态 */
const contextMenu = reactive<{
  open: boolean;
  x: number;
  y: number;
  process: ProcessListItem | null;
}>({ open: false, x: 0, y: 0, process: null });

/** 通用提示与确认弹窗状态 */
const dialog = reactive<{
  open: boolean;
  type: "info" | "confirm";
  title: string;
  message: string;
  confirmText: string;
  confirmDanger: boolean;
  resolve?: (confirmed: boolean) => void;
}>({
  open: false,
  type: "info",
  title: "",
  message: "",
  confirmText: "确定",
  confirmDanger: false,
});

/** 正在执行终止操作的进程身份键 */
const terminatingKey = ref("");

let pollTimer: number | null = null;
let noticeTimer: number | null = null;
let listRequestSequence = 0;
let detailRequestSequence = 0;
let currentSessionId = "";

/** 生成进程的稳定身份键，PID 复用时会得到不同键 */
function processKey(process: ProcessListItem): string {
  return `${process.pid}:${process.startTime}`;
}

/** 当前选中的进程列表条目 */
const selectedProcess = computed(() =>
  processes.value.find((process) => processKey(process) === selectedKey.value)
);
/** 当前会话的物理内存总量 */
const totalMemory = computed(
  () => monitorStore.state(props.sessionId)?.data?.memTotal ?? 0
);

/** 计算进程内存占系统物理内存的百分比 */
function processMemoryPercent(process: ProcessListItem): number {
  if (totalMemory.value <= 0) return 0;
  return Math.min(100, Math.max(0, (process.memBytes / totalMemory.value) * 100));
}

/** 比较两个文本字段，兼容数字片段并保持大小写不敏感 */
function compareText(left: string, right: string): number {
  return left.localeCompare(right, "zh-CN", { numeric: true, sensitivity: "base" });
}

/** 按当前字段比较两个进程条目 */
function compareProcesses(left: ProcessListItem, right: ProcessListItem): number {
  let result = 0;
  switch (sortState.key) {
    case "pid":
      result = left.pid - right.pid;
      break;
    case "memBytes":
      result = left.memBytes - right.memBytes;
      break;
    case "cpu":
      result = left.cpu - right.cpu;
      break;
    case "user":
      result = compareText(left.user, right.user);
      break;
    case "executable":
      result = compareText(left.executable, right.executable);
      break;
    default:
      result = compareText(left.name, right.name);
      if (result === 0) result = compareText(left.command, right.command);
  }
  if (result === 0) result = left.pid - right.pid;
  return sortState.direction === "asc" ? result : -result;
}

/** 按当前表头规则排序后的进程列表 */
const sortedProcesses = computed(() => [...processes.value].sort(compareProcesses));

/** 点击表头时切换排序字段或方向 */
function setSort(key: SortKey): void {
  if (sortState.key === key) {
    sortState.direction = sortState.direction === "asc" ? "desc" : "asc";
    return;
  }
  sortState.key = key;
  sortState.direction = "asc";
}

/** 返回当前表头的排序方向标记 */
function sortMark(key: SortKey): string {
  if (sortState.key !== key) return "";
  return sortState.direction === "asc" ? "↑" : "↓";
}

/** 返回表头无障碍排序状态 */
function ariaSort(key: SortKey): "none" | "ascending" | "descending" {
  if (sortState.key !== key) return "none";
  return sortState.direction === "asc" ? "ascending" : "descending";
}

/** 清理当前进程详情选择 */
function clearSelection(): void {
  detailRequestSequence += 1;
  selectedKey.value = "";
  detail.value = null;
  detailLoading.value = false;
  detailError.value = "";
}

/** 聚焦指定进程行，并按需将其滚动到可见区域 */
async function focusProcessRow(
  key: string,
  block?: ScrollLogicalPosition
): Promise<void> {
  await nextTick();
  const row = tableWrap.value?.querySelector<HTMLTableRowElement>(
    `tr[data-process-key="${key}"]`
  );
  if (!row) return;
  if (block) row.scrollIntoView({ block, inline: "nearest" });
  row.focus({ preventScroll: true });
}

/** 取消当前选择并将键盘焦点交还原进程行 */
function dismissSelection(): void {
  const key = selectedKey.value;
  clearSelection();
  if (key) void focusProcessRow(key);
}

/** 将当前选中的进程行快速定位到表格中部 */
function locateSelectedProcess(): void {
  if (selectedKey.value) void focusProcessRow(selectedKey.value, "center");
}

/** 查询并展示指定进程的详细信息 */
async function loadProcessDetail(process: ProcessListItem): Promise<void> {
  const key = processKey(process);
  const sessionId = props.sessionId;
  const sequence = ++detailRequestSequence;
  selectedKey.value = key;
  detail.value = null;
  detailError.value = "";
  detailLoading.value = true;
  try {
    const next = await fetchProcessDetail(sessionId, process.pid, process.startTime);
    if (
      sequence !== detailRequestSequence ||
      sessionId !== props.sessionId ||
      selectedKey.value !== key
    ) {
      return;
    }
    detail.value = next;
  } catch (reason) {
    if (
      sequence === detailRequestSequence &&
      sessionId === props.sessionId &&
      selectedKey.value === key
    ) {
      detailError.value = String(reason);
    }
  } finally {
    if (sequence === detailRequestSequence) detailLoading.value = false;
  }
}

/** 选中一项进程并按需读取详情 */
function selectProcess(process: ProcessListItem): void {
  const key = processKey(process);
  if (selectedKey.value === key && (detail.value || detailLoading.value)) return;
  void loadProcessDetail(process);
}

/** 按当前排序顺序选择相邻进程 */
function selectAdjacentProcess(process: ProcessListItem, offset: -1 | 1): void {
  const index = sortedProcesses.value.findIndex(
    (item) => processKey(item) === processKey(process)
  );
  if (index < 0) return;
  const nextIndex = Math.min(
    sortedProcesses.value.length - 1,
    Math.max(0, index + offset)
  );
  const next = sortedProcesses.value[nextIndex];
  if (!next) return;
  selectProcess(next);
  void focusProcessRow(processKey(next), "nearest");
}

/** 读取完整进程列表，并保留仍有效的当前选择 */
async function refreshProcesses(refreshSelectedDetail = false): Promise<void> {
  if (!props.sessionId || loading.value) return;
  const sessionId = props.sessionId;
  const sequence = ++listRequestSequence;
  loading.value = true;
  error.value = "";
  try {
    const next = await processList(sessionId);
    if (sequence !== listRequestSequence || sessionId !== props.sessionId) return;
    processes.value = next;
    const selected = next.find((process) => processKey(process) === selectedKey.value);
    if (!selected && selectedKey.value) {
      clearSelection();
    } else if (selected && refreshSelectedDetail) {
      await loadProcessDetail(selected);
    }
  } catch (reason) {
    if (sequence === listRequestSequence && sessionId === props.sessionId) {
      error.value = String(reason);
    }
  } finally {
    if (sequence === listRequestSequence) loading.value = false;
  }
}

/** 手动刷新进程列表，并保证刷新动画可被用户感知 */
async function refreshProcessesManually(): Promise<void> {
  if (manualRefreshing.value) return;
  manualRefreshing.value = true;
  await nextTick();
  const startedAt = performance.now();
  try {
    await refreshProcesses(true);
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

/** 停止当前进程列表轮询 */
function stopPolling(): void {
  if (pollTimer !== null) window.clearInterval(pollTimer);
  pollTimer = null;
}

/** 按应用监控间隔启动进程列表轮询 */
function startPolling(): void {
  stopPolling();
  if (!props.active || !props.sessionId) return;
  const interval = Math.max(1, settingsStore.settings.monitorInterval) * 1000;
  pollTimer = window.setInterval(() => void refreshProcesses(), interval);
}

/** 重置切换会话后不再有效的页面状态 */
function resetWorkspaceState(): void {
  listRequestSequence += 1;
  processes.value = [];
  loading.value = false;
  error.value = "";
  clearSelection();
  closeContextMenu();
}

/** 关闭进程右键菜单 */
function closeContextMenu(): void {
  contextMenu.open = false;
  contextMenu.process = null;
}

/** 在鼠标位置打开进程右键菜单，并确保菜单不超出视口 */
function openContextMenu(process: ProcessListItem, event: MouseEvent): void {
  event.preventDefault();
  selectProcess(process);
  const width = 156;
  const height = 194;
  const margin = 6;
  contextMenu.process = process;
  contextMenu.x = Math.max(margin, Math.min(event.clientX, window.innerWidth - width - margin));
  contextMenu.y = Math.max(margin, Math.min(event.clientY, window.innerHeight - height - margin));
  contextMenu.open = true;
}

/** 点击菜单外区域时关闭进程右键菜单 */
function onGlobalPointerDown(event: PointerEvent): void {
  if (!contextMenu.open) return;
  if ((event.target as HTMLElement).closest(".process-context-menu")) return;
  closeContextMenu();
}

/** 处理 F5 手动刷新与 Escape 取消当前进程选择 */
function onGlobalKeyDown(event: KeyboardEvent): void {
  if (!props.active || hasOpenModal()) return;
  if (event.key === "F5") {
    if (event.repeat) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    closeContextMenu();
    void refreshProcessesManually();
    return;
  }
  if (event.key !== "Escape") return;
  if (contextMenu.open) closeContextMenu();
  if (!selectedKey.value) return;
  event.preventDefault();
  event.stopImmediatePropagation();
  dismissSelection();
}

/** 显示短暂的工具栏状态提示 */
function showNotice(message: string): void {
  notice.value = message;
  if (noticeTimer !== null) window.clearTimeout(noticeTimer);
  noticeTimer = window.setTimeout(() => {
    notice.value = "";
    noticeTimer = null;
  }, 1600);
}

/** 显示操作结果提示弹窗 */
function showMessage(title: string, message: string): void {
  Object.assign(dialog, {
    open: true,
    type: "info",
    title,
    message,
    confirmText: "确定",
    confirmDanger: false,
    resolve: undefined,
  });
}

/** 显示危险操作确认弹窗 */
function showConfirm(title: string, message: string): Promise<boolean> {
  return new Promise((resolve) => {
    Object.assign(dialog, {
      open: true,
      type: "confirm",
      title,
      message,
      confirmText: "终止进程",
      confirmDanger: true,
      resolve,
    });
  });
}

/** 结算当前提示或确认弹窗 */
function settleDialog(confirmed: boolean): void {
  const resolve = dialog.resolve;
  dialog.open = false;
  dialog.resolve = undefined;
  resolve?.(confirmed);
}

/** 返回进程复制菜单对应的文本 */
function processCopyValue(process: ProcessListItem, field: CopyField): string {
  if (field === "pid") return String(process.pid);
  if (field === "name") return process.name;
  if (field === "command") return process.command;
  if (selectedKey.value === processKey(process) && detail.value?.executable) {
    return detail.value.executable;
  }
  return process.executable;
}

/** 将文本写入系统剪贴板，浏览器预览时回退到 Web 剪贴板 */
async function copyText(value: string): Promise<void> {
  try {
    await writeText(value);
  } catch {
    await navigator.clipboard.writeText(value);
  }
}

/** 执行右键菜单中的字段复制操作 */
async function copyProcessField(field: CopyField): Promise<void> {
  const process = contextMenu.process;
  closeContextMenu();
  if (!process) return;
  const value = processCopyValue(process, field);
  if (!value) {
    showMessage("无法复制", "当前进程没有可复制的对应信息");
    return;
  }
  try {
    await copyText(value);
    showNotice("已复制到剪贴板");
  } catch (reason) {
    showMessage("复制失败", String(reason));
  }
}

/** 请求确认后向目标进程发送 SIGTERM */
async function requestTerminate(): Promise<void> {
  const process = contextMenu.process;
  closeContextMenu();
  if (!process || terminatingKey.value) return;
  const confirmed = await showConfirm(
    "终止进程",
    `确定要向进程 ${process.pid}（${process.name || "未命名进程"}）发送终止信号吗？`
  );
  if (!confirmed) return;

  const key = processKey(process);
  const sessionId = props.sessionId;
  terminatingKey.value = key;
  try {
    await processTerminate(sessionId, process.pid, process.startTime);
    if (sessionId === props.sessionId) {
      showNotice(`已向进程 ${process.pid} 发送终止信号`);
      await refreshProcesses();
    }
  } catch (reason) {
    if (sessionId === props.sessionId) showMessage("终止进程失败", String(reason));
  } finally {
    if (terminatingKey.value === key) terminatingKey.value = "";
  }
}

/** 详情区域显示的进程名称 */
const detailName = computed(() => detail.value?.name || selectedProcess.value?.name || "-");
/** 详情区域显示的完整命令行 */
const detailCommand = computed(
  () => detail.value?.command || selectedProcess.value?.command || "-"
);
/** 详情区域显示的可执行文件位置 */
const detailExecutable = computed(
  () => detail.value?.executable || selectedProcess.value?.executable || "-"
);
/** 详情区域显示的工作目录 */
const detailWorkingDirectory = computed(() => detail.value?.workingDirectory || "-");

watch(
  () => [props.sessionId, props.active, settingsStore.settings.monitorInterval] as const,
  ([sessionId, active]) => {
    const sessionChanged = sessionId !== currentSessionId;
    if (sessionChanged) {
      currentSessionId = sessionId;
      resetWorkspaceState();
    }
    stopPolling();
    if (!active) {
      closeContextMenu();
      return;
    }
    void refreshProcesses();
    startPolling();
  },
  { immediate: true }
);

onMounted(() => {
  window.addEventListener("pointerdown", onGlobalPointerDown);
  window.addEventListener("keydown", onGlobalKeyDown, true);
  window.addEventListener("resize", closeContextMenu);
  window.addEventListener("scroll", closeContextMenu, true);
});

onBeforeUnmount(() => {
  listRequestSequence += 1;
  detailRequestSequence += 1;
  stopPolling();
  if (noticeTimer !== null) window.clearTimeout(noticeTimer);
  window.removeEventListener("pointerdown", onGlobalPointerDown);
  window.removeEventListener("keydown", onGlobalKeyDown, true);
  window.removeEventListener("resize", closeContextMenu);
  window.removeEventListener("scroll", closeContextMenu, true);
  settleDialog(false);
});
</script>

<template>
  <div class="process-workspace" :class="{ 'detail-open': selectedProcess }">
    <header class="process-toolbar">
      <div class="toolbar-title">
        <Icon name="activity" :size="14" />
        <span>进程列表</span>
        <span class="process-count">{{ processes.length }}</span>
      </div>
      <span
        v-if="manualRefreshing"
        class="toolbar-refresh"
        role="status"
        aria-label="正在刷新"
        title="正在刷新"
      >
        <Icon class="manual-refresh-icon" name="refresh" :size="13" />
      </span>
      <span v-else-if="notice" class="toolbar-status success">{{ notice }}</span>
      <span v-else-if="error" class="toolbar-status error" :title="error">{{ error }}</span>
    </header>

    <div ref="tableWrap" class="process-table-wrap" @scroll="closeContextMenu">
      <table class="process-table">
        <colgroup>
          <col class="col-pid" />
          <col class="col-user" />
          <col class="col-memory" />
          <col class="col-cpu" />
          <col class="col-command" />
          <col class="col-location" />
        </colgroup>
        <thead>
          <tr>
            <th :aria-sort="ariaSort('pid')">
              <button type="button" @click="setSort('pid')">PID <span>{{ sortMark("pid") }}</span></button>
            </th>
            <th :aria-sort="ariaSort('user')">
              <button type="button" @click="setSort('user')">用户 <span>{{ sortMark("user") }}</span></button>
            </th>
            <th :aria-sort="ariaSort('memBytes')">
              <button type="button" @click="setSort('memBytes')">内存 <span>{{ sortMark("memBytes") }}</span></button>
            </th>
            <th :aria-sort="ariaSort('cpu')">
              <button type="button" @click="setSort('cpu')">CPU <span>{{ sortMark("cpu") }}</span></button>
            </th>
            <th :aria-sort="ariaSort('name')">
              <button type="button" @click="setSort('name')">名称 / 命令行 <span>{{ sortMark("name") }}</span></button>
            </th>
            <th :aria-sort="ariaSort('executable')">
              <button type="button" @click="setSort('executable')">位置 <span>{{ sortMark("executable") }}</span></button>
            </th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="(process, index) in sortedProcesses"
            :key="processKey(process)"
            :data-process-key="processKey(process)"
            :class="{ selected: selectedKey === processKey(process) }"
            :tabindex="
              selectedKey === processKey(process) || (!selectedKey && index === 0) ? 0 : -1
            "
            @click="selectProcess(process)"
            @keydown.enter.prevent="selectProcess(process)"
            @keydown.up.prevent.stop="selectAdjacentProcess(process, -1)"
            @keydown.down.prevent.stop="selectAdjacentProcess(process, 1)"
            @contextmenu="openContextMenu(process, $event)"
          >
            <td class="numeric mono">{{ process.pid }}</td>
            <td :title="process.user">{{ process.user || "-" }}</td>
            <td class="numeric mono usage-cell">
              <span
                v-if="process.memBytes > 0 && totalMemory > 0"
                class="usage-bar memory"
                :style="{ width: `${processMemoryPercent(process)}%` }"
                aria-hidden="true"
              ></span>
              <span class="usage-value">{{ formatShort(process.memBytes) }}</span>
            </td>
            <td class="numeric mono usage-cell">
              <span
                v-if="process.cpu > 0"
                class="usage-bar cpu"
                :style="{ width: `${Math.min(100, Math.max(0, process.cpu))}%` }"
                aria-hidden="true"
              ></span>
              <span class="usage-value">{{ process.cpu.toFixed(1) }}</span>
            </td>
            <td :title="`${process.name} | ${process.command}`">
              <div class="command-cell">
                <span class="process-name">{{ process.name || "-" }}</span>
                <span class="command-divider">|</span>
                <span class="process-command">{{ process.command || "-" }}</span>
              </div>
            </td>
            <td class="ellipsis" :title="process.executable">{{ process.executable || "-" }}</td>
          </tr>
          <tr v-if="!sortedProcesses.length">
            <td colspan="6" class="table-state">
              {{ loading ? "正在读取进程列表..." : error ? "进程列表读取失败" : "暂无进程数据" }}
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <section v-if="selectedProcess" class="process-detail">
      <header class="detail-header">
        <span class="detail-title">{{ `${selectedProcess.pid} - ${detailName}` }}</span>
        <div class="detail-actions">
          <span v-if="detailLoading" class="detail-state">正在读取</span>
          <span v-else-if="detailError" class="detail-state error" :title="detailError">
            详情不可用
          </span>
          <button
            type="button"
            class="detail-action"
            title="定位当前选中进程"
            aria-label="定位当前选中进程"
            @click="locateSelectedProcess"
          >
            <Icon name="locate" :size="14" />
          </button>
          <button
            type="button"
            class="detail-action"
            title="关闭进程详情"
            aria-label="关闭进程详情"
            @click="dismissSelection"
          >
            <Icon name="close" :size="14" />
          </button>
        </div>
      </header>

      <div class="detail-content">
        <div class="detail-main">
          <div class="detail-inline-fields">
            <label>
              <span>PID</span>
              <input
                class="detail-value mono"
                :value="selectedProcess.pid"
                readonly
                aria-label="PID"
              />
            </label>
            <label>
              <span>名称</span>
              <input
                class="detail-value"
                :value="detailName"
                readonly
                aria-label="名称"
              />
            </label>
          </div>
          <label class="detail-row">
            <span>位置</span>
            <input
              class="detail-value mono"
              :value="detailExecutable"
              :title="detailExecutable"
              readonly
              aria-label="位置"
            />
          </label>
          <label class="detail-row">
            <span>工作目录</span>
            <input
              class="detail-value mono"
              :value="detailWorkingDirectory"
              :title="detailWorkingDirectory"
              readonly
              aria-label="工作目录"
            />
          </label>
          <textarea
            class="command-detail mono"
            :value="detailCommand"
            :title="detailCommand"
            readonly
            spellcheck="false"
            aria-label="完整命令行"
          ></textarea>
          <p v-if="detailError" class="detail-error">{{ detailError }}</p>
        </div>

        <div class="environment-panel">
          <table>
            <thead>
              <tr><th>环境变量</th><th>变量值</th></tr>
            </thead>
            <tbody>
              <tr
                v-for="(variable, index) in detail?.environment ?? []"
                :key="`${variable.name}:${index}`"
              >
                <td class="mono" :title="variable.name">{{ variable.name }}</td>
                <td class="mono" :title="variable.value">{{ variable.value }}</td>
              </tr>
              <tr v-if="!detailLoading && !(detail?.environment.length)">
                <td colspan="2" class="environment-empty">无可读环境变量</td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </section>

    <div
      v-if="contextMenu.open"
      class="process-context-menu"
      :style="{ left: `${contextMenu.x}px`, top: `${contextMenu.y}px` }"
      @click.stop
    >
      <button type="button" class="danger" :disabled="Boolean(terminatingKey)" @click="requestTerminate">
        终止进程
      </button>
      <div class="menu-separator"></div>
      <button type="button" @click="copyProcessField('pid')">复制 PID</button>
      <button type="button" @click="copyProcessField('name')">复制名称</button>
      <button type="button" @click="copyProcessField('command')">复制命令行</button>
      <button type="button" @click="copyProcessField('executable')">复制位置</button>
    </div>

    <AppDialog
      :open="dialog.open"
      :type="dialog.type"
      :title="dialog.title"
      :message="dialog.message"
      :confirm-text="dialog.confirmText"
      :confirm-danger="dialog.confirmDanger"
      @confirm="settleDialog(true)"
      @cancel="settleDialog(false)"
    />
  </div>
</template>

<style scoped>
.process-workspace {
  display: grid;
  grid-template-rows: 34px minmax(0, 1fr);
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  background: var(--bg-window);
  color: var(--text);
}

.process-workspace.detail-open {
  grid-template-rows: 34px minmax(180px, 1fr) minmax(190px, 38%);
}

.process-toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
  padding: 0 8px 0 10px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-panel-alt);
}

.toolbar-title {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  color: var(--text-secondary);
  font-weight: 600;
}

.toolbar-title .icon {
  color: var(--accent);
}

.process-count {
  min-width: 24px;
  padding: 1px 5px;
  border: 1px solid var(--border);
  border-radius: 3px;
  background: #fff;
  color: var(--text-muted);
  font-size: 10px;
  line-height: 15px;
  text-align: center;
}

.toolbar-refresh {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  margin-left: auto;
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

.toolbar-status {
  min-width: 0;
  margin-left: auto;
  overflow: hidden;
  color: var(--text-muted);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.toolbar-status.success {
  color: var(--success);
}

.toolbar-status.error {
  color: var(--danger);
}

.process-table-wrap {
  min-width: 0;
  min-height: 0;
  overflow: auto;
  border-bottom: 1px solid var(--border);
  background: #fff;
}

.process-table {
  width: 100%;
  min-width: 960px;
  border-collapse: collapse;
  table-layout: fixed;
  font-size: 11px;
}

.col-pid { width: 82px; }
.col-user { width: 112px; }
.col-memory { width: 88px; }
.col-cpu { width: 72px; }
.col-command { width: 39%; }
.col-location { width: 29%; }

.process-table thead {
  position: sticky;
  top: 0;
  z-index: 2;
}

.process-table th {
  height: 27px;
  padding: 0;
  border-right: 1px solid var(--border);
  border-bottom: 1px solid var(--border);
  background: linear-gradient(var(--table-head-top), var(--table-head-bottom));
  color: var(--text-secondary);
  font-weight: 600;
  text-align: left;
}

.process-table th:last-child,
.process-table td:last-child {
  border-right: none;
}

.process-table th button {
  display: flex;
  align-items: center;
  justify-content: space-between;
  width: 100%;
  height: 100%;
  padding: 0 7px;
  border: none;
  background: transparent;
  color: inherit;
  font: inherit;
  letter-spacing: 0;
  cursor: pointer;
}

.process-table th button:hover {
  background: #dfe9f3;
  color: var(--accent);
}

.process-table th button span {
  width: 10px;
  color: var(--accent);
  text-align: right;
}

.process-table td {
  height: 24px;
  padding: 3px 7px;
  overflow: hidden;
  border-right: 1px solid var(--border-light);
  border-bottom: 1px solid var(--border-light);
  color: var(--text-secondary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.process-table tbody tr:nth-child(even) td {
  background: #f7f9fb;
}

.process-table tbody tr:hover td {
  background: var(--row-hover);
}

.process-table tbody tr.selected td {
  background: #d8e8f5;
}

.process-table tbody tr:focus {
  outline: none;
}

.process-table tbody tr:focus td:first-child {
  box-shadow: inset 2px 0 0 var(--accent);
}

.numeric {
  text-align: right;
}

.process-table td.usage-cell {
  position: relative;
  padding-top: 0;
  padding-bottom: 0;
}

.usage-bar {
  position: absolute;
  top: 0;
  right: 0;
  bottom: 0;
  min-width: 2px;
  pointer-events: none;
}

.usage-bar.memory {
  background: rgba(224, 168, 104, 0.55);
}

.usage-bar.cpu {
  background: rgba(126, 196, 106, 0.55);
}

.usage-value {
  position: relative;
  z-index: 1;
}

.mono {
  font-family: "Cascadia Mono", "Consolas", monospace;
}

.ellipsis {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.command-cell {
  display: flex;
  align-items: center;
  min-width: 0;
  gap: 5px;
}

.process-name {
  max-width: 34%;
  flex: 0 1 auto;
  overflow: hidden;
  color: var(--text);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.command-divider {
  flex: 0 0 auto;
  color: #b3bac1;
}

.process-command {
  min-width: 0;
  flex: 1 1 auto;
  overflow: hidden;
  color: var(--text-secondary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.table-state {
  height: 96px !important;
  color: var(--text-muted) !important;
  text-align: center !important;
}

.process-detail {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  background: #fff;
}

.detail-header {
  display: flex;
  align-items: center;
  gap: 10px;
  height: 29px;
  min-width: 0;
  flex: 0 0 auto;
  padding: 0 9px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-panel-alt);
  color: var(--text-secondary);
  font-weight: 600;
}

.detail-title {
  min-width: 0;
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.detail-actions {
  display: flex;
  align-items: center;
  gap: 3px;
  min-width: 0;
  flex: 0 0 auto;
}

.detail-state {
  max-width: 120px;
  margin-right: 4px;
  overflow: hidden;
  color: var(--text-muted);
  font-size: 10px;
  font-weight: 400;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.detail-state.error {
  color: var(--danger);
}

.detail-action {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 23px;
  flex: 0 0 auto;
  padding: 0;
  border: 1px solid transparent;
  border-radius: 3px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
}

.detail-action:hover {
  border-color: var(--border);
  background: var(--row-hover);
  color: var(--accent);
}

.detail-action:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}

.detail-content {
  display: grid;
  grid-template-columns: minmax(380px, 2fr) minmax(280px, 1fr);
  min-width: 0;
  min-height: 0;
  flex: 1 1 auto;
  overflow: hidden;
}

.detail-main {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
  min-height: 0;
  padding: 7px 9px;
  overflow: auto;
  border-right: 1px solid var(--border);
}

.detail-inline-fields {
  display: grid;
  grid-template-columns: minmax(130px, 0.7fr) minmax(220px, 1.3fr);
  gap: 8px;
}

.detail-inline-fields label,
.detail-row {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  align-items: center;
  gap: 6px;
  min-width: 0;
  color: var(--text-secondary);
}

.detail-value {
  box-sizing: border-box;
  width: 100%;
  min-width: 0;
  height: 24px;
  padding: 3px 6px;
  overflow: hidden;
  border: 1px solid var(--border);
  background: #fff;
  color: var(--text);
  caret-color: var(--text);
  cursor: text;
  font-family: inherit;
  font-size: inherit;
  letter-spacing: 0;
  line-height: 16px;
  outline: none;
  text-overflow: ellipsis;
  white-space: nowrap;
  user-select: text;
}

.detail-value.mono {
  font-family: "Cascadia Mono", "Consolas", monospace;
}

.detail-value:focus,
.command-detail:focus {
  border-color: var(--accent);
  box-shadow: 0 0 0 1px rgba(74, 127, 171, 0.2);
}

.command-detail {
  box-sizing: border-box;
  width: 100%;
  min-height: 48px;
  flex: 1 1 auto;
  padding: 7px;
  overflow: auto;
  border: 1px solid var(--border-light);
  background: #fff;
  color: var(--text-secondary);
  caret-color: var(--text);
  cursor: text;
  font-family: "Cascadia Mono", "Consolas", monospace;
  font-size: 11px;
  letter-spacing: 0;
  line-height: 1.5;
  outline: none;
  resize: none;
  white-space: pre-wrap;
  word-break: break-word;
  user-select: text;
}

.detail-error {
  margin: 0;
  color: var(--danger);
  font-size: 11px;
  user-select: text;
}

.environment-panel {
  min-width: 0;
  min-height: 0;
  overflow: auto;
}

.environment-panel table {
  width: 100%;
  min-width: 420px;
  border-collapse: collapse;
  table-layout: fixed;
  font-size: 11px;
}

.environment-panel th {
  position: sticky;
  top: 0;
  z-index: 1;
  width: 34%;
  height: 25px;
  padding: 3px 7px;
  border-right: 1px solid var(--border);
  border-bottom: 1px solid var(--border);
  background: linear-gradient(var(--table-head-top), var(--table-head-bottom));
  color: var(--text-secondary);
  text-align: left;
}

.environment-panel th:last-child {
  width: 66%;
  border-right: none;
}

.environment-panel td {
  height: 23px;
  padding: 3px 7px;
  overflow: hidden;
  border-right: 1px solid var(--border-light);
  border-bottom: 1px solid var(--border-light);
  color: var(--text-secondary);
  text-overflow: ellipsis;
  white-space: nowrap;
  user-select: text;
  cursor: text;
}

.environment-panel td:last-child {
  border-right: none;
}

.environment-panel tbody tr:nth-child(even) td {
  background: #f7f9fb;
}

.environment-empty {
  color: var(--text-muted) !important;
  text-align: center;
}

.process-context-menu {
  position: fixed;
  z-index: 1300;
  width: 156px;
  padding: 4px 0;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: #fff;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.18);
}

.process-context-menu button {
  display: block;
  width: 100%;
  height: 30px;
  padding: 0 14px;
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-family: inherit;
  font-size: 12px;
  letter-spacing: 0;
  text-align: left;
  cursor: pointer;
}

.process-context-menu button:hover:not(:disabled) {
  background: var(--row-hover);
}

.process-context-menu button.danger {
  color: var(--danger);
}

.process-context-menu button:disabled {
  color: #b8bec4;
  cursor: default;
}

.menu-separator {
  height: 1px;
  margin: 3px 0;
  background: var(--border-light);
}

@media (max-width: 820px) {
  .process-workspace.detail-open {
    grid-template-rows: 34px minmax(170px, 1fr) minmax(220px, 44%);
  }

  .detail-content {
    display: block;
    overflow: auto;
  }

  .detail-main {
    min-height: 150px;
    border-right: none;
    border-bottom: 1px solid var(--border);
  }

  .environment-panel {
    min-height: 120px;
  }
}
</style>
