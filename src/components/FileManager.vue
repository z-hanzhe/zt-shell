<script setup lang="ts">
/**
 * 右下文件管理器：SFTP 目录浏览（左目录树 + 右文件列表），支持上传下载增删改
 */
import { computed, nextTick, onBeforeUnmount, reactive, ref, watch } from "vue";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import AppDialog from "./AppDialog.vue";
import Icon from "./Icon.vue";
import {
  sftpList,
  sftpHome,
  sftpRemoveFile,
  sftpRemoveDir,
  sftpCreateDir,
  sftpRename,
  sftpDownload,
} from "../api";
import type { FileEntry } from "../types";
import { formatShort, formatTime, joinPath, parentPath } from "../utils";

const props = defineProps<{
  /** 当前会话标识，空表示无活动会话 */
  sessionId: string;
  /** 会话是否已连接 */
  connected: boolean;
}>();

/** 当前目录 */
const cwd = ref("/");
/** 目录条目 */
const entries = ref<FileEntry[]>([]);
/** 加载中标志 */
const loading = ref(false);
/** 错误信息 */
const error = ref("");
/** 选中项名称 */
const selected = ref<string>("");
/** 已展开的目录路径 */
const expandedDirs = ref<Set<string>>(new Set(["/"]));
/** 目录树节点缓存 */
const treeChildren = ref<Record<string, FileEntry[]>>({});
/** 目录树宽度 */
const treeWidth = ref(190);
/** 目录树滚动容器 */
const dirTreeRef = ref<HTMLElement | null>(null);
/** 排序状态 */
const sortState = ref<{ key: SortKey; direction: SortDirection }>({
  key: "name",
  direction: "asc",
});
/** 通用弹窗状态 */
const dialog = reactive<DialogState>({
  open: false,
  type: "info",
  title: "",
  message: "",
  defaultValue: "",
  placeholder: "",
  confirmText: "确定",
  hintTemplate: "",
  resolve: undefined,
});

/** 鼠标拖拽清理函数 */
let stopResize: (() => void) | undefined;

type SortKey = "name" | "size" | "type" | "modified" | "permissions" | "owner";
type SortDirection = "asc" | "desc";
type TreeNode = { path: string; name: string; depth: number };
type ColumnKey = SortKey;
type DialogState = {
  open: boolean;
  type: "info" | "confirm" | "prompt";
  title: string;
  message: string;
  defaultValue: string;
  placeholder: string;
  confirmText: string;
  hintTemplate: string;
  resolve?: (value: string | boolean | null) => void;
};

const columns: { key: ColumnKey; label: string }[] = [
  { key: "name", label: "文件名" },
  { key: "size", label: "大小" },
  { key: "type", label: "类型" },
  { key: "modified", label: "修改时间" },
  { key: "permissions", label: "权限" },
  { key: "owner", label: "用户/组" },
];

/** 文件列表列宽 */
const columnWidths = reactive<Record<ColumnKey, number>>({
  name: 360,
  size: 64,
  type: 76,
  modified: 116,
  permissions: 88,
  owner: 88,
});

/** 表格展示项，非根目录补充置顶的返回上级项 */
const visibleEntries = computed(() => {
  const sorted = [...entries.value].sort(compareEntries);
  if (cwd.value === "/") return sorted;
  return [createParentEntry(), ...sorted];
});

/** 扁平化后的目录树节点 */
const treeNodes = computed(() => {
  const nodes: TreeNode[] = [{ path: "/", name: "/", depth: 0 }];
  appendTreeChildren(nodes, "/", 1);
  return nodes;
});

/** 依据文件条目推断类型描述 */
function fileType(entry: FileEntry): string {
  if (entry.name === "...") return "上级目录";
  if (entry.isDir) return "文件夹";
  if (entry.isSymlink) return "链接";
  const dot = entry.name.lastIndexOf(".");
  if (dot > 0) return `${entry.name.slice(dot + 1).toUpperCase()} 文件`;
  return "文件";
}

