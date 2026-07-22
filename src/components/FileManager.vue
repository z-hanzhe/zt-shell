<script setup lang="ts">
/**
 * 右下文件管理器：SFTP 目录浏览（左目录树 + 右文件列表），支持上传下载增删改
 */
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import AppDialog from "./AppDialog.vue";
import Icon from "./Icon.vue";
import {
  sftpList,
  sftpHome,
  sftpRemoveFile,
  sftpRemoveDir,
  sftpCreateDir,
  sftpRename,
  sftpWrite,
  sftpCreateArchive,
  sftpExtractArchive,
  sftpSetSudo,
  transferUpload,
  transferDownload,
  transferPackDownload,
} from "../api";
import type { FileEntry, TransferCreateResult } from "../types";
import { formatShort, formatTime, joinPath, parentPath } from "../utils";
import { hasOpenModal } from "../composables/useEscClose";
import { useTransfersStore } from "../stores/transfers";
import { useSessionsStore } from "../stores/sessions";
import {
  focusExistingTextEditorWindow,
  openTextEditorWindow,
} from "../editorWindows";

const props = defineProps<{
  /** 当前会话标识，空表示无活动会话 */
  sessionId: string;
  /** 会话是否已连接 */
  connected: boolean;
  /** 是否为当前激活选项卡，决定键盘导航是否响应 */
  active: boolean;
}>();

const emit = defineEmits<{
  (e: "sync-terminal-path", path: string): void;
  (e: "sync-file-path"): void;
}>();

/** 当前目录 */
const cwd = ref("/");
/** 目录条目 */
const entries = ref<FileEntry[]>([]);
/** 加载中标志 */
const loading = ref(false);
/** 错误信息 */
const error = ref("");
/** 是否启用 sudo 提权文件管理 */
const sudoActive = ref(false);
/** 选中项名称集合 */
const selectedNames = ref<Set<string>>(new Set());
/** 多选锚点，用于 Shift 范围选择 */
const selectionAnchor = ref("");
/** 已展开的目录路径 */
const expandedDirs = ref<Set<string>>(new Set(["/"]));
/** 目录树节点缓存 */
const treeChildren = ref<Record<string, FileEntry[]>>({});
/** 目录树宽度 */
const treeWidth = ref(190);
/** 目录树滚动容器 */
const dirTreeRef = ref<HTMLElement | null>(null);
/** 文件列表滚动容器 */
const fileListRef = ref<HTMLElement | null>(null);
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
  confirmDanger: false,
  resolve: undefined,
});
/** 鼠标框选状态 */
const marquee = reactive({ active: false, x: 0, y: 0, width: 0, height: 0 });
/** 拖拽移动状态 */
const fileDrag = reactive({ active: false, count: 0, x: 0, y: 0, target: "" });
/** 右键菜单状态 */
const contextMenu = reactive({ open: false, x: 0, y: 0, submenuLeft: false });
/** 系统文件拖入悬停提示 */
const dropHover = ref(false);
/** 键入快速定位状态 */
const typeahead = reactive({ active: false, zone: "list" as TypeaheadZone, keyword: "", index: 0 });
/** 树区快速定位当前命中的节点路径 */
const typeaheadTreePath = ref("");

const transfersStore = useTransfersStore();
const sessionsStore = useSessionsStore();

/** 鼠标拖拽清理函数 */
let stopResize: (() => void) | undefined;
let pointerAction: PointerAction | null = null;
/** 系统文件拖拽事件反注册函数 */
let unlistenDragDrop: (() => void) | undefined;
/** 文本编辑窗口保存事件反注册函数 */
let unlistenEditorSaved: UnlistenFn | undefined;
/** 上传完成后的延迟刷新计时器（合并批量完成） */
let uploadRefreshTimer: ReturnType<typeof setTimeout> | undefined;
/** 最近一次交互的区域，决定键入快速定位作用于树还是列表 */
let activeZone: TypeaheadZone = "list";

type SortKey = "name" | "size" | "type" | "modified" | "permissions" | "owner";
type SortDirection = "asc" | "desc";
type TreeNode = { path: string; name: string; depth: number };
type ColumnKey = SortKey;
type PointerMode = "select" | "drag";
type TypeaheadZone = "tree" | "list";
type ArchiveFormat = "zip" | "tarGz";
type EditorSavedPayload = { sessionId: string; path: string };
type PointerAction = {
  mode: PointerMode;
  entry?: FileEntry;
  startX: number;
  startY: number;
  moved: boolean;
  baseSelected: Set<string>;
  toggle: boolean;
};
type MenuAction =
  | "refresh"
  | "edit"
  | "copyPath"
  | "rename"
  | "uploadFile"
  | "uploadDir"
  | "download"
  | "packDownload"
  | "archiveZip"
  | "archiveTarGz"
  | "extractArchive"
  | "newFile"
  | "newDir"
  | "delete";
type MenuItem = {
  key: string;
  label: string;
  disabled: boolean;
  action?: MenuAction;
  children?: MenuItem[];
};
type DialogState = {
  open: boolean;
  type: "info" | "confirm" | "prompt" | "loading";
  title: string;
  message: string;
  defaultValue: string;
  placeholder: string;
  confirmText: string;
  hintTemplate: string;
  /** 确认按钮是否使用红色警示样式 */
  confirmDanger: boolean;
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

/** 可选择的真实文件条目 */
const selectableEntries = computed(() => visibleEntries.value.filter((entry) => entry.name !== "..."));

/** 当前选中的真实条目 */
const selectedEntries = computed(() => entries.value.filter((entry) => selectedNames.value.has(entry.name)));

/** 右键菜单项 */
const contextMenuItems = computed<MenuItem[]>(() => {
  const count = selectedEntries.value.length;
  const first = selectedEntries.value[0];
  const multi = count > 1;
  const singleDir = count === 1 && first?.isDir;
  return [
    { key: "refresh", action: "refresh", label: "刷新", disabled: false },
    { key: "edit", action: "edit", label: "编辑文本", disabled: count !== 1 || singleDir || multi },
    { key: "copyPath", action: "copyPath", label: "复制路径", disabled: count !== 1 || multi },
    { key: "rename", action: "rename", label: "重命名", disabled: count !== 1 || multi },
    {
      key: "new",
      label: "新建",
      disabled: false,
      children: [
        { key: "newFile", action: "newFile", label: "文件", disabled: false },
        { key: "newDir", action: "newDir", label: "文件夹", disabled: false },
      ],
    },
    {
      key: "archive",
      label: "压缩",
      disabled: count === 0,
      children: [
        { key: "archiveZip", action: "archiveZip", label: "压缩为 zip", disabled: count === 0 },
        { key: "archiveTarGz", action: "archiveTarGz", label: "压缩为 tar.gz", disabled: count === 0 },
        {
          key: "extractArchive",
          action: "extractArchive",
          label: "解压到当前目录",
          disabled: count !== 1 || singleDir || !isExtractableArchive(first?.name ?? ""),
        },
      ],
    },
    {
      key: "upload",
      label: "上传",
      disabled: false,
      children: [
        { key: "uploadFile", action: "uploadFile", label: "文件", disabled: false },
        { key: "uploadDir", action: "uploadDir", label: "文件夹", disabled: false },
      ],
    },
    {
      key: "download",
      label: "下载",
      disabled: count === 0,
      children: [
        { key: "downloadDirect", action: "download", label: "直接下载", disabled: count === 0 },
        { key: "packDownload", action: "packDownload", label: "打包下载", disabled: count === 0 },
      ],
    },
    // sudo 模式经 SFTP 递归删除，普通模式经 rm -rf 快速删除
    { key: "delete", action: "delete", label: sudoActive.value ? "删除" : "删除(rm -rf)", disabled: count === 0 },
  ];
});

const CONTEXT_MENU_WIDTH = 152;
const CONTEXT_SUBMENU_WIDTH = 172;
const CONTEXT_MENU_HEIGHT = 226;
const CONTEXT_MENU_MARGIN = 8;
/** PageUp/PageDown 一次移动的条目数 */
const PAGE_STEP = 10;

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
  const version = sessionViewVersion;
  const sessionId = props.sessionId;
  const list = await sftpList(sessionId, path);
  if (version !== sessionViewVersion || sessionId !== props.sessionId || !props.connected) return;
  treeChildren.value = {
    ...treeChildren.value,
    [path]: list.filter((e) => e.isDir).sort((a, b) => compareName(a.name, b.name)),
  };
}

