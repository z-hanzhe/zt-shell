<script setup lang="ts">
/**
 * 远端系统信息视图：展示当前会话的完整系统信息与动态 CPU、网络数据
 */
import { computed, type ComponentPublicInstance } from "vue";
import { useDialogDrag } from "../composables/useDialogDrag";
import { useEscClose } from "../composables/useEscClose";
import type { MonitorData, NetInterface } from "../types";
import { formatBytes, formatRate } from "../utils";

const props = withDefaults(
  defineProps<{
    /** 当前会话最新一次监控数据 */
    data: MonitorData;
    /** 是否嵌入主工作区，默认保持原弹窗形态 */
    embedded?: boolean;
  }>(),
  { embedded: false }
);

const emit = defineEmits<{
  (e: "close"): void;
}>();

/** 弹窗模式下的拖动控制器，嵌入工作区时不注册窗口级拖动监听 */
const dialogDrag = props.embedded ? null : useDialogDrag();

/** 弹窗打开时的静态信息快照 */
const staticData = {
  ...props.data,
  fileSystems: props.data.fileSystems.map((item) => ({ ...item })),
};

/** 内核名称与版本合并后的显示文本 */
const kernelVersion =
  [staticData.kernelName, staticData.kernel].filter((part) => part.trim()).join(" ") || "-";

/** CPU 各类别占用项 */
const cpuUsageItems = computed(() => [
  { label: "用户", value: props.data.cpuUsageBreakdown.user },
  { label: "系统", value: props.data.cpuUsageBreakdown.system },
  { label: "Nice", value: props.data.cpuUsageBreakdown.nice },
  { label: "空闲", value: props.data.cpuUsageBreakdown.idle },
  { label: "I/O 等待", value: props.data.cpuUsageBreakdown.ioWait },
  { label: "硬件中断", value: props.data.cpuUsageBreakdown.irq },
  { label: "软件中断", value: props.data.cpuUsageBreakdown.softIrq },
  {
    label: "窃取",
    value: props.data.cpuUsageBreakdown.steal,
    title: "虚拟机被宿主机占用的时间",
  },
]);

/** 将缺失的文本字段统一显示为占位符 */
function displayText(value: string): string {
  return value.trim() || "-";
}

/** 将占用率格式化为固定一位小数 */
function formatPercent(value: number): string {
  return `${Number.isFinite(value) ? value.toFixed(1) : "0.0"}%`;
}

/** 格式化 CPU 频率 */
function formatFrequency(value: number): string {
  return value > 0 && Number.isFinite(value) ? `${value.toFixed(1)} MHz` : "-";
}

/** 格式化 CPU BogoMips */
function formatBogoMips(value: number): string {
  return value > 0 && Number.isFinite(value) ? value.toFixed(2) : "-";
}

/** 计算内存或交换区的占用率 */
function memoryPercent(used: number, total: number): string {
  return formatPercent(total > 0 ? (used / total) * 100 : 0);
}

/** 计算内存或交换区剩余容量 */
function remainingBytes(used: number, total: number): number {
  return Math.max(total - used, 0);
}

/** 返回网络接口类别文案 */
function interfaceType(item: NetInterface): string {
  if (item.name === "lo") return "回环";
  return item.isPhysical ? "物理" : "虚拟";
}

// 组件挂载即为打开状态，ESC 关闭
const { isTop: isTopModal } = useEscClose(
  () => !props.embedded,
  () => emit("close")
);

/** 仅在弹窗模式下绑定可拖动容器 */
function setDialogElement(element: Element | ComponentPublicInstance | null) {
  if (!dialogDrag) return;
  dialogDrag.dialogRef.value = element instanceof HTMLElement ? element : null;
}

/** 仅在弹窗模式下响应标题栏拖动 */
function onDialogHeaderPointerDown(event: PointerEvent) {
  dialogDrag?.onDialogHeaderPointerDown(event);
}
</script>

