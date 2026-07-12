<script setup lang="ts">
/**
 * 左侧系统监控面板：展示当前激活会话的监控数据
 * 数据来源于 monitor store（按会话持续采集），切换选项卡不清空不重采
 * 未连接时展示占位骨架，避免面板空荡
 */
import { computed, ref } from "vue";
import Icon from "./Icon.vue";
import type { ConnectionConfig, ProcessInfo } from "../types";
import { useMonitorStore } from "../stores/monitor";
import { formatShort, formatShortRate, formatUptime } from "../utils";

const props = defineProps<{
  /** 当前会话标识 */
  sessionId: string;
  /** 会话是否已连接 */
  connected: boolean;
  /** 当前连接配置，用于显示 IP */
  config?: ConnectionConfig;
}>();

const monitor = useMonitorStore();

/** 当前会话的监控状态 */
const st = computed(() => monitor.state(props.sessionId));
/** 监控数据 */
const data = computed(() => st.value?.data ?? null);
/** 采集错误 */
const error = computed(() => st.value?.error ?? "");
/** 当前选中网卡名 */
const netName = computed(() => st.value?.netName ?? "");

/** 进程排序字段：按 CPU 或内存占用降序 */
const sortKey = ref<"cpu" | "mem">("cpu");

/** IP 复制反馈 */
const copied = ref(false);

/** 复制 IP 到剪贴板 */
async function copyIp() {
  const ip = props.config?.host || data.value?.hostname || "";
  if (!ip) return;
  try {
    await navigator.clipboard.writeText(ip);
  } catch {
    // 剪贴板 API 不可用时用兜底方案
    const ta = document.createElement("textarea");
    ta.value = ip;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    document.body.removeChild(ta);
  }
  copied.value = true;
  window.setTimeout(() => (copied.value = false), 1200);
}

/** 切换网卡 */
function onNetChange(e: Event) {
  monitor.setNetName(props.sessionId, (e.target as HTMLSelectElement).value);
}

/** 内存使用率百分比 */
function memPercent(): number {
  const d = data.value;
  if (!d || d.memTotal === 0) return 0;
  return (d.memUsed / d.memTotal) * 100;
}

/** 交换区使用率百分比 */
function swapPercent(): number {
  const d = data.value;
  if (!d || d.swapTotal === 0) return 0;
  return (d.swapUsed / d.swapTotal) * 100;
}

/** 依据占用率返回进度条颜色 */
function usageColor(percent: number): string {
  if (percent >= 90) return "var(--progress-danger)";
  if (percent >= 60) return "var(--progress-mem)";
  return "var(--progress-cpu)";
}

/** 内存用量文案 */
function memText(): string {
  const d = data.value;
  if (!d) return "0/0";
  return `${formatShort(d.memUsed)}/${formatShort(d.memTotal)}`;
}

/** 交换用量文案 */
function swapText(): string {
  const d = data.value;
  if (!d) return "0/0";
  return `${formatShort(d.swapUsed)}/${formatShort(d.swapTotal)}`;
}

/** 按当前排序字段降序排列的进程列表（取前 6） */
const sortedProcesses = computed<ProcessInfo[]>(() => {
  const list = data.value?.processes ?? [];
  return [...list].sort((a, b) => b[sortKey.value] - a[sortKey.value]).slice(0, 6);
});

/** 当前选中网卡对象 */
function currentNet() {
  return data.value?.netInterfaces.find((n) => n.name === netName.value);
}

/**
 * 网络图数据：以当前网卡收发速率历史中的峰值为纵轴上限，
 * 每个采样点给出上传/下载两根柱子的高度百分比（0-100）
 */
const netChart = computed(() => {
  const hist = st.value?.netHistories[netName.value] ?? [];
  const peak = Math.max(1, ...hist.map((s) => Math.max(s.rx, s.tx)));
  const bars = hist.map((s) => ({
    rx: Math.max((s.rx / peak) * 100, s.rx > 0 ? 4 : 0),
    tx: Math.max((s.tx / peak) * 100, s.tx > 0 ? 4 : 0),
  }));
  // 纵轴四档刻度：顶部峰值、2/3、1/3、底部 0
  const axis = [peak, (peak * 2) / 3, peak / 3, 0];
  return { bars, axis };
});
</script>