/** 构造返回上级目录的虚拟条目 */
function createParentEntry(): FileEntry {
  return {
    name: "...",
    isDir: true,
    isSymlink: false,
    size: 0,
    permissions: 0,
    permissionsStr: "",
    modified: 0,
    owner: "",
    group: "",
  };
}

/** 递归追加已展开的目录树子节点 */
function appendTreeChildren(nodes: TreeNode[], path: string, depth: number) {
  if (!expandedDirs.value.has(path)) return;
  const children = treeChildren.value[path] ?? [];
  for (const child of children) {
    const childPath = joinPath(path, child.name);
    nodes.push({ path: childPath, name: child.name, depth });
    appendTreeChildren(nodes, childPath, depth + 1);
  }
}

/** 取目录树指定目录的子目录 */
async function loadTreeDir(path: string) {
  if (!props.sessionId || !props.connected) return;
  if (treeChildren.value[path]) return;
  const list = await sftpList(props.sessionId, path);
  treeChildren.value = {
    ...treeChildren.value,
    [path]: list.filter((e) => e.isDir).sort((a, b) => compareName(a.name, b.name)),
  };
}

/** 确保当前路径在目录树中可见并选中 */
async function syncTreeToCwd() {
  if (!props.sessionId || !props.connected) return;
  const parts = cwd.value.split("/").filter(Boolean);
  let path = "/";
  expandedDirs.value.add("/");
  await loadTreeDir("/");
  for (const part of parts.slice(0, -1)) {
    path = joinPath(path, part);
    expandedDirs.value.add(path);
    await loadTreeDir(path);
  }
  expandedDirs.value = new Set(expandedDirs.value);
  await scrollActiveTreeNodeIntoView();
}

/** 统一切换当前目录 */
async function setCwd(path: string) {
  cwd.value = normalizePath(path);
  selected.value = "";
  await refresh();
  await syncTreeToCwd();
}

/** 规范化远端目录路径 */
function normalizePath(path: string): string {
  const normalized = `/${path.trim().replace(/\\/g, "/").replace(/^\/+/, "")}`.replace(/\/+$/, "");
  return normalized || "/";
}

/** 按文件名比较 */
function compareName(a: string, b: string): number {
  return a.localeCompare(b, "zh-Hans-CN", { numeric: true, sensitivity: "base" });
}

/** 按当前表头排序比较条目 */
function compareEntries(a: FileEntry, b: FileEntry): number {
  if (a.name === "...") return -1;
  if (b.name === "...") return 1;
  if (a.isDir !== b.isDir) return sortState.value.direction === "asc" ? (a.isDir ? -1 : 1) : a.isDir ? 1 : -1;
  let result = 0;
  switch (sortState.value.key) {
    case "size":
      result = a.size - b.size;
      break;
    case "type":
      result = compareName(fileType(a), fileType(b));
      break;
    case "modified":
      result = a.modified - b.modified;
      break;
    case "permissions":
      result = compareName(a.permissionsStr, b.permissionsStr);
      break;
    case "owner":
      result = compareName(`${a.owner}/${a.group}`, `${b.owner}/${b.group}`);
      break;
    default:
      result = compareName(a.name, b.name);
  }
  if (result === 0) result = compareName(a.name, b.name);
  return sortState.value.direction === "asc" ? result : -result;
}

/** 将目录树选中项滚动到可视区域 */
async function scrollActiveTreeNodeIntoView() {
  await nextTick();
  const active = dirTreeRef.value?.querySelector<HTMLElement>(".dir-item.active");
  active?.scrollIntoView({ block: "nearest" });
}

/** 点击表头切换排序方向 */
function setSort(key: SortKey) {
  if (sortState.value.key === key) {
    sortState.value.direction = sortState.value.direction === "asc" ? "desc" : "asc";
    return;
  }
  sortState.value = { key, direction: "asc" };
}

/** 显示排序方向 */
function sortMark(key: SortKey): string {
  if (sortState.value.key !== key) return "";
  return sortState.value.direction === "asc" ? "↑" : "↓";
}