<template>
  <div
    :class="embedded ? 'system-info-page' : ['modal-mask', { 'modal-top-mask': isTopModal }]"
    :inert="!embedded && !isTopModal"
    :aria-hidden="!embedded && !isTopModal ? 'true' : undefined"
  >
    <div
      :ref="setDialogElement"
      :class="embedded ? 'system-info-page-content' : 'modal dialog-draggable system-info-dialog'"
      :role="embedded ? undefined : 'dialog'"
      :aria-modal="embedded ? undefined : isTopModal ? 'true' : 'false'"
      :aria-labelledby="embedded ? undefined : 'system-info-title'"
    >
      <div
        v-if="!embedded"
        class="modal-header dialog-drag-handle"
        @pointerdown="onDialogHeaderPointerDown"
      >
        <span id="system-info-title">系统信息 - {{ displayText(staticData.hostname) }}</span>
        <button class="modal-close" title="关闭" @click="emit('close')">×</button>
      </div>

      <div class="modal-body system-info-body">
        <section class="overview" aria-label="基本信息">
          <dl class="overview-grid">
            <div class="overview-item">
              <dt>操作系统</dt>
              <dd :title="staticData.os">{{ displayText(staticData.os) }}</dd>
            </div>
            <div class="overview-item">
              <dt>主机名称</dt>
              <dd :title="staticData.hostname">{{ displayText(staticData.hostname) }}</dd>
            </div>
            <div class="overview-item">
              <dt>内核版本</dt>
              <dd :title="kernelVersion">{{ kernelVersion }}</dd>
            </div>
            <div class="overview-item">
              <dt>硬件架构</dt>
              <dd>{{ displayText(staticData.architecture) }}</dd>
            </div>
          </dl>
        </section>

        <section class="info-section" aria-labelledby="cpu-heading">
          <h2 id="cpu-heading" class="section-title">CPU</h2>
          <div class="table-scroll">
            <table class="info-table cpu-table">
              <colgroup>
                <col class="cpu-name-col" />
                <col class="cpu-number-col" />
                <col class="cpu-frequency-col" />
                <col class="cpu-cache-col" />
                <col class="cpu-bogo-col" />
              </colgroup>
              <thead>
                <tr>
                  <th>名称</th>
                  <th class="numeric">逻辑核心</th>
                  <th class="numeric">频率</th>
                  <th class="numeric">缓存</th>
                  <th class="numeric">BogoMips</th>
                </tr>
              </thead>
              <tbody>
                <tr>
                  <td class="ellipsis" :title="staticData.cpuModel">{{ displayText(staticData.cpuModel) }}</td>
                  <td class="numeric">{{ staticData.cpuCount || "-" }}</td>
                  <td class="numeric">{{ formatFrequency(data.cpuFrequencyMhz) }}</td>
                  <td class="numeric">{{ staticData.cpuCache > 0 ? formatBytes(staticData.cpuCache) : "-" }}</td>
                  <td class="numeric">{{ formatBogoMips(staticData.cpuBogoMips) }}</td>
                </tr>
              </tbody>
            </table>
          </div>
        </section>

        <section class="info-section" aria-labelledby="cpu-usage-heading">
          <div class="section-heading">
            <h2 id="cpu-usage-heading" class="section-title">CPU 占用</h2>
            <span class="usage-total">总占用 {{ formatPercent(data.cpuUsage) }}</span>
          </div>
          <div class="cpu-usage-grid">
            <div v-for="item in cpuUsageItems" :key="item.label" class="cpu-usage-item">
              <span :title="item.title">{{ item.label }}</span>
              <strong>{{ formatPercent(item.value) }}</strong>
            </div>
          </div>
        </section>

        <section class="info-section" aria-labelledby="memory-heading">
          <h2 id="memory-heading" class="section-title">内存与交换</h2>
          <div class="table-scroll">
            <table class="info-table memory-table">
              <thead>
                <tr>
                  <th>类型</th>
                  <th class="numeric">总量</th>
                  <th class="numeric">已用</th>
                  <th class="numeric">占用</th>
                  <th class="numeric">剩余</th>
                </tr>
              </thead>
              <tbody>
                <tr>
                  <td>内存</td>
                  <td class="numeric">{{ formatBytes(data.memTotal) }}</td>
                  <td class="numeric">{{ formatBytes(data.memUsed) }}</td>
                  <td class="numeric">{{ memoryPercent(data.memUsed, data.memTotal) }}</td>
                  <td class="numeric">{{ formatBytes(data.memAvailable) }}</td>
                </tr>
                <tr>
                  <td>交换区</td>
                  <td class="numeric">{{ formatBytes(data.swapTotal) }}</td>
                  <td class="numeric">{{ formatBytes(data.swapUsed) }}</td>
                  <td class="numeric">{{ memoryPercent(data.swapUsed, data.swapTotal) }}</td>
                  <td class="numeric">{{ formatBytes(remainingBytes(data.swapUsed, data.swapTotal)) }}</td>
                </tr>
              </tbody>
            </table>
          </div>
        </section>

        <section class="info-section" aria-labelledby="network-heading">
          <h2 id="network-heading" class="section-title">网络接口</h2>
          <div class="table-scroll">
            <table class="info-table network-table">
              <colgroup>
                <col class="network-name-col" />
                <col class="network-type-col" />
                <col class="network-total-col" />
                <col class="network-total-col" />
                <col class="network-rate-col" />
                <col class="network-rate-col" />
              </colgroup>
              <thead>
                <tr>
                  <th>名称</th>
                  <th>类型</th>
                  <th class="numeric">发送</th>
                  <th class="numeric">接收</th>
                  <th class="numeric">发送速度</th>
                  <th class="numeric">接收速度</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="item in data.netInterfaces" :key="item.name">
                  <td class="ellipsis" :title="item.name">{{ item.name }}</td>
                  <td>{{ interfaceType(item) }}</td>
                  <td class="numeric">{{ formatBytes(item.txTotal) }}</td>
                  <td class="numeric">{{ formatBytes(item.rxTotal) }}</td>
                  <td class="numeric rate-up">{{ formatRate(item.txRate) }}</td>
                  <td class="numeric rate-down">{{ formatRate(item.rxRate) }}</td>
                </tr>
                <tr v-if="data.netInterfaces.length === 0" class="empty-row">
                  <td colspan="6">暂无网络接口</td>
                </tr>
              </tbody>
            </table>
          </div>
        </section>

        <section class="info-section file-system-section" aria-labelledby="file-system-heading">
          <h2 id="file-system-heading" class="section-title">文件系统</h2>
          <div class="table-scroll">
            <table class="info-table file-system-table">
              <colgroup>
                <col class="file-system-name-col" />
                <col class="file-system-number-col" />
                <col class="file-system-number-col" />
                <col class="file-system-number-col" />
                <col class="file-system-percent-col" />
                <col class="file-system-mount-col" />
              </colgroup>
              <thead>
                <tr>
                  <th>名称</th>
                  <th class="numeric">大小</th>
                  <th class="numeric">已用</th>
                  <th class="numeric">可用</th>
                  <th class="numeric">占用</th>
                  <th>挂载点</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="(item, index) in staticData.fileSystems" :key="`${item.filesystem}:${item.mount}:${index}`">
                  <td class="ellipsis" :title="item.filesystem">{{ item.filesystem }}</td>
                  <td class="numeric">{{ formatBytes(item.total) }}</td>
                  <td class="numeric">{{ formatBytes(item.used) }}</td>
                  <td class="numeric">{{ formatBytes(item.available) }}</td>
                  <td class="numeric">{{ formatPercent(item.usePercent) }}</td>
                  <td class="ellipsis" :title="item.mount">{{ item.mount }}</td>
                </tr>
                <tr v-if="staticData.fileSystems.length === 0" class="empty-row">
                  <td colspan="6">暂无文件系统</td>
                </tr>
              </tbody>
            </table>
          </div>
        </section>
      </div>
    </div>
  </div>