<template>
  <!-- 骨架始终渲染，data 为空时显示占位值 -->
  <div class="sidebar">
    <!-- IP 行（含复制按钮） -->
    <div class="ip-row">
      IP <span class="ip-val">{{ config?.host || data?.hostname || "-" }}</span>
      <button class="copy-btn" title="复制 IP" @click="copyIp">
        <Icon name="copy" :size="13" />
      </button>
      <transition name="fade">
        <span v-if="copied" class="copied-tip">已复制</span>
      </transition>
    </div>

    <!-- 系统信息按钮 -->
    <div class="btn-sysinfo" :class="{ disabled: !data }" :title="data?.os || ''">
      系统信息
    </div>

    <!-- 运行 / 负载 -->
    <div class="stat-line">
      运行 <b>{{ data ? formatUptime(data.uptime) : "-" }}</b>
    </div>
    <div class="stat-line">
      负载
      <b>{{ data ? data.loadAvg.map((n) => n.toFixed(2)).join(", ") : "-" }}</b>
    </div>

    <!-- CPU -->
    <div class="meter">
      <span class="label">CPU</span>
      <div class="track">
        <div
          class="fill"
          :style="{ width: (data?.cpuUsage ?? 0) + '%', background: usageColor(data?.cpuUsage ?? 0) }"
        ></div>
        <span class="pct">{{ (data?.cpuUsage ?? 0).toFixed(0) }}%</span>
      </div>
    </div>
    <!-- 内存 -->
    <div class="meter">
      <span class="label">内存</span>
      <div class="track">
        <div
          class="fill"
          :style="{ width: memPercent() + '%', background: usageColor(memPercent()) }"
        ></div>
        <span class="pct">{{ memPercent().toFixed(0) }}%</span>
        <span class="right-text">{{ memText() }}</span>
      </div>
    </div>
    <!-- 交换 -->
    <div class="meter">
      <span class="label">交换</span>
      <div class="track">
        <div
          class="fill"
          :style="{ width: swapPercent() + '%', background: usageColor(swapPercent()) }"
        ></div>
        <span class="pct">{{ swapPercent().toFixed(0) }}%</span>
        <span class="right-text">{{ swapText() }}</span>
      </div>
    </div>

    <!-- 进程表：内存/CPU 列窄且带占用背景条，表头可点击排序 -->
    <div class="mini-table proc-table">
      <table>
        <colgroup>
          <col style="width: 24%" />
          <col style="width: 20%" />
          <col />
        </colgroup>
        <thead>
          <tr>
            <th
              class="sortable"
              :class="{ active: sortKey === 'mem' }"
              @click="sortKey = 'mem'"
            >
              内存
            </th>
            <th
              class="sortable"
              :class="{ active: sortKey === 'cpu' }"
              @click="sortKey = 'cpu'"
            >
              CPU
            </th>
            <th>命令</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="p in sortedProcesses" :key="p.pid">
            <td class="bar-cell">
              <div
                class="cell-bar mem"
                :style="{ width: Math.min(p.mem, 100) + '%' }"
              ></div>
              <span class="cell-val">{{ formatShort(p.memBytes) }}</span>
            </td>
            <td class="bar-cell">
              <div
                class="cell-bar cpu"
                :style="{ width: Math.min(p.cpu, 100) + '%' }"
              ></div>
              <span class="cell-val">{{ p.cpu.toFixed(1) }}</span>
            </td>
            <td class="ellipsis cmd" :title="p.name">{{ p.name }}</td>
          </tr>
          <tr v-if="!sortedProcesses.length" class="placeholder-row">
            <td colspan="3"></td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 网络 -->
    <div class="net-head">
      <span class="up">↑ {{ data ? formatShortRate(currentNet()?.txRate ?? 0) : "-" }}</span>
      <span class="down">↓ {{ data ? formatShortRate(currentNet()?.rxRate ?? 0) : "-" }}</span>
      <select
        v-if="data && data.netInterfaces.length"
        class="iface"
        :value="netName"
        :title="netName"
        @change="onNetChange"
      >
        <option v-for="n in data.netInterfaces" :key="n.name" :value="n.name">
          {{ n.name }}{{ n.isPhysical ? "" : " (虚拟)" }}
        </option>
      </select>
      <span v-else class="iface-empty">-</span>
    </div>
    <div class="net-chart">
      <!-- 纵轴刻度 -->
      <div class="net-axis">
        <span v-for="(v, i) in netChart.axis" :key="i">{{ formatShort(v) }}</span>
      </div>
      <!-- 柱状图区：柱宽固定，右对齐使新数据从右向左推入 -->
      <div class="net-bars">
        <!-- 中间两档虚线刻度（1/3、2/3 高度处） -->
        <div class="grid-line" style="bottom: 33.33%"></div>
        <div class="grid-line" style="bottom: 66.66%"></div>
        <div class="net-col" v-for="(b, i) in netChart.bars" :key="i">
          <div class="bar tx" :style="{ height: b.tx + '%' }"></div>
          <div class="bar rx" :style="{ height: b.rx + '%' }"></div>
        </div>
      </div>
    </div>

    <!-- 磁盘表 -->
    <div class="mini-table disk-table">
      <table>
        <colgroup>
          <col />
          <col style="width: 118px" />
        </colgroup>
        <thead>
          <tr><th>路径</th><th style="text-align:right">可用/大小</th></tr>
        </thead>
        <tbody>
          <tr v-for="d in data?.disks ?? []" :key="d.mount">
            <td class="ellipsis" :title="d.mount">{{ d.mount }}</td>
            <td class="bar-cell">
              <div class="cell-bar disk" :style="{ width: Math.min(d.usePercent, 100) + '%' }"></div>
              <span class="cell-val">{{ formatShort(d.available) }}/{{ formatShort(d.total) }}</span>
            </td>
          </tr>
          <tr v-if="!data?.disks.length" class="placeholder-row">
            <td colspan="2"></td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 采集错误提示 -->
    <div v-if="error" class="mon-error">{{ error }}</div>
  </div>
