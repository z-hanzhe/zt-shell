<script setup lang="ts">
/**
 * 传输面板：上传/下载任务列表，文件夹任务支持展开收起，
 * 支持 Ctrl/Shift 多选与框选，右键菜单提供暂停/继续/删除/重试操作
 */
import { computed, onBeforeUnmount, onMounted, reactive, ref } from "vue";
import Icon from "./Icon.vue";
import {
  transferPause,
  transferRemove,
  transferResume,
  transferRetryFailed,
} from "../api";
import { useTransfersStore } from "../stores/transfers";
import type { TransferStatus, TransferTask } from "../types";
import { formatDuration, formatRate, formatSizeFixed } from "../utils";

const transfersStore = useTransfersStore();

/** 已展开的目录任务标识 */
const expandedIds = ref<Set<string>>(new Set());
/** 选中任务标识集合 */
const selectedIds = ref<Set<string>>(new Set());
/** 多选锚点，用于 Shift 范围选择 */
const selectionAnchor = ref("");
/** 列表滚动容器 */
const listRef = ref<HTMLElement | null>(null);
/** 鼠标框选状态 */
const marquee = reactive({ active: false, x: 0, y: 0, width: 0, height: 0 });
/** 右键菜单状态 */
const contextMenu = reactive({ open: false, x: 0, y: 0 });

/** 指针交互状态（点击选择或拖动框选） */
let pointerAction: PointerAction | null = null;

type PointerAction = {
  task?: TransferTask;
  startX: number;
  startY: number;
  moved: boolean;
  baseSelected: Set<string>;
  toggle: boolean;
};
type MenuAction =
  | "pause"
  | "pauseAll"
  | "resume"
  | "resumeAll"
  | "remove"
  | "removeAll"
  | "retryFailed";
type MenuItem = { action: MenuAction; label: string; disabled: boolean };
type ColumnKey =
  | "name"
  | "status"
  | "progress"
  | "size"
  | "localPath"
  | "kind"
  | "remotePath"
  | "speed"
  | "eta"
  | "elapsed";
type Row = { task: TransferTask; depth: number; hasChildren: boolean };

const columns: { key: ColumnKey; label: string }[] = [
  { key: "name", label: "文件名称" },
  { key: "status", label: "传输状态" },
  { key: "progress", label: "传输进度" },
  { key: "size", label: "文件大小" },
  { key: "localPath", label: "本地路径" },
  { key: "kind", label: "操作类型" },
  { key: "remotePath", label: "远程路径" },
  { key: "speed", label: "传输速度" },
  { key: "eta", label: "预计剩余" },
  { key: "elapsed", label: "经过时间" },
];

/** 各列宽度 */
const columnWidths = reactive<Record<ColumnKey, number>>({
  name: 220,
  status: 72,
  progress: 96,
  size: 150,
  localPath: 190,
  kind: 72,
  remotePath: 190,
  speed: 92,
  eta: 84,
  elapsed: 84,
});

/** 各列最小宽度 */
const COLUMN_MIN_WIDTHS: Record<ColumnKey, number> = {
  name: 120,
  status: 60,
  progress: 70,
  size: 100,
  localPath: 110,
  kind: 60,
  remotePath: 110,
  speed: 70,
  eta: 70,
  elapsed: 70,
};

const CONTEXT_MENU_WIDTH = 148;
const CONTEXT_MENU_HEIGHT = 184;
const CONTEXT_MENU_MARGIN = 8;

/** 父任务标识到子任务列表的映射（保持后端顺序） */
const childrenMap = computed(() => {
  const map = new Map<string, TransferTask[]>();
  for (const task of transfersStore.tasks) {
    const key = task.parentId ?? "";
    const list = map.get(key);
    if (list) list.push(task);
    else map.set(key, [task]);
  }
  return map;
});

