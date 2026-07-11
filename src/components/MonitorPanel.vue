<script setup lang="ts">
/**
 * 左侧系统监控面板：定时采集远端监控数据并按 FinalShell 布局分模块展示
 */
import { onBeforeUnmount, ref, watch } from "vue";
import { monitorCollect } from "../api";
import type { MonitorData, ConnectionConfig } from "../types";
import { useSettingsStore } from "../stores/settings";
import { formatShort, formatShortRate, formatUptime } from "../utils";

const props = defineProps<{
  /** 当前会话标识 */
  sessionId: string;
  /** 会话是否已连接 */
  connected: boolean;
  /** 当前连接配置，用于显示 IP */
  config?: ConnectionConfig;
}>();

const settings = useSettingsStore();

/** 监控数据 */
const data = ref<MonitorData | null>(null);
/** 是否采集出错 */
const error = ref("");
/** 当前选中网卡索引 */
const netIndex = ref(0);
/** 网卡速率历史（用于柱状图，最多 40 个采样点） */
const netHistory = ref<number[]>([]);
/** 采集定时器 */
let timer: number | null = null;

/** 执行一次采集 */
async function collect() {
  if (!props.sessionId || !props.connected) return;
  try {
    const d = await monitorCollect(props.sessionId);
    data.value = d;
    error.value = "";
    // 记录当前网卡收发速率之和到历史
    const net = d.netInterfaces[netIndex.value];
    if (net) {
      netHistory.value.push(net.rxRate + net.txRate);
      if (netHistory.value.length > 40) netHistory.value.shift();
    }
  } catch (e) {
    error.value = String(e);
  }
}

/** 启动定时采集 */
function start() {
  stop();
  collect();
  timer = window.setInterval(collect, settings.settings.monitorInterval * 1000);
}

/** 停止采集 */
function stop() {
  if (timer !== null) {
    window.clearInterval(timer);
    timer = null;
  }
}

/** 内存使用率百分比 */
function memPercent(): number {
  if (!data.value || data.value.memTotal === 0) return 0;
  return (data.value.memUsed / data.value.memTotal) * 100;
}

/** 交换区使用率百分比 */
function swapPercent(): number {
  if (!data.value || data.value.swapTotal === 0) return 0;
  return (data.value.swapUsed / data.value.swapTotal) * 100;
}

/** 依据占用率返回进度条颜色 */
function usageColor(percent: number): string {
  if (percent >= 90) return "var(--progress-danger)";
  if (percent >= 60) return "var(--progress-mem)";
  return "var(--progress-cpu)";
}

/** 网络柱状图各柱高度百分比 */
function netBarHeights(): number[] {
  const max = Math.max(...netHistory.value, 1);
  return netHistory.value.map((v) => Math.max((v / max) * 100, 2));
}

/** 当前选中网卡 */
function currentNet() {
  return data.value?.netInterfaces[netIndex.value];
}

// 会话状态变化时启停采集
watch(
  () => [props.sessionId, props.connected] as const,
  ([id, conn]) => {
    data.value = null;
    error.value = "";
    netHistory.value = [];
    netIndex.value = 0;
    if (id && conn) {
      start();
    } else {
      stop();
    }
  },
  { immediate: true }
);

onBeforeUnmount(stop);
</script>