/** 使目录树缓存失效，下次需要时重新加载真实目录 */
function invalidateTreeDirs(...paths: string[]) {
  const next = { ...treeChildren.value };
  for (const path of paths) delete next[path];
  treeChildren.value = next;
}

/** 重新加载仍处于展开状态的目录，避免缓存失效后左侧树临时变空 */
async function reloadExpandedTreeDirs(...paths: string[]) {
  for (const path of [...new Set(paths)]) {
    if (expandedDirs.value.has(path)) await loadTreeDir(path);
  }
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
  clearSelection();
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

/** 判断条目是否被选中 */
function isSelected(entry: FileEntry): boolean {
  return selectedNames.value.has(entry.name);
}

/** 清空文件列表选择 */
function clearSelection() {
  selectedNames.value = new Set();
  selectionAnchor.value = "";
}

/** 关闭右键菜单 */
function closeContextMenu() {
  contextMenu.open = false;
}

/** 全局按键：F5 刷新当前目录，键入触发快速定位，Esc 清空文件列表选择 */
function onFileKeyDown(event: KeyboardEvent) {
  if (event.key === "F5") {
    // App 层已拦截浏览器刷新，这里借 F5 刷新文件管理；终端聚焦时 F5 归终端使用
    const target = event.target as HTMLElement;
    if (target.closest?.(".xterm")) return;
    if (props.connected && !dialog.open) refresh();
    return;
  }
  if (handleTypeaheadKey(event)) return;
  if (handleNavKey(event)) return;
  // 存在打开中的模态弹窗时，ESC 交由弹窗关闭，避免同时清空文件选择
  if (event.key !== "Escape" || dialog.open || hasOpenModal()) return;
  if (contextMenu.open) {
    closeContextMenu();
    return;
  }
  clearSelection();
  marquee.active = false;
  fileDrag.active = false;
  fileDrag.target = "";
}

/**
 * 处理上下箭头与 PageUp/PageDown 导航：移动选中项到上一条/下一条，
 * 无选中时默认选中首项，到首尾停止不循环，返回 true 表示按键已消费
 */
function handleNavKey(event: KeyboardEvent): boolean {
  if (!props.active || !props.connected || dialog.open || contextMenu.open) return false;
  // 存在其他模态弹窗（如连接管理器）时不响应，避免后台文件列表被方向键误操作
  if (hasOpenModal()) return false;
  const target = event.target as HTMLElement;
  if (target.closest?.("input, textarea, select, .xterm")) return false;
  if (target.closest?.(".dir-tree") || (activeZone === "tree" && target === document.body)) {
    return handleTreeNavKey(event);
  }
  const step = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : event.key === "PageDown" ? PAGE_STEP : event.key === "PageUp" ? -PAGE_STEP : 0;
  if (step === 0) return false;
  const names = selectableEntries.value.map((entry) => entry.name);
  if (names.length === 0) {
    event.preventDefault();
    return true;
  }
  // 以锚点为基准移动；锚点丢失时回退到当前选中项在可见顺序中的最后一个
  let current = names.indexOf(selectionAnchor.value);
  if (current < 0) {
    for (let i = names.length - 1; i >= 0; i--) {
      if (selectedNames.value.has(names[i])) {
        current = i;
        break;
      }
    }
  }
  // 无选中状态时上下键默认选中首项，否则到首尾停止不循环
  const next = current < 0 ? 0 : Math.max(0, Math.min(names.length - 1, current + step));
  selectSingle(names[next]);
  nextTick(() => {
    entryRows()
      .find((row) => row.dataset.name === names[next])
      ?.scrollIntoView({ block: "nearest" });
  });
  event.preventDefault();
  return true;
}

/** 处理目录树方向键与回车：上下移动，左右展开收起，回车切换展开状态 */
function handleTreeNavKey(event: KeyboardEvent): boolean {
  const nodes = treeNodes.value;
  let visiblePath = cwd.value;
  let current = nodes.findIndex((node) => node.path === visiblePath);
  while (current < 0 && visiblePath !== "/") {
    visiblePath = parentPath(visiblePath);
    current = nodes.findIndex((node) => node.path === visiblePath);
  }
  current = Math.max(current, 0);
  const step = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : event.key === "PageDown" ? PAGE_STEP : event.key === "PageUp" ? -PAGE_STEP : 0;
  if (step !== 0) {
    const next = Math.max(0, Math.min(nodes.length - 1, current + step));
    if (nodes[next]) selectTreeNode(nodes[next].path);
    scrollActiveTreeNodeIntoView();
  } else if (event.key === "ArrowRight") {
    const node = nodes[current];
    if (node && !expandedDirs.value.has(node.path)) toggleTreeNode(node.path);
  } else if (event.key === "ArrowLeft") {
    const node = nodes[current];
    if (node && node.path !== "/" && expandedDirs.value.has(node.path)) {
      toggleTreeNode(node.path);
    } else if (node && node.path !== "/") {
      selectTreeNode(parentPath(node.path));
      scrollActiveTreeNodeIntoView();
    }
  } else if (event.key === "Enter") {
    const node = nodes[current];
    if (node && node.path !== "/") toggleTreeNode(node.path);
  } else {
    return false;
  }
  event.preventDefault();
  return true;
}

/** 键入快速定位候选：列表区为文件名，树区为可见节点路径 */
const typeaheadMatches = computed<string[]>(() => {
  if (!typeahead.active || !typeahead.keyword) return [];
  const keyword = typeahead.keyword.toLowerCase();
  if (typeahead.zone === "list") {
    return selectableEntries.value
      .filter((entry) => entry.name.toLowerCase().includes(keyword))
      .map((entry) => entry.name);
  }
  return treeNodes.value
    .filter((node) => node.name.toLowerCase().includes(keyword))
    .map((node) => node.path);
});

/** 处理键入快速定位按键，返回 true 表示按键已被消费 */
function handleTypeaheadKey(event: KeyboardEvent): boolean {
  if (!props.connected || dialog.open || contextMenu.open) return false;
  // 存在其他模态弹窗时不响应键入快速定位
  if (hasOpenModal()) return false;
  const target = event.target as HTMLElement;
  if (target.closest?.("input, textarea, select, .xterm")) return false;
  if (typeahead.active) {
    if (event.key === "Escape") {
      endTypeahead();
      event.preventDefault();
      return true;
    }
    if (event.key === "Backspace") {
      typeahead.keyword = typeahead.keyword.slice(0, -1);
      typeahead.index = 0;
      if (typeahead.keyword) applyTypeahead();
      else endTypeahead();
      event.preventDefault();
      return true;
    }
    if (event.key === "Enter") {
      endTypeahead();
      event.preventDefault();
      return true;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      const total = typeaheadMatches.value.length;
      if (total > 0) {
        typeahead.index =
          event.key === "ArrowDown"
            ? (typeahead.index + 1) % total
            : (typeahead.index - 1 + total) % total;
        applyTypeahead();
      }
      event.preventDefault();
      return true;
    }
  }
  // 可打印字符开始或追加检索关键字
  if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
    if (!typeahead.active) {
      typeahead.active = true;
      typeahead.zone = activeZone;
    }
    typeahead.keyword += event.key;
    typeahead.index = 0;
    applyTypeahead();
    event.preventDefault();
    return true;
  }
  return false;
}