/** 扁平化后的可见行（按展开状态展开树） */
const rows = computed<Row[]>(() => {
  const out: Row[] = [];
  const walk = (parentKey: string, depth: number) => {
    for (const task of childrenMap.value.get(parentKey) ?? []) {
      const hasChildren = (childrenMap.value.get(task.id)?.length ?? 0) > 0;
      out.push({ task, depth, hasChildren });
      if (hasChildren && expandedIds.value.has(task.id)) walk(task.id, depth + 1);
    }
  };
  walk("", 0);
  return out;
});

/** 当前选中的任务列表 */
const selectedTasks = computed(() =>
  transfersStore.tasks.filter((t) => selectedIds.value.has(t.id))
);

/** 判断任务是否处于可暂停状态 */
function isPausable(status: TransferStatus): boolean {
  return status === "pending" || status === "running" || status === "packing";
}

/** 右键菜单项 */
const contextMenuItems = computed<MenuItem[]>(() => {
  const all = transfersStore.tasks;
  return [
    { action: "pause", label: "暂停", disabled: !selectedTasks.value.some((t) => isPausable(t.status)) },
    { action: "pauseAll", label: "全部暂停", disabled: !all.some((t) => isPausable(t.status)) },
    { action: "resume", label: "继续", disabled: !selectedTasks.value.some((t) => t.status === "paused") },
    { action: "resumeAll", label: "全部继续", disabled: !all.some((t) => t.status === "paused") },
    { action: "remove", label: "删除", disabled: selectedTasks.value.length === 0 },
    { action: "removeAll", label: "全部删除", disabled: all.length === 0 },
    { action: "retryFailed", label: "重试失败的作业", disabled: !all.some((t) => t.status === "failed") },
  ];
});

/** 状态展示文案 */
function statusText(task: TransferTask): string {
  switch (task.status) {
    case "running":
      return "进行中";
    case "packing":
      return "打包中";
    case "paused":
      return "已暂停";
    case "failed":
      return "已失败";
    case "completed":
      return "已完成";
    default:
      return "等待中";
  }
}

/** 进度百分比（0-100 整数） */
function progressPercent(task: TransferTask): number {
  if (task.status === "completed") return 100;
  if (task.total <= 0) return 0;
  return Math.min(Math.floor((task.transferred / task.total) * 100), 100);
}

/** 大小列：未开始显示总大小，否则显示 已传/总量 */
function sizeText(task: TransferTask): string {
  if (task.total <= 0 && task.transferred <= 0) return "";
  if (task.status === "pending" && task.transferred <= 0) return formatSizeFixed(task.total);
  return `${formatSizeFixed(task.transferred)}/${formatSizeFixed(task.total)}`;
}

/** 速度列：仅传输中展示 */
function speedText(task: TransferTask): string {
  if (task.status !== "running" || task.speed <= 0) return "";
  return formatRate(task.speed);
}

/** 预计剩余列 */
function etaText(task: TransferTask): string {
  if (task.status === "completed") return "00:00:00";
  if (task.status !== "running") return "";
  return formatDuration(task.etaSecs);
}

/** 经过时间列 */
function elapsedText(task: TransferTask): string {
  if (task.elapsedMs <= 0) return "";
  return formatDuration(task.elapsedMs / 1000);
}