/** 获取表格单元格完整内容 */
function cellTitle(entry: FileEntry, key: ColumnKey): string {
  switch (key) {
    case "name":
      return entry.name;
    case "size":
      return entry.isDir ? "" : formatShort(entry.size);
    case "type":
      return fileType(entry);
    case "modified":
      return formatTime(entry.modified);
    case "permissions":
      return entry.permissionsStr;
    case "owner":
      return entry.owner || entry.group ? `${entry.owner}/${entry.group}` : "";
  }
}

/** 开始拖拽表格列宽 */
function startColumnResize(key: ColumnKey, event: MouseEvent) {
  const startX = event.clientX;
  const startWidth = columnWidths[key];
  const minWidth: Record<ColumnKey, number> = {
    name: 180,
    size: 48,
    type: 54,
    modified: 96,
    permissions: 72,
    owner: 72,
  };
  const move = (e: MouseEvent) => {
    columnWidths[key] = Math.max(startWidth + e.clientX - startX, minWidth[key]);
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

/** 刷新当前目录 */
async function refresh() {
  if (!props.sessionId || !props.connected) {
    entries.value = [];
    return;
  }
  loading.value = true;
  error.value = "";
  try {
    entries.value = await sftpList(props.sessionId, cwd.value);
    treeChildren.value = {
      ...treeChildren.value,
      [cwd.value]: entries.value.filter((e) => e.isDir).sort((a, b) => compareName(a.name, b.name)),
    };
  } catch (e) {
    error.value = String(e);
    entries.value = [];
  } finally {
    loading.value = false;
  }
}

/** 进入目录 */
function enterDir(name: string) {
  setCwd(joinPath(cwd.value, name));
}

/** 双击条目：目录则进入 */
function onOpen(entry: FileEntry) {
  if (entry.name === "...") {
    goUp();
    return;
  }
  if (entry.isDir) enterDir(entry.name);
}

/** 返回上级目录 */
function goUp() {
  if (cwd.value === "/") return;
  setCwd(parentPath(cwd.value));
}

/** 手动跳转到输入的路径 */
function goPath(path: string) {
  setCwd(path.trim() || "/");
}

/** 点击目录树节点后右侧显示该目录 */
function selectTreeNode(path: string) {
  setCwd(path);
}

/** 展开或收起目录树节点 */
async function toggleTreeNode(path: string) {
  if (path === "/") return;
  const next = new Set(expandedDirs.value);
  if (next.has(path) && path !== "/") {
    next.delete(path);
  } else {
    next.add(path);
    await loadTreeDir(path);
  }
  expandedDirs.value = next;
}

/** 双击目录树节点切换展开状态 */
function onTreeDblClick(path: string) {
  toggleTreeNode(path);
}

/** 开始拖拽左右分割线 */
function startResize(event: MouseEvent) {
  stopResize?.();
  const startX = event.clientX;
  const startWidth = treeWidth.value;
  const move = (e: MouseEvent) => {
    treeWidth.value = Math.min(Math.max(startWidth + e.clientX - startX, 130), 360);
  };
  const up = () => {
    window.removeEventListener("mousemove", move);
    window.removeEventListener("mouseup", up);
    document.body.classList.remove("is-resizing");
    stopResize = undefined;
  };
  stopResize = up;
  document.body.classList.add("is-resizing");
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", up);
}

onBeforeUnmount(() => {
  stopResize?.();
});

/** 下载选中文件 */
async function onDownload(entry: FileEntry) {
  if (entry.isDir) {
    showMessage("下载提示", "暂不支持下载目录");
    return;
  }
  const localPath = await saveDialog({ defaultPath: entry.name });
  if (!localPath) return;
  try {
    await sftpDownload(props.sessionId, joinPath(cwd.value, entry.name), localPath);
  } catch (e) {
    showMessage("下载失败", String(e));
  }
}

/** 删除选中项 */
async function onDelete(entry: FileEntry) {
  const confirmed = await showConfirm("删除确认", `确定删除「${entry.name}」？`);
  if (!confirmed) return;
  const path = joinPath(cwd.value, entry.name);
  try {
    if (entry.isDir) {
      await sftpRemoveDir(props.sessionId, path);
    } else {
      await sftpRemoveFile(props.sessionId, path);
    }
    await refresh();
  } catch (e) {
    showMessage("删除失败", String(e));
  }
}

/** 新建目录 */
async function onNewDir() {
  const name = await showPrompt("新建目录", "请输入新目录名称", "新目录名称", "", joinPath(cwd.value, "{value}"));
  if (!name?.trim()) return;
  try {
    await sftpCreateDir(props.sessionId, joinPath(cwd.value, name.trim()));
    treeChildren.value = { ...treeChildren.value, [cwd.value]: [] };
    await refresh();
    await syncTreeToCwd();
  } catch (e) {
    showMessage("创建失败", String(e));
  }
}

/** 重命名选中项 */
async function onRename(entry: FileEntry) {
  const newName = await showPrompt("重命名", "请输入新名称", "新名称", entry.name);
  if (!newName?.trim() || newName.trim() === entry.name) return;
  try {
    await sftpRename(
      props.sessionId,
      joinPath(cwd.value, entry.name),
      joinPath(cwd.value, newName.trim())
    );
    treeChildren.value = { ...treeChildren.value, [cwd.value]: [] };
    await refresh();
  } catch (e) {
    showMessage("重命名失败", String(e));
  }
}

/** 显示提示弹窗 */
function showMessage(title: string, message: string) {
  openDialogState("info", title, message, "确定");
}

/** 显示确认弹窗 */
function showConfirm(title: string, message: string): Promise<boolean> {
  return new Promise((resolve) => {
    openDialogState("confirm", title, message, "确定", "", "", "", (value) => resolve(value === true));
  });
}

/** 显示输入弹窗 */
function showPrompt(
  title: string,
  message: string,
  placeholder: string,
  defaultValue = "",
  hintTemplate = ""
): Promise<string | null> {
  return new Promise((resolve) => {
    openDialogState("prompt", title, message, "确定", placeholder, defaultValue, hintTemplate, (value) => {
      resolve(typeof value === "string" ? value : null);
    });
  });
}

/** 打开通用弹窗 */
function openDialogState(
  type: DialogState["type"],
  title: string,
  message: string,
  confirmText: string,
  placeholder = "",
  defaultValue = "",
  hintTemplate = "",
  resolve?: DialogState["resolve"]
) {
  Object.assign(dialog, { open: true, type, title, message, confirmText, placeholder, defaultValue, hintTemplate, resolve });
}

/** 确认通用弹窗 */
function onDialogConfirm(value: string) {
  const resolve = dialog.resolve;
  dialog.open = false;
  resolve?.(dialog.type === "prompt" ? value : true);
}

/** 取消通用弹窗 */
function onDialogCancel() {
  const resolve = dialog.resolve;
  dialog.open = false;
  resolve?.(dialog.type === "prompt" ? null : false);
}

// 会话切换或连接成功后，定位到主目录
watch(
  () => [props.sessionId, props.connected] as const,
  async ([id, conn]) => {
    if (id && conn) {
      try {
        cwd.value = await sftpHome(id);
      } catch {
        cwd.value = "/";
      }
      treeChildren.value = {};
      expandedDirs.value = new Set(["/"]);
      await refresh();
      await syncTreeToCwd();
    } else {
      entries.value = [];
      treeChildren.value = {};
    }
  },
  { immediate: true }
);
</script>

<template>
  <div class="fm-panel">
    <!-- 工具栏 -->
    <div class="file-toolbar">
      <button class="ic" title="上级目录" @click="goUp">
        <Icon name="arrowUp" :size="14" />
      </button>
      <button class="ic" title="刷新" @click="refresh">
        <Icon name="refresh" :size="13" />
      </button>
      <button class="ic" title="新建目录" @click="onNewDir">
        <Icon name="plus" :size="14" />
      </button>
      <input
        class="path-input"
        :value="cwd"
        @keyup.enter="goPath(($event.target as HTMLInputElement).value)"
      />
    </div>

    <div class="file-body">
      <!-- 目录树 -->
      <div ref="dirTreeRef" class="dir-tree" :style="{ flexBasis: `${treeWidth}px`, width: `${treeWidth}px` }">
        <div
          v-for="node in treeNodes"
          :key="node.path"
          :class="['dir-item', { active: cwd === node.path, root: node.path === '/' }]"
          :style="{ paddingLeft: `${node.path === '/' ? 4 : 4 + (node.depth - 1) * 16}px` }"
          :title="node.path"
          @click="selectTreeNode(node.path)"
          @dblclick.stop="onTreeDblClick(node.path)"
        >
          <button v-if="node.path !== '/'" class="tree-toggle" @click.stop="toggleTreeNode(node.path)">
            <Icon
              name="chevronRight"
              :size="11"
              :class="{ expanded: expandedDirs.has(node.path) }"
            />
          </button>
          <Icon name="folder" :size="13" class="ic-folder" />
          <span class="ellipsis">{{ node.name }}</span>
        </div>
      </div>
      <div class="tree-resizer" @mousedown="startResize"></div>

      <!-- 文件列表 -->
      <div class="file-list">
        <div v-if="!connected" class="fm-tip">未连接会话</div>
        <div v-else-if="loading" class="fm-tip">加载中…</div>
        <div v-else-if="error" class="fm-tip error">{{ error }}</div>
        <table v-else>
          <colgroup>
            <col
              v-for="column in columns"
              :key="column.key"
              :style="{ width: `${columnWidths[column.key]}px` }"
            />
          </colgroup>
          <thead>
            <tr>
              <th
                v-for="column in columns"
                :key="column.key"
                @click="setSort(column.key)"
              >
                <span>{{ column.label }} {{ sortMark(column.key) }}</span>
                <span class="col-resizer" @mousedown.stop.prevent="startColumnResize(column.key, $event)"></span>
              </th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="entry in visibleEntries"
              :key="entry.name"
              :class="{ selected: selected === entry.name }"
              @click="selected = entry.name"
              @dblclick="onOpen(entry)"
            >
              <td class="name" :title="cellTitle(entry, 'name')">
                <Icon
                  :name="entry.isDir ? 'folder' : 'file'"
                  :size="14"
                  :class="entry.isDir ? 'ic-folder' : 'ic-file'"
                />
                <span class="ellipsis">{{ entry.name }}</span>
                <span class="row-ops">
                  <button v-if="!entry.isDir" title="下载" @click.stop="onDownload(entry)">
                    <Icon name="download" :size="12" />
                  </button>
                  <button v-if="entry.name !== '...'" title="重命名" @click.stop="onRename(entry)">
                    <Icon name="edit" :size="12" />
                  </button>
                  <button v-if="entry.name !== '...'" class="danger" title="删除" @click.stop="onDelete(entry)">
                    <Icon name="trash" :size="12" />
                  </button>
                </span>
              </td>
              <td class="size" :title="cellTitle(entry, 'size')">{{ entry.isDir ? "" : formatShort(entry.size) }}</td>
              <td :title="cellTitle(entry, 'type')">{{ fileType(entry) }}</td>
              <td :title="cellTitle(entry, 'modified')">{{ formatTime(entry.modified) }}</td>
              <td class="perm mono" :title="cellTitle(entry, 'permissions')">{{ entry.permissionsStr }}</td>
              <td class="owner" :title="cellTitle(entry, 'owner')">{{ cellTitle(entry, "owner") }}</td>
            </tr>
            <tr v-if="visibleEntries.length === 0">
              <td colspan="6" class="fm-tip">空目录</td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
    <AppDialog
      :open="dialog.open"
      :type="dialog.type"
      :title="dialog.title"
      :message="dialog.message"
      :default-value="dialog.defaultValue"
      :placeholder="dialog.placeholder"
      :confirm-text="dialog.confirmText"
      :hint-template="dialog.hintTemplate"
      @confirm="onDialogConfirm"
      @cancel="onDialogCancel"
    />
  </div>
</template>

<style scoped>
.fm-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg-window);
}