/** 将当前命中项选中/高亮并滚动到可视区域 */
function applyTypeahead() {
  const matches = typeaheadMatches.value;
  if (matches.length === 0) {
    if (typeahead.zone === "tree") typeaheadTreePath.value = "";
    return;
  }
  const value = matches[Math.min(typeahead.index, matches.length - 1)];
  if (typeahead.zone === "list") {
    selectSingle(value);
    nextTick(() => {
      entryRows()
        .find((row) => row.dataset.name === value)
        ?.scrollIntoView({ block: "nearest" });
    });
  } else {
    typeaheadTreePath.value = value;
    nextTick(() => {
      treeItems()
        .find((item) => item.dataset.path === value)
        ?.scrollIntoView({ block: "nearest" });
    });
  }
}

/** 结束键入快速定位：树区停留在最后定位的目录上 */
function endTypeahead() {
  const stayPath = typeahead.zone === "tree" ? typeaheadTreePath.value : "";
  cancelTypeahead();
  if (stayPath) setCwd(stayPath);
}

/** 取消键入快速定位（点击等主动操作打断时不触发目录跳转） */
function cancelTypeahead() {
  typeahead.active = false;
  typeahead.keyword = "";
  typeahead.index = 0;
  typeaheadTreePath.value = "";
}

/** 记录最近交互区域为目录树，并打断进行中的快速定位 */
function onTreeZonePointerDown() {
  activeZone = "tree";
  dirTreeRef.value?.focus({ preventScroll: true });
  if (typeahead.active) cancelTypeahead();
}

/** 记录最近交互区域为文件列表，并打断进行中的快速定位 */
function onListZonePointerDown() {
  activeZone = "list";
  fileListRef.value?.focus({ preventScroll: true });
  if (typeahead.active) cancelTypeahead();
}

/** 点击应用任意非菜单区域时关闭右键菜单 */
function onGlobalPointerDown(event: PointerEvent) {
  if (!contextMenu.open) return;
  const target = event.target as HTMLElement;
  if (target.closest(".context-menu")) return;
  closeContextMenu();
}

/** 单选指定条目 */
function selectSingle(name: string) {
  selectedNames.value = new Set([name]);
  selectionAnchor.value = name;
}

/** 切换指定条目的选中状态 */
function toggleSelection(name: string) {
  const next = new Set(selectedNames.value);
  if (next.has(name)) next.delete(name);
  else next.add(name);
  selectedNames.value = next;
  selectionAnchor.value = name;
}

/** 按 Shift 范围选择 */
function selectRange(name: string) {
  const names = selectableEntries.value.map((entry) => entry.name);
  const from = names.indexOf(selectionAnchor.value || name);
  const to = names.indexOf(name);
  if (from < 0 || to < 0) {
    selectSingle(name);
    return;
  }
  const [start, end] = from < to ? [from, to] : [to, from];
  selectedNames.value = new Set(names.slice(start, end + 1));
}

/** 根据修饰键选择条目 */
function selectByMouse(entry: FileEntry, event: MouseEvent | PointerEvent) {
  if (entry.name === "...") return;
  if (event.shiftKey) selectRange(entry.name);
  else if (event.ctrlKey || event.metaKey) toggleSelection(entry.name);
  else selectSingle(entry.name);
}

/** 文件列表行 DOM */
function entryRows(): HTMLElement[] {
  return Array.from(fileListRef.value?.querySelectorAll<HTMLElement>("tbody tr.file-row") ?? []);
}

/** 目录树节点 DOM */
function treeItems(): HTMLElement[] {
  return Array.from(dirTreeRef.value?.querySelectorAll<HTMLElement>(".dir-item") ?? []);
}

/** 按坐标查找命中的元素，避免拖拽浮层遮挡 elementFromPoint */
function hitElement<T extends HTMLElement>(items: T[], x: number, y: number): T | undefined {
  return items.find((item) => {
    const rect = item.getBoundingClientRect();
    return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
  });
}

/** DOM 矩形是否相交 */
function rectsIntersect(a: DOMRect, b: DOMRect): boolean {
  return a.left <= b.right && a.right >= b.left && a.top <= b.bottom && a.bottom >= b.top;
}

/** 按框选矩形更新选中项 */
function updateMarqueeSelection() {
  const rect = new DOMRect(marquee.x, marquee.y, marquee.width, marquee.height);
  const next = new Set(pointerAction?.baseSelected ?? []);
  for (const row of entryRows()) {
    const name = row.dataset.name;
    if (!name || !rectsIntersect(rect, row.getBoundingClientRect())) continue;
    if (pointerAction?.toggle && pointerAction.baseSelected.has(name)) next.delete(name);
    else next.add(name);
  }
  selectedNames.value = next;
}

/** 开始文件行指针交互：已选中项拖拽移动，未选中项拖动框选 */
function onEntryPointerDown(entry: FileEntry, event: PointerEvent) {
  if (event.button !== 0 || entry.name === "...") return;
  const target = event.target as HTMLElement;
  if (target.closest("button") || target.closest(".col-resizer")) return;
  pointerAction = {
    mode: isSelected(entry) && !event.ctrlKey && !event.metaKey && !event.shiftKey ? "drag" : "select",
    entry,
    startX: event.clientX,
    startY: event.clientY,
    moved: false,
    baseSelected: event.ctrlKey || event.metaKey ? new Set(selectedNames.value) : new Set(),
    toggle: event.ctrlKey || event.metaKey,
  };
  window.addEventListener("pointermove", onFilePointerMove);
  window.addEventListener("pointerup", onFilePointerUp, { once: true });
}

/** 行右键：未选中项先切换为单选，确保菜单动作作用于右键目标 */
function onEntryContextMenu(entry: FileEntry, event: MouseEvent) {
  event.preventDefault();
  if (entry.name === "...") clearSelection();
  else if (!isSelected(entry)) selectSingle(entry.name);
  openContextMenu(event);
}

/** 在空白区域打开右键菜单 */
function onFileListContextMenu(event: MouseEvent) {
  event.preventDefault();
  const target = event.target as HTMLElement;
  if (target.closest("tr.file-row") || target.closest("thead")) return;
  openContextMenu(event);
}