</template>

<style scoped>
.sidebar {
  height: 100%;
  overflow-y: auto;
  overflow-x: hidden;
  background: var(--bg-panel);
  display: flex;
  flex-direction: column;
}

.ip-row {
  display: flex;
  align-items: center;
  padding: 8px 10px 6px;
  color: #444;
}
.ip-row .ip-val {
  margin-left: 6px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.copy-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 20px;
  margin-left: 6px;
  border: none;
  border-radius: 3px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  flex-shrink: 0;
}
.copy-btn:hover {
  background: var(--row-hover);
  color: var(--accent);
}
.copied-tip {
  margin-left: 4px;
  font-size: 11px;
  color: var(--success);
}
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.2s;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

.btn-sysinfo {
  margin: 2px 10px 10px;
  padding: 7px 0;
  text-align: center;
  background: linear-gradient(#5e86ad, #4a739c);
  color: #fff;
  border-radius: 3px;
  font-size: 13px;
  letter-spacing: 2px;
  cursor: pointer;
}
.btn-sysinfo:hover {
  background: linear-gradient(#6a92b9, #547fab);
}
.btn-sysinfo.disabled {
  background: #cdd5dd;
  color: #eef2f5;
  cursor: default;
}

.stat-line {
  padding: 3px 10px;
  color: #3a3f45;
}
.stat-line b {
  font-weight: 600;
}

/* 进度条 */
.meter {
  display: flex;
  align-items: center;
  padding: 3px 10px;
  gap: 8px;
}
.meter .label {
  width: 34px;
  color: #4a4f55;
}
.meter .track {
  position: relative;
  flex: 1;
  height: 15px;
  background: var(--progress-track);
  border: 1px solid #d0d6db;
  border-radius: 2px;
  overflow: hidden;
}
.meter .fill {
  height: 100%;
  transition: width 0.4s ease;
}
.meter .track .pct {
  position: absolute;
  left: 6px;
  top: 50%;
  transform: translateY(-50%);
  font-size: 11px;
  color: #3a3f45;
}
.meter .track .right-text {
  position: absolute;
  right: 5px;
  top: 50%;
  transform: translateY(-50%);
  font-size: 11px;
  color: #3a3f45;
}

/* 通用小表格（进程/磁盘） */
.mini-table {
  margin: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 3px;
  background: #fff;
  overflow: hidden;
}
.mini-table table {
  width: 100%;
  border-collapse: collapse;
  font-size: 11px;
  table-layout: fixed;
}
.mini-table th {
  background: linear-gradient(#eef3f8, #dde5ee);
  color: #3a3f45;
  font-weight: 600;
  padding: 4px 6px;
  border-bottom: 1px solid var(--border);
  border-right: 1px solid var(--border);
  text-align: left;
}
.mini-table th:last-child {
  border-right: none;
}
.mini-table td {
  padding: 3px 6px;
  border-bottom: 1px solid var(--border-light);
  color: #444;
}
.mini-table td.num {
  text-align: right;
  color: #666;
  width: 38%;
}
.mini-table tr:last-child td {
  border-bottom: none;
}
.mini-table tbody tr:nth-child(even) td {
  background: #f7f9fb;
}
.mini-table .placeholder-row td {
  height: 60px;
  background: #fff !important;
}
.ellipsis {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 进程表专属：可排序表头 */
.proc-table th.sortable {
  cursor: pointer;
  user-select: none;
}
.proc-table th.sortable:hover {
  color: var(--accent);
}
.proc-table th.sortable.active {
  background: linear-gradient(#dbe8f5, #c5d8ec);
  color: var(--accent);
}
/* 单元格占用背景条（进程内存/CPU、磁盘可用大小共用） */
.mini-table td.bar-cell {
  position: relative;
  text-align: right;
  padding: 0 6px;
  height: 18px;
}
/* 占用条：锚定右侧，从右向左延伸；min-width 保证极低占用也可见成一条线 */
.mini-table .cell-bar {
  position: absolute;
  right: 0;
  top: 0;
  bottom: 0;
  min-width: 2px;
  z-index: 0;
}
.mini-table .cell-bar.cpu {
  background: rgba(126, 196, 106, 0.55);
}
.mini-table .cell-bar.mem {
  background: rgba(224, 168, 104, 0.55);
}
.mini-table .cell-bar.disk {
  background: rgba(126, 158, 234, 0.5);
}
.mini-table .cell-val {
  position: relative;
  z-index: 1;
  color: #555;
}
.proc-table td.cmd {
  color: #444;
}

/* 网络 */
.net-head {
  display: flex;
  align-items: center;
  padding: 4px 10px 2px;
  font-size: 11px;
  gap: 8px;
}
/* 上下行速率不换行、不压缩 */
.net-head .up,
.net-head .down {
  white-space: nowrap;
  flex-shrink: 0;
}
.net-head .up {
  color: #e07b3c;
}
.net-head .down {
  color: #3b8f4b;
}
/* 网卡下拉：随名称自适应宽度，面板变窄时收缩并省略号，展开列表仍显全名 */
.net-head .iface {
  margin-left: auto;
  min-width: 0;
  max-width: 110px;
  color: #444;
  font-size: 11px;
  border: 1px solid var(--border);
  border-radius: 2px;
  background: #fff;
  outline: none;
  padding: 1px 2px;
  text-overflow: ellipsis;
}
.net-head .iface-empty {
  margin-left: auto;
  color: var(--text-muted);
}
.net-chart {
  margin: 2px 10px 6px;
  height: 76px;
  background: #fff;
  border: 1px solid var(--border);
  border-radius: 3px;
  overflow: hidden;
  display: flex;
  padding: 3px 3px 3px 0;
}
/* 纵轴刻度列 */
.net-axis {
  flex: 0 0 auto;
  width: 34px;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  padding: 1px 3px 1px 4px;
  font-size: 9px;
  color: #9aa0a6;
  text-align: right;
  line-height: 1;
}
/* 柱状图区：右对齐，历史增长时从右向左推入 */
.net-bars {
  position: relative;
  flex: 1;
  display: flex;
  align-items: flex-end;
  justify-content: flex-end;
  gap: 2px;
  min-width: 0;
  overflow: hidden;
  border-left: 1px solid var(--border-light);
  padding-left: 2px;
}
/* 中间档虚线刻度 */
.net-bars .grid-line {
  position: absolute;
  left: 0;
  right: 0;
  height: 0;
  border-top: 1px dashed #e3e7eb;
  z-index: 0;
  pointer-events: none;
}
/* 单个采样点：固定宽度，上传/下载两根柱并排 */
.net-col {
  position: relative;
  z-index: 1;
  flex: 0 0 7px;
  display: flex;
  align-items: flex-end;
  justify-content: center;
  gap: 1px;
  height: 100%;
}
.net-col .bar {
  width: 3px;
  border-radius: 1px 1px 0 0;
}
.net-col .bar.tx {
  background: #e6924a;
}
.net-col .bar.rx {
  background: #6ab97a;
}

.mon-error {
  margin: 6px 10px;
  padding: 6px 8px;
  font-size: 11px;
  color: var(--danger);
  background: #fbeaea;
  border: 1px solid #f0c9c9;
  border-radius: 3px;
}
</style>