/* 工具栏 */
.file-toolbar {
  display: flex;
  align-items: center;
  height: 28px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
  padding: 0 8px;
  gap: 6px;
  flex: 0 0 auto;
}
.file-toolbar .ic {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border: none;
  border-radius: var(--radius);
  background: transparent;
  color: #556;
  cursor: pointer;
  flex-shrink: 0;
}
.file-toolbar .ic:hover {
  background: var(--row-hover);
  color: var(--accent);
}
.path-input {
  flex: 1;
  height: 22px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: #fff;
  color: var(--text);
  outline: none;
  font-size: 12px;
}
.path-input:focus {
  border-color: var(--accent);
}

/* 主体 */
.file-body {
  flex: 1;
  display: flex;
  min-height: 0;
}

/* 目录树 */
.dir-tree {
  flex: 0 0 auto;
  overflow-y: auto;
  background: #fbfcfd;
  font-size: 12px;
}
.tree-resizer {
  flex: 0 0 5px;
  width: 5px;
  border-left: 1px solid var(--border);
  border-right: 1px solid transparent;
  background: #f4f7fa;
  cursor: col-resize;
}
.tree-resizer:hover {
  background: #dce8f4;
  border-left-color: #9db8d3;
}
.dir-item {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 4px 8px;
  color: #3a3f45;
  cursor: pointer;
  white-space: nowrap;
  overflow: hidden;
}
.dir-item:hover {
  background: var(--row-hover);
}
.dir-item.active {
  background: #d9e6f4;
}
.dir-item.root {
  gap: 3px;
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
.ic-folder {
  color: #e0b64a;
  flex: 0 0 auto;
}
.ic-file {
  color: #9aa6b0;
}

/* 文件列表 */
.file-list {
  flex: 1;
  overflow: auto;
  min-width: 0;
}
.file-list table {
  width: max-content;
  min-width: 100%;
  border-collapse: collapse;
  font-size: 12px;
  table-layout: fixed;
}
.file-list thead th {
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
  cursor: pointer;
  user-select: none;
  overflow: hidden;
  text-overflow: ellipsis;
}
.file-list thead th:hover {
  background: linear-gradient(#edf3f8, #dfe8f1);
}
.file-list thead th:last-child {
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
.file-list tbody td {
  height: 26px;
  padding: 0 8px;
  border-bottom: 1px solid var(--border-light);
  color: #444;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.file-list tbody tr:hover td {
  background: var(--row-hover);
}
.file-list tbody tr.selected td {
  background: #d9e6f4;
}
.file-list td.name {
  display: flex;
  align-items: center;
  gap: 7px;
  overflow: hidden;
}
.file-list td.name .ellipsis {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
}
.file-list td.size {
  text-align: right;
  color: #666;
}
.file-list td.perm,
.file-list td.owner {
  color: #666;
}
.mono {
  font-family: "Consolas", monospace;
}
.ellipsis {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 行内操作按钮 */
.row-ops {
  display: flex;
  visibility: hidden;
  gap: 2px;
  margin-left: auto;
  flex-shrink: 0;
}
.file-list tr:hover .row-ops {
  visibility: visible;
}
.row-ops button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  border: none;
  border-radius: 3px;
  background: transparent;
  color: #778;
  cursor: pointer;
}
.row-ops button:hover {
  background: #e3e9f0;
  color: var(--accent);
}
.row-ops button.danger:hover {
  color: var(--danger);
}

.fm-tip {
  padding: 24px;
  text-align: center;
  color: var(--text-muted);
}
.fm-tip.error {
  color: var(--danger);
}
</style>