/** 定位右键菜单 */
function openContextMenu(event: MouseEvent) {
  contextMenu.open = true;
  contextMenu.x = Math.min(event.clientX, window.innerWidth - CONTEXT_MENU_WIDTH - CONTEXT_MENU_MARGIN);
  contextMenu.y = Math.min(event.clientY, window.innerHeight - CONTEXT_MENU_HEIGHT - CONTEXT_MENU_MARGIN);
  contextMenu.submenuLeft = contextMenu.x + CONTEXT_MENU_WIDTH + CONTEXT_SUBMENU_WIDTH > window.innerWidth - CONTEXT_MENU_MARGIN;
}

/** 开始空白区域框选 */
function onFileListPointerDown(event: PointerEvent) {
  if (event.button !== 0) return;
  const target = event.target as HTMLElement;
  if (target.closest("tr.file-row") || target.closest("thead") || target.closest("button") || target.closest("input")) return;
  pointerAction = {
    mode: "select",
    startX: event.clientX,
    startY: event.clientY,
    moved: false,
    baseSelected: event.ctrlKey || event.metaKey ? new Set(selectedNames.value) : new Set(),
    toggle: event.ctrlKey || event.metaKey,
  };
  if (!event.ctrlKey && !event.metaKey && !event.shiftKey) clearSelection();
  window.addEventListener("pointermove", onFilePointerMove);
  window.addEventListener("pointerup", onFilePointerUp, { once: true });
}

/** 文件行指针移动：超过阈值后进入拖拽或框选 */
function onFilePointerMove(event: PointerEvent) {
  if (!pointerAction) return;
  const dx = event.clientX - pointerAction.startX;
  const dy = event.clientY - pointerAction.startY;
  if (!pointerAction.moved && Math.hypot(dx, dy) < 4) return;
  pointerAction.moved = true;
  if (pointerAction.mode === "drag") {
    fileDrag.active = true;
    fileDrag.count = selectedNames.value.size;
    fileDrag.x = event.clientX + 12;
    fileDrag.y = event.clientY + 12;
    fileDrag.target = findDropTarget(event.clientX, event.clientY);
    return;
  }
  const left = Math.min(pointerAction.startX, event.clientX);
  const top = Math.min(pointerAction.startY, event.clientY);
  marquee.active = true;
  marquee.x = left;
  marquee.y = top;
  marquee.width = Math.abs(dx);
  marquee.height = Math.abs(dy);
  updateMarqueeSelection();
}

/** 文件行指针结束：点击选择或执行拖拽移动 */
async function onFilePointerUp(event: PointerEvent) {
  window.removeEventListener("pointermove", onFilePointerMove);
  if (!pointerAction) return;
  const action = pointerAction;
  pointerAction = null;
  marquee.active = false;
  if (fileDrag.active) {
    const target = fileDrag.target;
    fileDrag.active = false;
    fileDrag.target = "";
    if (target) await moveSelectedTo(target);
    return;
  }
  if (!action.moved && action.entry) selectByMouse(action.entry, event);
}

/** 查找鼠标下可投放的目录 */
function findDropTarget(x: number, y: number): string {
  const treeItem = hitElement(treeItems(), x, y);
  const treePath = treeItem?.dataset.path;
  if (treePath && canDropTo(treePath)) return treePath;

  const row = hitElement(entryRows(), x, y);
  const name = row?.dataset.name;
  if (name === "...") {
    const target = parentPath(cwd.value);
    return canDropTo(target) ? target : "";
  }
  if (!name) return "";
  const entry = visibleEntries.value.find((item) => item.name === name);
  const target = joinPath(cwd.value, name);
  if (!entry?.isDir || selectedNames.value.has(name)) return "";
  return canDropTo(target) ? target : "";
}

/** 判断目标目录是否可接收当前选中项 */
function canDropTo(targetDir: string): boolean {
  if (!targetDir || targetDir === cwd.value) return false;
  return [...selectedNames.value].every((name) => {
    const source = joinPath(cwd.value, name);
    return targetDir !== source && !targetDir.startsWith(`${source}/`);
  });
}

/** 将选中文件移动到目标目录 */
async function moveSelectedTo(targetDir: string) {
  const names = [...selectedNames.value];
  if (names.length === 0) return;
  const message =
    names.length === 1
      ? `是否将「${names[0]}」移动至「${targetDir}」目录？`
      : `是否将 ${names.length} 个文件移动至「${targetDir}」目录？`;
  const confirmed = await showConfirm("移动确认", message);
  if (!confirmed) return;
  try {
    for (const name of names) {
      await sftpRename(props.sessionId, joinPath(cwd.value, name), joinPath(targetDir, name));
    }
    clearSelection();
    invalidateTreeDirs(cwd.value, parentPath(cwd.value), targetDir, parentPath(targetDir));
    await refresh();
    await syncTreeToCwd();
    await reloadExpandedTreeDirs(parentPath(cwd.value), targetDir, parentPath(targetDir));
    await loadTreeDir(targetDir);
  } catch (e) {
    showMessage("移动失败", String(e));
  }
}

/** 执行右键菜单动作 */
async function runMenuAction(item: MenuItem) {
  if (item.disabled || !item.action) return;
  closeContextMenu();
  switch (item.action) {
    case "refresh":
      await refresh();
      break;
    case "edit":
      await onEditText();
      break;
    case "copyPath":
      await copySelectedPath();
      break;
    case "rename":
      if (selectedEntries.value[0]) await onRename(selectedEntries.value[0]);
      break;
    case "uploadFile":
      await onUploadFiles();
      break;
    case "uploadDir":
      await onUploadDirs();
      break;
    case "download":
      await onDownloadSelected();
      break;
    case "packDownload":
      await onPackDownload();
      break;
    case "archiveZip":
      await onCreateArchive("zip");
      break;
    case "archiveTarGz":
      await onCreateArchive("tarGz");
      break;
    case "extractArchive":
      await onExtractArchive();
      break;
    case "newFile":
      await onNewFile();
      break;
    case "newDir":
      await onNewDir();
      break;
    case "delete":
      await onDeleteSelected();
      break;
  }
}

/** 复制单个选中项完整路径 */
async function copySelectedPath() {
  const entry = selectedEntries.value[0];
  if (!entry) return;
  try {
    await navigator.clipboard.writeText(joinPath(cwd.value, entry.name));
  } catch (e) {
    showMessage("复制失败", String(e));
  }
}

/** 打开文本编辑器 */
async function onEditText() {
  const entry = selectedEntries.value[0];
  if (!entry || entry.isDir) return;
  const path = joinPath(cwd.value, entry.name);
  try {
    if (await focusExistingTextEditorWindow(props.sessionId, path)) return;
  } catch (e) {
    showMessage("打开失败", String(e));
    return;
  }
  if (entry.size > 1024 * 1024) {
    const confirmed = await showConfirm("编辑确认", "文件大于 1MB，是否继续打开编辑？");
    if (!confirmed) return;
  }
  try {
    const session = sessionsStore.sessions.find((item) => item.id === props.sessionId);
    await openTextEditorWindow({
      sessionId: props.sessionId,
      sessionName: session?.name || session?.config.host || props.sessionId,
      path,
    });
  } catch (e) {
    showMessage("打开失败", String(e));
  }
}