</template>

<style scoped>
.system-info-page,
.system-info-page-content {
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
}

.system-info-page {
  overflow: hidden;
  background: var(--bg-window);
}

.system-info-page-content {
  display: flex;
  flex-direction: column;
}

.system-info-page-content > .system-info-body {
  flex: 1 1 auto;
}

.system-info-dialog {
  width: min(1040px, calc(100vw - 32px));
  height: min(720px, calc(100vh - var(--titlebar-height) - 16px));
  max-height: calc(100vh - var(--titlebar-height) - 16px);
  min-width: 0;
}

.system-info-dialog .modal-header > span {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.system-info-dialog > .modal-header {
  flex: 0 0 38px;
}

.system-info-body {
  min-height: 0;
  padding: 0;
  overflow-x: hidden;
  background: var(--bg-window);
}

.overview,
.info-section {
  padding: 13px 18px;
  border-bottom: 1px solid var(--border-light);
}

.file-system-section {
  border-bottom: 0;
}

.overview {
  background: #fbfcfd;
}

.overview-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px 28px;
  margin: 0;
}

.overview-item {
  display: grid;
  grid-template-columns: 76px minmax(0, 1fr);
  align-items: baseline;
  min-width: 0;
}

.overview-item dt {
  color: var(--text-muted);
}