/** 切换目录任务展开状态 */
function toggleExpand(id: string) {
  const next = new Set(expandedIds.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  expandedIds.value = next;
}

/** 双击行：目录任务切换展开 */
function onRowDblClick(row: Row) {
  if (row.hasChildren) toggleExpand(row.task.id);
}

/** 判断任务是否选中 */
function isSelected(task: TransferTask): boolean {
  return selectedIds.value.has(task.id);
}

/** 清空选择 */
function clearSelection() {
  selectedIds.value = new Set();
  selectionAnchor.value = "";
}

/** 关闭右键菜单 */
function closeContextMenu() {
  contextMenu.open = false;
}

/** 单选指定任务 */
function selectSingle(id: string) {
  selectedIds.value = new Set([id]);
  selectionAnchor.value = id;
}

/** 切换指定任务选中状态 */
function toggleSelection(id: string) {
  const next = new Set(selectedIds.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  selectedIds.value = next;
  selectionAnchor.value = id;
}

/** Shift 范围选择（按可见行顺序） */
function selectRange(id: string) {
  const ids = rows.value.map((row) => row.task.id);
  const from = ids.indexOf(selectionAnchor.value || id);
  const to = ids.indexOf(id);
  if (from < 0 || to < 0) {
    selectSingle(id);
    return;
  }
  const [start, end] = from < to ? [from, to] : [to, from];
  selectedIds.value = new Set(ids.slice(start, end + 1));
}

/** 根据修饰键选择任务 */
function selectByMouse(task: TransferTask, event: MouseEvent | PointerEvent) {
  if (event.shiftKey) selectRange(task.id);
  else if (event.ctrlKey || event.metaKey) toggleSelection(task.id);
  else selectSingle(task.id);
}

/** 任务行 DOM 列表 */
function taskRows(): HTMLElement[] {
  return Array.from(listRef.value?.querySelectorAll<HTMLElement>("tbody tr.transfer-row") ?? []);
}

/** DOM 矩形是否相交 */
function rectsIntersect(a: DOMRect, b: DOMRect): boolean {
  return a.left <= b.right && a.right >= b.left && a.top <= b.bottom && a.bottom >= b.top;
}

/** 按框选矩形更新选中项 */
function updateMarqueeSelection() {
  const rect = new DOMRect(marquee.x, marquee.y, marquee.width, marquee.height);
  const next = new Set(pointerAction?.baseSelected ?? []);
  for (const row of taskRows()) {
    const id = row.dataset.id;
    if (!id || !rectsIntersect(rect, row.getBoundingClientRect())) continue;
    if (pointerAction?.toggle && pointerAction.baseSelected.has(id)) next.delete(id);
    else next.add(id);
  }
  selectedIds.value = next;
}

/** 任务行按下：准备点击选择或拖动框选 */
function onRowPointerDown(task: TransferTask, event: PointerEvent) {
  if (event.button !== 0) return;
  const target = event.target as HTMLElement;
  if (target.closest(".tree-toggle")) return;
  pointerAction = {
    task,
    startX: event.clientX,
    startY: event.clientY,
    moved: false,
    baseSelected: event.ctrlKey || event.metaKey ? new Set(selectedIds.value) : new Set(),
    toggle: event.ctrlKey || event.metaKey,
  };
  window.addEventListener("pointermove", onPointerMove);
  window.addEventListener("pointerup", onPointerUp, { once: true });
}

/** 空白区域按下：开始框选 */
function onListPointerDown(event: PointerEvent) {
  if (event.button !== 0) return;
  const target = event.target as HTMLElement;
  if (target.closest("tr.transfer-row") || target.closest("thead") || target.closest("button")) return;
  pointerAction = {
    startX: event.clientX,
    startY: event.clientY,
    moved: false,
    baseSelected: event.ctrlKey || event.metaKey ? new Set(selectedIds.value) : new Set(),
    toggle: event.ctrlKey || event.metaKey,
  };
  if (!event.ctrlKey && !event.metaKey && !event.shiftKey) clearSelection();
  window.addEventListener("pointermove", onPointerMove);
  window.addEventListener("pointerup", onPointerUp, { once: true });
}

/** 指针移动：超过阈值后进入框选 */
function onPointerMove(event: PointerEvent) {
  if (!pointerAction) return;
  const dx = event.clientX - pointerAction.startX;
  const dy = event.clientY - pointerAction.startY;
  if (!pointerAction.moved && Math.hypot(dx, dy) < 4) return;
  pointerAction.moved = true;
  marquee.active = true;
  marquee.x = Math.min(pointerAction.startX, event.clientX);
  marquee.y = Math.min(pointerAction.startY, event.clientY);
  marquee.width = Math.abs(dx);
  marquee.height = Math.abs(dy);
  updateMarqueeSelection();
}

/** 指针抬起：未移动则按点击处理 */
function onPointerUp(event: PointerEvent) {
  window.removeEventListener("pointermove", onPointerMove);
  if (!pointerAction) return;
  const action = pointerAction;
  pointerAction = null;
  marquee.active = false;
  if (!action.moved && action.task) selectByMouse(action.task, event);
}

/** 行右键：打开菜单且不改变当前选择 */
function onRowContextMenu(event: MouseEvent) {
  event.preventDefault();
  openContextMenu(event);
}

/** 空白区域右键 */
function onListContextMenu(event: MouseEvent) {
  event.preventDefault();
  const target = event.target as HTMLElement;
  if (target.closest("tr.transfer-row") || target.closest("thead")) return;
  openContextMenu(event);
}

/** 定位右键菜单（边缘收敛不超出视口） */
function openContextMenu(event: MouseEvent) {
  contextMenu.open = true;
  contextMenu.x = Math.min(event.clientX, window.innerWidth - CONTEXT_MENU_WIDTH - CONTEXT_MENU_MARGIN);
  contextMenu.y = Math.min(event.clientY, window.innerHeight - CONTEXT_MENU_HEIGHT - CONTEXT_MENU_MARGIN);
}

/** 执行右键菜单动作 */
async function runMenuAction(item: MenuItem) {
  if (item.disabled) return;
  closeContextMenu();
  const ids = [...selectedIds.value];
  try {
    switch (item.action) {
      case "pause":
        await transferPause(ids);
        break;
      case "pauseAll":
        await transferPause();
        break;
      case "resume":
        await transferResume(ids);
        break;
      case "resumeAll":
        await transferResume();
        break;
      case "remove":
        await transferRemove(ids);
        clearSelection();
        break;
      case "removeAll":
        await transferRemove();
        clearSelection();
        break;
      case "retryFailed":
        await transferRetryFailed();
        break;
    }
  } catch (e) {
    console.warn("传输任务操作失败", e);
  }
}

/** 按 Esc 关闭菜单或清空选择 */
function onKeyDown(event: KeyboardEvent) {
  if (event.key !== "Escape") return;
  if (contextMenu.open) {
    closeContextMenu();
    return;
  }
  clearSelection();
  marquee.active = false;
}

/** 点击应用任意非菜单区域时关闭右键菜单 */
function onGlobalPointerDown(event: PointerEvent) {
  if (!contextMenu.open) return;
  const target = event.target as HTMLElement;
  if (target.closest(".transfer-context-menu")) return;
  closeContextMenu();
}

/** 开始拖拽表格列宽 */
function startColumnResize(key: ColumnKey, event: MouseEvent) {
  const startX = event.clientX;
  const startWidth = columnWidths[key];
  const move = (e: MouseEvent) => {
    columnWidths[key] = Math.max(startWidth + e.clientX - startX, COLUMN_MIN_WIDTHS[key]);
  };
  const up = () => {
    window.removeEventListener("mousemove", move);
    window.removeEventListener("mouseup", up);
    document.body.classList.remove("is-resizing");
  };
  document.body.classList.add("is-resizing");
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", up);
}

onMounted(() => {
  window.addEventListener("keydown", onKeyDown);
  window.addEventListener("pointerdown", onGlobalPointerDown);
});

onBeforeUnmount(() => {
  window.removeEventListener("pointermove", onPointerMove);
  window.removeEventListener("keydown", onKeyDown);
  window.removeEventListener("pointerdown", onGlobalPointerDown);
});
</script>

<template>
  <div class="transfer-panel">
    <div
      ref="listRef"
      class="transfer-list"
      @pointerdown="onListPointerDown"
      @contextmenu="onListContextMenu"
    >
      <table v-if="rows.length">
        <colgroup>
          <col
            v-for="column in columns"
            :key="column.key"
            :style="{ width: `${columnWidths[column.key]}px` }"
          />
        </colgroup>
        <thead>
          <tr>
            <th v-for="column in columns" :key="column.key">
              <span>{{ column.label }}</span>
              <span class="col-resizer" @mousedown.stop.prevent="startColumnResize(column.key, $event)"></span>
            </th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="row in rows"
            :key="row.task.id"
            class="transfer-row"
            :class="{ selected: isSelected(row.task) }"
            :data-id="row.task.id"
            @pointerdown="onRowPointerDown(row.task, $event)"
            @contextmenu="onRowContextMenu($event)"
            @dblclick="onRowDblClick(row)"
          >
            <td class="name" :title="row.task.name">
              <span class="indent" :style="{ width: `${row.depth * 16}px` }"></span>
              <button v-if="row.hasChildren" class="tree-toggle" @click.stop="toggleExpand(row.task.id)">
                <Icon name="chevronRight" :size="11" :class="{ expanded: expandedIds.has(row.task.id) }" />
              </button>
              <span v-else class="toggle-placeholder"></span>
              <Icon
                :name="row.task.isDir ? 'folder' : 'file'"
                :size="13"
                :class="row.task.isDir ? 'ic-folder' : 'ic-file'"
              />
              <span class="ellipsis">{{ row.task.name }}</span>
            </td>
            <td :title="row.task.error || statusText(row.task)">
              <span :class="['status-text', row.task.status]">{{ statusText(row.task) }}</span>
            </td>
            <td class="progress">
              <div class="pg-track">
                <div class="pg-bar" :style="{ width: `${progressPercent(row.task)}%` }"></div>
                <span class="pg-label">{{ progressPercent(row.task) }}%</span>
              </div>
            </td>
            <td class="size" :title="sizeText(row.task)">{{ sizeText(row.task) }}</td>
            <td :title="row.task.localPath">{{ row.task.localPath }}</td>
            <td class="kind">
              <Icon
                :name="row.task.kind === 'upload' ? 'arrowUp' : 'arrowDown'"
                :size="12"
                :class="row.task.kind === 'upload' ? 'ic-upload' : 'ic-download'"
              />
              <span :class="row.task.kind === 'upload' ? 'ic-upload' : 'ic-download'">
                {{ row.task.kind === "upload" ? "上传" : "下载" }}
              </span>
            </td>
            <td :title="row.task.remotePath">{{ row.task.remotePath }}</td>
            <td class="speed">{{ speedText(row.task) }}</td>
            <td class="mono">{{ etaText(row.task) }}</td>
            <td class="mono">{{ elapsedText(row.task) }}</td>
          </tr>
        </tbody>
      </table>
      <div v-else class="tp-tip">暂无传输任务</div>
      <div
        v-if="marquee.active"
        class="selection-marquee"
        :style="{ left: `${marquee.x}px`, top: `${marquee.y}px`, width: `${marquee.width}px`, height: `${marquee.height}px` }"
      ></div>
      <div
        v-if="contextMenu.open"
        class="transfer-context-menu"
        :style="{ left: `${contextMenu.x}px`, top: `${contextMenu.y}px` }"
        @click.stop
      >
        <button
          v-for="item in contextMenuItems"
          :key="item.action"
          :disabled="item.disabled"
          @click="runMenuAction(item)"
        >
          {{ item.label }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.transfer-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg-window);
}
.transfer-list {
  flex: 1;
  overflow: auto;
  min-width: 0;
  position: relative;
}
.transfer-list table {
  width: max-content;
  min-width: 100%;
  border-collapse: collapse;
  font-size: 12px;
  table-layout: fixed;
}
.transfer-list thead th {
  position: sticky;
  top: 0;
  height: 26px;
  background: linear-gradient(var(--table-head-top), var(--table-head-bottom));
  color: #3a3f45;
  font-weight: 600;
  text-align: left;
  padding: 0 8px;
  border-bottom: 1px solid var(--border);
  border-right: 1px solid var(--border-light);
  white-space: nowrap;
  z-index: 1;
  user-select: none;
  overflow: hidden;
  text-overflow: ellipsis;
}
.transfer-list thead th:last-child {
  border-right: none;
}
.col-resizer {
  position: absolute;
  top: 0;
  right: -3px;
  bottom: 0;
  width: 6px;
  cursor: col-resize;
  z-index: 2;
}
.col-resizer:hover {
  background: rgba(74, 115, 156, 0.18);
}
.transfer-list tbody td {
  height: 26px;
  padding: 0 8px;
  border-bottom: 1px solid var(--border-light);
  color: #444;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.transfer-list tbody tr:hover td {
  background: var(--row-hover);
}
.transfer-list tbody tr.selected td {
  background: #d9e6f4;
}
.transfer-list td.name {
  display: flex;
  align-items: center;
  gap: 4px;
  overflow: hidden;
}
.transfer-list td.name .ellipsis {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
}
.indent {
  flex: 0 0 auto;
}
.tree-toggle {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 14px;
  height: 14px;
  padding: 0;
  border: none;
  background: transparent;
  color: #6d7782;
  cursor: pointer;
  flex: 0 0 14px;
}
.tree-toggle .expanded {
  transform: rotate(90deg);
}
.toggle-placeholder {
  flex: 0 0 14px;
}
.ic-folder {
  color: #e0b64a;
  flex: 0 0 auto;
}
.ic-file {
  color: #9aa6b0;
  flex: 0 0 auto;
}
/* 状态文案配色 */
.status-text.running,
.status-text.packing {
  color: #2769b0;
}
.status-text.paused {
  color: #c07a1c;
}
.status-text.failed {
  color: var(--danger);
}
.status-text.completed {
  color: #2b8a3e;
}
.status-text.pending {
  color: #667;
}
/* 进度条 */
.transfer-list td.progress {
  padding: 0 6px;
}
.pg-track {
  position: relative;
  height: 15px;
  border: 1px solid #c3ccd6;
  border-radius: 2px;
  background: #f0f2f5;
  overflow: hidden;
}
.pg-bar {
  position: absolute;
  top: 0;
  left: 0;
  bottom: 0;
  background: #3f8ae0;
  transition: width 0.2s linear;
}
.pg-label {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 11px;
  color: #223;
  mix-blend-mode: multiply;
}
/* 操作类型 */
.transfer-list td.kind {
  display: flex;
  align-items: center;
  gap: 3px;
}
.ic-upload {
  color: #d64545;
}
.ic-download {
  color: #2b8a3e;
}
.transfer-list td.size,
.transfer-list td.speed {
  text-align: right;
  color: #555;
}
.mono {
  font-family: "Consolas", monospace;
}
.tp-tip {
  padding: 24px;
  text-align: center;
  color: var(--text-muted);
}
.selection-marquee {
  position: fixed;
  z-index: 20;
  pointer-events: none;
  border: 1px solid #5b8ec5;
  background: rgba(91, 142, 197, 0.16);
}
.transfer-context-menu {
  position: fixed;
  z-index: 30;
  min-width: 140px;
  padding: 4px;
  border: 1px solid #b8c6d6;
  border-radius: 4px;
  background: #fff;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.18);
}
.transfer-context-menu button {
  display: block;
  width: 100%;
  height: 24px;
  padding: 0 10px;
  border: none;
  border-radius: 3px;
  background: transparent;
  color: #333;
  font-size: 12px;
  text-align: left;
  cursor: pointer;
}
.transfer-context-menu button:hover:not(:disabled) {
  background: var(--row-hover);
  color: var(--accent);
}
.transfer-context-menu button:disabled {
  color: #aab2bb;
  cursor: not-allowed;
}
</style>