<template>
  <div class="sidebar">
    <template v-if="connected && data">
      <!-- IP 行 -->
      <div class="ip-row">
        IP <span class="ip-val">{{ config?.host || data.hostname }}</span>
      </div>

      <!-- 系统信息按钮（展示主机名/系统） -->
      <div class="btn-sysinfo" :title="data.os">系统信息</div>

      <!-- 运行 / 负载 -->
      <div class="stat-line">运行 <b>{{ formatUptime(data.uptime) }}</b></div>
      <div class="stat-line">
        负载 <b>{{ data.loadAvg.map((n) => n.toFixed(2)).join(", ") }}</b>
      </div>

      <!-- CPU -->
      <div class="meter">
        <span class="label">CPU</span>
        <div class="track">
          <div class="fill" :style="{ width: data.cpuUsage + '%', background: usageColor(data.cpuUsage) }"></div>
          <span class="pct">{{ data.cpuUsage.toFixed(0) }}%</span>
        </div>
      </div>
      <!-- 内存 -->
      <div class="meter">
        <span class="label">内存</span>
        <div class="track">
          <div class="fill" :style="{ width: memPercent() + '%', background: usageColor(memPercent()) }"></div>
          <span class="pct">{{ memPercent().toFixed(0) }}%</span>
          <span class="right-text">{{ formatShort(data.memUsed) }}/{{ formatShort(data.memTotal) }}</span>
        </div>
      </div>
      <!-- 交换 -->
      <div class="meter" v-if="data.swapTotal > 0">
        <span class="label">交换</span>
        <div class="track">
          <div class="fill" :style="{ width: swapPercent() + '%', background: usageColor(swapPercent()) }"></div>
          <span class="pct">{{ swapPercent().toFixed(0) }}%</span>
          <span class="right-text">{{ formatShort(data.swapUsed) }}/{{ formatShort(data.swapTotal) }}</span>
        </div>
      </div>

      <!-- 进程表 -->
      <div class="mini-table" v-if="data.processes.length">
        <table>
          <thead>
            <tr><th>内存</th><th>CPU</th><th>命令</th></tr>
          </thead>
          <tbody>
            <tr v-for="p in data.processes.slice(0, 6)" :key="p.pid">
              <td class="num">{{ p.mem.toFixed(1) }}%</td>
              <td class="num">{{ p.cpu.toFixed(1) }}</td>
              <td class="ellipsis" :title="p.name">{{ p.name }}</td>
            </tr>
          </tbody>
        </table>
      </div>

      <!-- 网络 -->
      <template v-if="data.netInterfaces.length">
        <div class="net-head">
          <span class="up">↑ {{ formatShortRate(currentNet()?.txRate ?? 0) }}</span>
          <span class="down">↓ {{ formatShortRate(currentNet()?.rxRate ?? 0) }}</span>
          <select class="iface" v-model.number="netIndex">
            <option v-for="(n, i) in data.netInterfaces" :key="n.name" :value="i">
              {{ n.name }}
            </option>
          </select>
        </div>
        <div class="net-chart">
          <div class="bar" v-for="(h, i) in netBarHeights()" :key="i" :style="{ height: h + '%' }"></div>
        </div>
      </template>

      <!-- 磁盘表 -->
      <div class="mini-table" v-if="data.disks.length">
        <table>
          <thead>
            <tr><th>路径</th><th style="text-align:right">可用/大小</th></tr>
          </thead>
          <tbody>
            <tr v-for="d in data.disks" :key="d.mount">
              <td class="ellipsis" :title="d.mount">{{ d.mount }}</td>
              <td class="num">{{ formatShort(d.available) }}/{{ formatShort(d.total) }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </template>

    <!-- 未连接 / 采集中 -->
    <div v-else class="mon-empty">
      <span v-if="error" class="err">{{ error }}</span>
      <span v-else-if="connected">采集中…</span>
      <span v-else>连接后显示监控信息</span>
    </div>
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
  text-align: left;
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
.mini-table tr:nth-child(even) td {
  background: #f7f9fb;
}
.ellipsis {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 网络 */
.net-head {
  display: flex;
  align-items: center;
  padding: 4px 10px 2px;
  font-size: 11px;
  gap: 10px;
}
.net-head .up {
  color: #e07b3c;
}
.net-head .down {
  color: #3b8f4b;
}
.net-head .iface {
  margin-left: auto;
  color: #444;
  font-size: 11px;
  border: 1px solid var(--border);
  border-radius: 2px;
  background: #fff;
  outline: none;
  padding: 1px 2px;
}
.net-chart {
  margin: 2px 10px 6px;
  height: 56px;
  background: #fff;
  border: 1px solid var(--border);
  border-radius: 3px;
  overflow: hidden;
  display: flex;
  align-items: flex-end;
  gap: 1px;
  padding: 2px;
}
.net-chart .bar {
  flex: 1;
  background: #f3c19a;
  min-width: 2px;
}

.mon-empty {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  color: var(--text-muted);
  font-size: 12px;
  padding: 20px;
  text-align: center;
}
.mon-empty .err {
  color: var(--danger);
}
</style>