/** 处理菜单项点击，父级菜单不执行动作，子菜单由鼠标悬停展开 */
function onMenuItemClick(item: MenuItem) {
  if (item.action) runMenuAction(item);
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

/** 切换 sudo 提权模式：成功后以新权限重载当前目录与目录树，失败回滚开关 */
async function toggleSudo(event: Event) {
  const target = event.target as HTMLInputElement;
  const enabled = target.checked;
  if (!props.sessionId || !props.connected) {
    target.checked = false;
    sudoActive.value = false;
    return;
  }
  try {
    await sftpSetSudo(props.sessionId, enabled);
    sudoActive.value = enabled;
    clearSelection();
    treeChildren.value = {};
    expandedDirs.value = new Set(["/"]);
    await refresh();
    await syncTreeToCwd();
  } catch (e) {
    // 提权失败回滚复选框与状态
    target.checked = sudoActive.value;
    showMessage("提权失败", String(e));
  }
}

/** 刷新当前目录 */
async function refresh() {
  if (!props.sessionId || !props.connected) {
    entries.value = [];
    return;
  }
  const version = sessionViewVersion;
  const sessionId = props.sessionId;
  const path = cwd.value;
  loading.value = true;
  error.value = "";
  try {
    const nextEntries = await sftpList(sessionId, path);
    if (version !== sessionViewVersion || sessionId !== props.sessionId || path !== cwd.value) return;
    entries.value = nextEntries;
    treeChildren.value = {
      ...treeChildren.value,
      [path]: nextEntries.filter((e) => e.isDir).sort((a, b) => compareName(a.name, b.name)),
    };
  } catch (e) {
    if (version !== sessionViewVersion || sessionId !== props.sessionId || path !== cwd.value) return;
    error.value = String(e);
    entries.value = [];
  } finally {
    if (version === sessionViewVersion && sessionId === props.sessionId && path === cwd.value) {
      loading.value = false;
    }
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

/** 将文件管理路径同步到终端 */
function syncPathToTerminal() {
  if (!props.connected) return;
  emit("sync-terminal-path", cwd.value);
}

/** 从终端读取当前路径并同步到文件管理 */
function syncPathFromTerminal() {
  if (!props.connected) return;
  emit("sync-file-path");
}

/** 供父组件根据终端路径更新地址栏 */
function setPathFromTerminal(path: string) {
  return setCwd(path);
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

onMounted(async () => {
  // 捕获阶段注册，确保 F5 不被 App 层浏览器快捷键拦截影响
  window.addEventListener("keydown", onFileKeyDown, true);
  window.addEventListener("pointerdown", onGlobalPointerDown);
  // 注册系统文件拖入上传（仅 Tauri 环境有效，浏览器预览下静默失败）
  try {
    unlistenDragDrop = await getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload;
      if (!props.connected) {
        dropHover.value = false;
        return;
      }
      if (payload.type === "enter" || payload.type === "over") {
        dropHover.value = isInFileList(payload.position);
      } else if (payload.type === "drop") {
        const inside = isInFileList(payload.position);
        dropHover.value = false;
        if (inside && payload.paths.length > 0) startUpload(payload.paths);
      } else {
        dropHover.value = false;
      }
    });
  } catch (e) {
    console.warn("注册文件拖拽失败（可能非 Tauri 环境）", e);
  }
  try {
    unlistenEditorSaved = await listen<EditorSavedPayload>("editor://saved", (event) => {
      if (event.payload.sessionId !== props.sessionId) {
        staleSessionIds.add(event.payload.sessionId);
        return;
      }
      if (
        props.connected &&
        parentPath(event.payload.path) === cwd.value
      ) {
        refresh();
      }
    });
  } catch (e) {
    console.warn("注册文本编辑保存事件失败（可能非 Tauri 环境）", e);
  }
});

onBeforeUnmount(() => {
  stopResize?.();
  unlistenDragDrop?.();
  unlistenEditorSaved?.();
  clearTimeout(uploadRefreshTimer);
  window.removeEventListener("pointermove", onFilePointerMove);
  window.removeEventListener("keydown", onFileKeyDown, true);
  window.removeEventListener("pointerdown", onGlobalPointerDown);
});

/** 判断窗口物理坐标是否落在右侧文件列表区域内 */
function isInFileList(position: { x: number; y: number }): boolean {
  const rect = fileListRef.value?.getBoundingClientRect();
  if (!rect) return false;
  // Tauri 拖拽事件坐标为物理像素，需按缩放比转换为 CSS 像素
  const scale = window.devicePixelRatio || 1;
  const x = position.x / scale;
  const y = position.y / scale;
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}

// 当前会话的上传任务完成后延迟刷新当前目录，合并批量任务的密集完成事件
watch(
  () => transfersStore.uploadDoneTick,
  () => {
    const sessionId = transfersStore.uploadDoneSession;
    if (sessionId !== props.sessionId) {
      if (sessionId) staleSessionIds.add(sessionId);
      return;
    }
    if (!props.connected) return;
    const version = sessionViewVersion;
    clearTimeout(uploadRefreshTimer);
    uploadRefreshTimer = setTimeout(() => {
      if (version === sessionViewVersion && sessionId === props.sessionId) {
        refresh();
      } else {
        staleSessionIds.add(sessionId);
      }
    }, 600);
  }
);

/** 上传文件：弹出系统文件选择框（支持多选），任务同步到传输面板 */
async function onUploadFiles() {
  const picked = await openDialog({ multiple: true, title: "选择要上传的文件" });
  if (!picked) return;
  await startUpload(Array.isArray(picked) ? picked : [picked]);
}

/** 上传文件夹：弹出系统文件夹选择框（支持多选） */
async function onUploadDirs() {
  const picked = await openDialog({ directory: true, multiple: true, title: "选择要上传的文件夹" });
  if (!picked) return;
  await startUpload(Array.isArray(picked) ? picked : [picked]);
}

/** 拼接超量传输确认提示文案 */
function buildTransferConfirmMessage(result: TransferCreateResult): string {
  if (result.activeCount > 0) {
    return `本次共 ${result.fileCount} 个文件，加上传输中的 ${result.activeCount} 个任务已超过 50 个，建议打包压缩后传输`;
  }
  return `共 ${result.fileCount} 个文件，超过 50 个建议打包压缩后传输`;
}

/** 确认目标位置同名条目是否覆盖 */
async function confirmOverwrite(existNames: string[]): Promise<boolean> {
  const preview = existNames.slice(0, 5).join("、");
  const suffix = existNames.length > 5 ? ` 等 ${existNames.length} 个条目` : "";
  return showConfirm(
    "覆盖确认",
    `目标位置已存在同名条目：${preview}${suffix}，继续传输将覆盖同名内容，是否覆盖？`,
    "覆盖",
    true
  );
}

/** 创建上传任务：依次确认超量与同名覆盖，全部确认后开始传输 */
async function startUpload(paths: string[]) {
  if (paths.length === 0) return;
  let force = false;
  let overwrite = false;
  try {
    for (;;) {
      const result = await transferUpload(props.sessionId, paths, cwd.value, force, overwrite);
      if (result.needConfirm) {
        if (!(await showConfirm("上传确认", buildTransferConfirmMessage(result), "坚持传输", true))) return;
        force = true;
        continue;
      }
      if (result.existNames.length > 0) {
        if (!(await confirmOverwrite(result.existNames))) return;
        overwrite = true;
        continue;
      }
      break;
    }
  } catch (e) {
    showMessage("上传失败", String(e));
  }
}

/** 下载选中的文件/文件夹：选择本地保存目录，依次确认超量与同名覆盖 */
async function onDownloadSelected() {
  const targets = selectedEntries.value;
  if (targets.length === 0) return;
  const dir = await openDialog({ directory: true, title: "选择保存位置" });
  if (!dir || Array.isArray(dir)) return;
  const items = targets.map((entry) => ({
    path: joinPath(cwd.value, entry.name),
    isDir: entry.isDir,
  }));
  let force = false;
  let overwrite = false;
  try {
    for (;;) {
      const result = await transferDownload(props.sessionId, items, dir, force, overwrite);
      if (result.needConfirm) {
        if (!(await showConfirm("下载确认", buildTransferConfirmMessage(result), "坚持传输", true))) return;
        force = true;
        continue;
      }
      if (result.existNames.length > 0) {
        if (!(await confirmOverwrite(result.existNames))) return;
        overwrite = true;
        continue;
      }
      break;
    }
  } catch (e) {
    showMessage("下载失败", String(e));
  }
}

/** 打包下载：远端 tar 打包为单个压缩包后下载（要求远端存在 tar 命令） */
async function onPackDownload() {
  const targets = selectedEntries.value;
  if (targets.length === 0) return;
  const dirName = cwd.value.split("/").filter(Boolean).pop() ?? "archive";
  const defaultName = targets.length === 1 ? `${targets[0].name}.tar.gz` : `${dirName}.tar.gz`;
  const localPath = await saveDialog({ defaultPath: defaultName, title: "选择保存位置" });
  if (!localPath) return;
  try {
    await transferPackDownload(
      props.sessionId,
      cwd.value,
      targets.map((entry) => entry.name),
      localPath
    );
  } catch (e) {
    showMessage("打包下载失败", String(e));
  }
}

/** 判断文件名是否为当前支持解压的压缩包格式 */
function isExtractableArchive(name: string): boolean {
  const lowerName = name.toLowerCase();
  return lowerName.endsWith(".zip") || lowerName.endsWith(".tar.gz") || lowerName.endsWith(".tgz");
}

/** 将选中条目压缩为当前远端目录下的指定格式 */
async function onCreateArchive(format: ArchiveFormat) {
  const targets = selectedEntries.value;
  if (targets.length === 0) return;
  const suffix = format === "zip" ? ".zip" : ".tar.gz";
  const directoryName = cwd.value.split("/").filter(Boolean).pop() ?? "archive";
  const baseName = targets.length === 1
    ? targets[0].name.replace(/\.(?:tar\.gz|tgz|zip)$/i, "")
    : directoryName;
  const input = await showPrompt(
    format === "zip" ? "压缩为 zip" : "压缩为 tar.gz",
    "请输入压缩包名称",
    "压缩包名称",
    `${baseName}${suffix}`,
    joinPath(cwd.value, "{value}")
  );
  if (!input?.trim()) return;
  let archiveName = input.trim();
  if (!archiveName.toLowerCase().endsWith(suffix)) archiveName += suffix;
  if (archiveName === "." || archiveName === ".." || archiveName.includes("/") || archiveName.includes("\0")) {
    showMessage("压缩失败", "压缩包名称不合法");
    return;
  }
  if (targets.some((entry) => entry.name === archiveName)) {
    showMessage("压缩失败", "压缩包名称不能与选中的文件同名");
    return;
  }
  const existing = entries.value.find((entry) => entry.name === archiveName);
  if (existing?.isDir) {
    showMessage("压缩失败", `当前目录存在同名文件夹「${archiveName}」，请更换压缩包名称`);
    return;
  }
  if (existing) {
    const overwrite = await showConfirm(
      "覆盖确认",
      `当前目录已存在「${archiveName}」，继续压缩将覆盖该文件，是否继续？`,
      "覆盖",
      true
    );
    if (!overwrite) return;
  }

  openDialogState("loading", "压缩中", `正在创建「${archiveName}」，请稍候…`, "");
  try {
    await sftpCreateArchive(
      props.sessionId,
      cwd.value,
      targets.map((entry) => entry.name),
      format,
      archiveName
    );
    await refresh();
    showMessage("压缩完成", `「${archiveName}」已创建`);
  } catch (e) {
    showMessage("压缩失败", String(e));
  }
}

/** 将选中的压缩包解压到当前远端目录 */
async function onExtractArchive() {
  const target = selectedEntries.value[0];
  if (!target || target.isDir || !isExtractableArchive(target.name)) return;
  const confirmed = await showConfirm(
    "解压确认",
    `将「${target.name}」解压到当前目录，可能覆盖同名文件，是否继续？`,
    "继续解压",
    true
  );
  if (!confirmed) return;

  openDialogState("loading", "解压中", `正在解压「${target.name}」，请稍候…`, "");
  try {
    await sftpExtractArchive(props.sessionId, cwd.value, target.name);
    invalidateTreeDirs(cwd.value);
    await refresh();
    showMessage("解压完成", `「${target.name}」已解压到当前目录`);
  } catch (e) {
    showMessage("解压失败", String(e));
  }
}

/** 删除当前选中的条目 */
async function onDeleteSelected() {
  await deleteEntries(selectedEntries.value);
}

/** 批量删除文件或目录（目录连同其中全部内容一并删除），删除中显示进行中弹窗防止误操作 */
async function deleteEntries(targets: FileEntry[]) {
  if (targets.length === 0) return;
  const dirCount = targets.filter((entry) => entry.isDir).length;
  let message: string;
  if (targets.length === 1) {
    message = targets[0].isDir
      ? `是否删除文件夹「${targets[0].name}」？其中的全部内容将一并删除`
      : `是否删除「${targets[0].name}」？`;
  } else {
    message =
      dirCount > 0
        ? `是否删除 ${targets.length} 个项目？其中 ${dirCount} 个文件夹将连同全部内容一并删除`
        : `是否删除 ${targets.length} 个文件？`;
  }
  const confirmed = await showConfirm("删除确认", message, "删除", true);
  if (!confirmed) return;
  openDialogState("loading", "删除中", "正在删除，请稍候…", "");
  try {
    for (const entry of targets) {
      const path = joinPath(cwd.value, entry.name);
      if (entry.isDir) {
        await sftpRemoveDir(props.sessionId, path);
      } else {
        await sftpRemoveFile(props.sessionId, path);
      }
    }
    clearSelection();
    invalidateTreeDirs(cwd.value);
    await refresh();
    showMessage("删除完成", targets.length === 1 ? `「${targets[0].name}」已删除` : `${targets.length} 个项目已删除`);
  } catch (e) {
    showMessage("删除失败", String(e));
  }
}

/** 新建文件 */
async function onNewFile() {
  const name = await showPrompt("新建文件", "请输入新文件名称", "新文件名称", "", joinPath(cwd.value, "{value}"));
  if (!name?.trim()) return;
  try {
    await sftpWrite(props.sessionId, joinPath(cwd.value, name.trim()), []);
    await refresh();
  } catch (e) {
    showMessage("创建失败", String(e));
  }
}

/** 新建目录 */
async function onNewDir() {
  const name = await showPrompt("新建目录", "请输入新目录名称", "新目录名称", "", joinPath(cwd.value, "{value}"));
  if (!name?.trim()) return;
  try {
    await sftpCreateDir(props.sessionId, joinPath(cwd.value, name.trim()));
    invalidateTreeDirs(cwd.value);
    await refresh();
    await syncTreeToCwd();
  } catch (e) {
    showMessage("创建失败", String(e));
  }
}

/** 重命名选中项 */
async function onRename(entry: FileEntry) {
  const newName = await showPrompt("重命名", `将「${entry.name}」重命名为`, "新名称", entry.name);
  if (!newName?.trim() || newName.trim() === entry.name) return;
  try {
    await sftpRename(
      props.sessionId,
      joinPath(cwd.value, entry.name),
      joinPath(cwd.value, newName.trim())
    );
    invalidateTreeDirs(cwd.value);
    await refresh();
  } catch (e) {
    showMessage("重命名失败", String(e));
  }
}

/** 显示提示弹窗 */
function showMessage(title: string, message: string) {
  openDialogState("info", title, message, "确定");
}

/** 显示确认弹窗，可自定义确认按钮文案与红色警示样式 */
function showConfirm(title: string, message: string, confirmText = "确定", danger = false): Promise<boolean> {
  return new Promise((resolve) => {
    openDialogState("confirm", title, message, confirmText, "", "", "", (value) => resolve(value === true), danger);
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
  resolve?: DialogState["resolve"],
  confirmDanger = false
) {
  Object.assign(dialog, { open: true, type, title, message, confirmText, placeholder, defaultValue, hintTemplate, confirmDanger, resolve });
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

/** 单个会话的文件管理界面状态（切换会话时缓存、切回时恢复） */
type SessionUiState = {
  cwd: string;
  sudoActive: boolean;
  entries: FileEntry[];
  error: string;
  treeChildren: Record<string, FileEntry[]>;
  expandedDirs: Set<string>;
};

/** 各会话的界面状态缓存 */
const sessionUiStates = new Map<string, SessionUiState>();
/** 已知发生文件变更、切回时需要刷新列表的会话 */
const staleSessionIds = new Set<string>();
/** 上一个活动会话标识，用于切换时保存其界面状态 */
let lastSessionId = "";
/** 当前文件管理视图版本，用于丢弃选项卡切换前返回的异步结果 */
let sessionViewVersion = 0;

/** 保存指定会话的界面状态（会话已关闭时清除缓存） */
function stashSessionUiState(id: string) {
  if (!id) return;
  if (!sessionsStore.sessions.some((s) => s.id === id)) {
    sessionUiStates.delete(id);
    return;
  }
  // 切换发生在异步加载完成前时不缓存半成品，下次进入重新初始化
  if (loading.value) {
    sessionUiStates.delete(id);
    return;
  }
  sessionUiStates.set(id, {
    cwd: cwd.value,
    sudoActive: sudoActive.value,
    entries: entries.value,
    error: error.value,
    treeChildren: treeChildren.value,
    expandedDirs: expandedDirs.value,
  });
}

// 会话切换或连接成功后，恢复该会话的界面状态或定位到主目录
watch(
  () => [props.sessionId, props.connected] as const,
  async ([id, conn]) => {
    const version = ++sessionViewVersion;
    if (lastSessionId !== id) {
      stashSessionUiState(lastSessionId);
      lastSessionId = id;
    }
    clearSelection();
    cancelTypeahead();
    if (id && conn) {
      const cached = sessionUiStates.get(id);
      if (cached) {
        // 切回已浏览过的会话：恢复目录、sudo 开关与树展开状态（后端 sudo 状态本就按会话保留）
        cwd.value = cached.cwd;
        sudoActive.value = cached.sudoActive;
        entries.value = cached.entries;
        error.value = cached.error;
        treeChildren.value = cached.treeChildren;
        expandedDirs.value = cached.expandedDirs;
        loading.value = false;
        if (staleSessionIds.delete(id)) await refresh();
        return;
      }
      loading.value = true;
      error.value = "";
      sudoActive.value = false;
      staleSessionIds.delete(id);
      let home = "/";
      try {
        home = await sftpHome(id);
      } catch {
        // 主目录解析失败时沿用根目录，再由列表加载展示具体错误
      }
      if (version !== sessionViewVersion) return;
      cwd.value = home;
      treeChildren.value = {};
      expandedDirs.value = new Set(["/"]);
      await refresh();
      if (version !== sessionViewVersion) return;
      await syncTreeToCwd();
    } else {
      // 断开或无会话：清空展示与该会话的缓存
      sessionUiStates.delete(id);
      staleSessionIds.delete(id);
      sudoActive.value = false;
      entries.value = [];
      treeChildren.value = {};
    }
  },
  { immediate: true }
);

// 终端断开或选项卡关闭时清理对应前端缓存，重连后首次进入重新加载
watch(
  () => sessionsStore.sessions.map((session) => `${session.id}:${session.status}`),
  () => {
    const connectedIds = new Set(
      sessionsStore.sessions
        .filter((session) => session.status === "connected")
        .map((session) => session.id)
    );
    for (const id of sessionUiStates.keys()) {
      if (!connectedIds.has(id)) sessionUiStates.delete(id);
    }
    for (const id of staleSessionIds) {
      if (!connectedIds.has(id)) staleSessionIds.delete(id);
    }
  }
);

defineExpose({ setPathFromTerminal });
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
      <button class="ic sync" title="同步地址栏路径到终端" :disabled="!connected" @click="syncPathToTerminal">
        <Icon name="pathToTerminal" :size="15" />
      </button>
      <button class="ic sync" title="同步终端路径到地址栏" :disabled="!connected" @click="syncPathFromTerminal">
        <Icon name="pathToFile" :size="15" />
      </button>
      <label class="sudo-toggle" :class="{ on: sudoActive }" title="以 sudo 提权执行文件操作（谨慎使用）">
        <input type="checkbox" :checked="sudoActive" :disabled="!connected" @change="toggleSudo" />
        <span>sudo</span>
      </label>
    </div>

    <div class="file-body">
      <!-- 目录树 -->
      <div
        ref="dirTreeRef"
        class="dir-tree"
        tabindex="0"
        :style="{ flexBasis: `${treeWidth}px`, width: `${treeWidth}px` }"
        @pointerdown.capture="onTreeZonePointerDown"
      >
        <div
          v-for="node in treeNodes"
          :key="node.path"
          :class="['dir-item', { active: cwd === node.path, root: node.path === '/', 'drop-target': fileDrag.target === node.path, locating: typeaheadTreePath === node.path }]"
          :style="{ paddingLeft: `${node.path === '/' ? 4 : 4 + (node.depth - 1) * 16}px` }"
          :title="node.path"
          :data-path="node.path"
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
      <div class="file-list-wrap">
        <div
          ref="fileListRef"
          class="file-list"
          tabindex="0"
          @pointerdown="onFileListPointerDown"
          @pointerdown.capture="onListZonePointerDown"
          @contextmenu="onFileListContextMenu"
        >
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
              class="file-row"
              :class="{ selected: isSelected(entry), 'drop-target': fileDrag.target === (entry.name === '...' ? parentPath(cwd) : joinPath(cwd, entry.name)) }"
              :data-name="entry.name"
              @pointerdown="onEntryPointerDown(entry, $event)"
              @contextmenu="onEntryContextMenu(entry, $event)"
              @dblclick="onOpen(entry)"
            >
              <td class="name" :title="cellTitle(entry, 'name')">
                <Icon
                  :name="entry.isDir ? 'folder' : 'file'"
                  :size="14"
                  :class="entry.isDir ? 'ic-folder' : 'ic-file'"
                />
                <span class="ellipsis">{{ entry.name }}</span>
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
        <div
          v-if="marquee.active"
          class="selection-marquee"
          :style="{ left: `${marquee.x}px`, top: `${marquee.y}px`, width: `${marquee.width}px`, height: `${marquee.height}px` }"
        ></div>
        <div
          v-if="fileDrag.active"
          class="drag-badge"
          :style="{ left: `${fileDrag.x}px`, top: `${fileDrag.y}px` }"
        >
          移动 {{ fileDrag.count }} 项
        </div>
        <div
          v-if="contextMenu.open"
          class="context-menu"
          :style="{ left: `${contextMenu.x}px`, top: `${contextMenu.y}px` }"
          @click.stop
        >
          <div
            v-for="item in contextMenuItems"
            :key="item.key"
            class="context-menu-item"
            :class="{ disabled: item.disabled, 'has-submenu': item.children }"
          >
            <button :disabled="item.disabled" @click.stop="onMenuItemClick(item)">
              <span>{{ item.label }}</span>
              <Icon v-if="item.children" name="chevronRight" :size="11" class="submenu-arrow" />
            </button>
            <div
              v-if="item.children"
              class="context-submenu"
              :class="{ left: contextMenu.submenuLeft }"
            >
              <button
                v-for="child in item.children"
                :key="child.key"
                :disabled="child.disabled"
                @click.stop="onMenuItemClick(child)"
              >
                {{ child.label }}
              </button>
            </div>
          </div>
        </div>
        </div>
        <div v-if="dropHover" class="drop-overlay">松开鼠标上传到当前目录</div>
      </div>
    </div>
    <!-- 键入快速定位提示：显示当前关键字与命中序号 -->
    <div v-if="typeahead.active" class="typeahead-badge">
      <Icon name="search" :size="12" />
      <span class="ta-keyword">{{ typeahead.keyword || "键入以定位" }}</span>
      <span v-if="typeaheadMatches.length" class="ta-count">
        {{ typeahead.index + 1 }}/{{ typeaheadMatches.length }}
      </span>
      <span v-else-if="typeahead.keyword" class="ta-count none">无匹配</span>
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
      :confirm-danger="dialog.confirmDanger"
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
  /* 供键入快速定位提示浮层定位 */
  position: relative;
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
.file-toolbar .ic:disabled {
  color: #aab2bb;
  cursor: not-allowed;
}
.file-toolbar .ic:disabled:hover {
  background: transparent;
  color: #aab2bb;
}
.file-toolbar .sync {
  color: #4d657d;
}
/* sudo 提权复选框：勾选后红背景警示 */
.sudo-toggle {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 22px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: #fff;
  color: #556;
  font-size: 12px;
  cursor: pointer;
  flex-shrink: 0;
  user-select: none;
}
.sudo-toggle input {
  margin: 0;
  cursor: pointer;
}
.sudo-toggle.on {
  background: var(--danger);
  border-color: var(--danger);
  color: #fff;
}
.sudo-toggle:has(input:disabled) {
  opacity: 0.5;
  cursor: not-allowed;
}
.sudo-toggle:has(input:disabled) input {
  cursor: not-allowed;
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
  outline: none;
}
.dir-tree:focus-visible,
.file-list:focus-visible {
  box-shadow: inset 0 0 0 1px #9db8d3;
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
/* 键入快速定位命中的树节点 */
.dir-item.locating {
  background: #fff3cd;
  box-shadow: inset 0 0 0 1px #e0b64a;
}
.dir-item.drop-target {
  background: #cfe2f6;
  box-shadow: inset 0 0 0 1px var(--accent);
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
.file-list-wrap {
  flex: 1;
  min-width: 0;
  position: relative;
  display: flex;
}
.file-list {
  flex: 1;
  overflow: auto;
  min-width: 0;
  outline: none;
}
/* 系统文件拖入提示遮罩 */
.drop-overlay {
  position: absolute;
  inset: 0;
  z-index: 15;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 2px dashed var(--accent);
  background: rgba(91, 142, 197, 0.08);
  color: var(--accent);
  font-size: 13px;
  pointer-events: none;
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
.file-list tbody tr.drop-target td {
  background: #cfe2f6;
  box-shadow: inset 0 0 0 1px var(--accent);
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

.fm-tip {
  padding: 24px;
  text-align: center;
  color: var(--text-muted);
}
/* 键入快速定位提示浮层 */
.typeahead-badge {
  position: absolute;
  left: 50%;
  bottom: 12px;
  transform: translateX(-50%);
  z-index: 25;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 12px;
  border: 1px solid #b8c6d6;
  border-radius: 14px;
  background: rgba(255, 255, 255, 0.97);
  color: #2c5f91;
  font-size: 12px;
  box-shadow: 0 3px 10px rgba(0, 0, 0, 0.18);
  pointer-events: none;
}
.ta-keyword {
  font-weight: 600;
  max-width: 200px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ta-count {
  color: #6d7782;
}
.ta-count.none {
  color: var(--danger);
}
.fm-tip.error {
  color: var(--danger);
}
.selection-marquee {
  position: fixed;
  z-index: 20;
  pointer-events: none;
  border: 1px solid #5b8ec5;
  background: rgba(91, 142, 197, 0.16);
}
.drag-badge {
  position: fixed;
  z-index: 21;
  pointer-events: none;
  padding: 4px 8px;
  border: 1px solid #7fa2c9;
  border-radius: 3px;
  background: #eef6ff;
  color: #2c5f91;
  font-size: 12px;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.18);
}
.context-menu {
  position: fixed;
  z-index: 30;
  width: 152px;
  box-sizing: border-box;
  padding: 4px;
  border: 1px solid #b8c6d6;
  border-radius: 4px;
  background: #fff;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.18);
}
.context-menu-item {
  position: relative;
}
.context-menu button {
  display: flex;
  align-items: center;
  justify-content: space-between;
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
.context-menu button:hover:not(:disabled) {
  background: var(--row-hover);
  color: var(--accent);
}
.context-menu button:disabled {
  color: #aab2bb;
  cursor: not-allowed;
}
.submenu-arrow {
  flex: 0 0 auto;
  color: #7d8792;
}
.context-submenu {
  display: none;
  position: absolute;
  top: -4px;
  left: 100%;
  width: 172px;
  box-sizing: border-box;
  padding: 4px;
  border: 1px solid #b8c6d6;
  border-radius: 4px;
  background: #fff;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.18);
}
.context-submenu.left {
  right: 100%;
  left: auto;
}
.context-menu-item.has-submenu:not(.disabled):hover > .context-submenu {
  display: block;
}
</style>