.overview-item dd {
  min-width: 0;
  margin: 0;
  overflow: hidden;
  color: var(--text);
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.section-heading {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 8px;
}

.section-title {
  margin: 0 0 8px;
  color: var(--text-secondary);
  font-size: 12px;
  font-weight: 600;
}

.section-heading .section-title {
  margin-bottom: 0;
}

.usage-total {
  color: var(--accent);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.table-scroll {
  width: 100%;
  overflow-x: auto;
  border: 1px solid var(--border);
  border-radius: var(--radius);
}

.info-table {
  width: 100%;
  border-collapse: collapse;
  table-layout: fixed;
  color: var(--text-secondary);
  font-size: 11px;
}

.info-table th,
.info-table td {
  height: 27px;
  padding: 4px 8px;
  overflow: hidden;
  border-right: 1px solid var(--border-light);
  border-bottom: 1px solid var(--border-light);
  text-align: left;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.info-table th {
  background: linear-gradient(var(--table-head-top), var(--table-head-bottom));
  color: var(--text-secondary);
  font-weight: 600;
}

.info-table tbody tr:nth-child(even) td {
  background: #f8fafb;
}

.info-table th:last-child,
.info-table td:last-child {
  border-right: 0;
}

.info-table tbody tr:last-child td {
  border-bottom: 0;
}

.info-table .numeric {
  text-align: right;
  font-variant-numeric: tabular-nums;
}

.info-table .ellipsis {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cpu-table {
  min-width: 720px;
}

.cpu-name-col {
  width: 42%;
}

.cpu-number-col {
  width: 10%;
}

.cpu-frequency-col,
.cpu-cache-col,
.cpu-bogo-col {
  width: 16%;
}

.cpu-usage-grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  overflow: hidden;
  border: 1px solid var(--border);
  border-radius: var(--radius);
}

.cpu-usage-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  min-width: 0;
  height: 30px;
  padding: 5px 9px;
  border-right: 1px solid var(--border-light);
  border-bottom: 1px solid var(--border-light);
  color: var(--text-muted);
}

.cpu-usage-item:nth-child(4n) {
  border-right: 0;
}

.cpu-usage-item:nth-last-child(-n + 4) {
  border-bottom: 0;
}

.cpu-usage-item strong {
  color: var(--text-secondary);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.cpu-usage-item > span {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.memory-table {
  min-width: 600px;
}

.memory-table th:first-child,
.memory-table td:first-child {
  width: 20%;
}

.network-table {
  min-width: 780px;
}

.network-name-col {
  width: 22%;
}

.network-type-col {
  width: 10%;
}

.network-total-col {
  width: 17%;
}

.network-rate-col {
  width: 17%;
}

.rate-up {
  color: #c96f32;
}

.rate-down {
  color: var(--success);
}

.file-system-table {
  min-width: 900px;
}

.file-system-name-col {
  width: 18%;
}

.file-system-number-col {
  width: 12%;
}

.file-system-percent-col {
  width: 10%;
}

.file-system-mount-col {
  width: 36%;
}

.empty-row td {
  height: 42px;
  color: var(--text-muted);
  text-align: center;
}

@media (max-width: 680px) {
  .system-info-dialog {
    width: calc(100vw - 16px);
    height: calc(100vh - var(--titlebar-height) - 16px);
    max-height: calc(100vh - var(--titlebar-height) - 16px);
  }

  .overview,
  .info-section {
    padding-right: 12px;
    padding-left: 12px;
  }

  .overview-grid {
    grid-template-columns: minmax(0, 1fr);
  }

  .cpu-usage-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .cpu-usage-item:nth-child(4n) {
    border-right: 1px solid var(--border-light);
  }

  .cpu-usage-item:nth-child(2n) {
    border-right: 0;
  }

  .cpu-usage-item:nth-last-child(-n + 4) {
    border-bottom: 1px solid var(--border-light);
  }

  .cpu-usage-item:nth-last-child(-n + 2) {
    border-bottom: 0;
  }
}
</style>
