<script setup lang="ts">
/**
 * 连接管理器弹窗：以文件夹树形分组展示已保存连接，支持新建/编辑/重命名/复制/删除、
 * 分组归类、键盘导航、单选、同级拖拽排序与拖拽移动
 */
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref } from "vue";
import Icon from "./Icon.vue";
import AppDialog from "./AppDialog.vue";
import ConnectionEditor from "./ConnectionEditor.vue";
import { useConnectionsStore } from "../stores/connections";
import { useEscClose } from "../composables/useEscClose";
import type { ConnectionConfig, ConnectionFolder } from "../types";

const emit = defineEmits<{
  (e: "connect", config: ConnectionConfig): void;
  (e: "close"): void;
}>();

const store = useConnectionsStore();

/** 搜索关键字 */
const keyword = ref("");
/** 当前选中项 id（文件夹与连接共用 id 空间） */
const selectedId = ref("");
/** 已展开的文件夹 id 集合 */
const expandedFolders = ref<Set<string>>(new Set());
/** 编辑弹窗状态：undefined 关闭，null 新增，对象为编辑 */
const editing = ref<ConnectionConfig | null | undefined>(undefined);
/** 新建连接时预设的所属文件夹 id */
const editorParentId = ref<string | null>(null);
/** 连接后关闭窗口 */
const closeAfterConnect = ref(true);
/** 列表滚动容器 */
const listRef = ref<HTMLElement | null>(null);

/** 通用弹窗状态 */
const dialog = reactive<DialogState>({
  open: false,
  type: "info",
  title: "",
  message: "",
  defaultValue: "",
  placeholder: "",
  confirmText: "确定",
  confirmDanger: false,
  resolve: undefined,
});
/** 拖拽状态 */
const drag = reactive({
  active: false,
  sourceId: "",
  x: 0,
  y: 0,
  mode: "sort" as DropMode,
  targetId: null as string | null,
  position: "before" as DropPosition,
  valid: false,
});
/** 右键菜单状态 */
const contextMenu = reactive({ open: false, x: 0, y: 0, parentId: null as string | null, submenuLeft: false });

/** 一次指针交互的上下文 */
type PointerAction = {
  id: string;
  startX: number;
  startY: number;
  moved: boolean;
};
let pointerAction: PointerAction | null = null;

/** 扁平树行：文件夹或连接 */
type FolderRow = { kind: "folder"; id: string; name: string; depth: number; parentId: string | null; folder: ConnectionFolder };
type ConnRow = { kind: "conn"; id: string; name: string; depth: number; parentId: string | null; config: ConnectionConfig };
type Row = FolderRow | ConnRow;
type DropMode = "sort" | "move";
type DropPosition = "before" | "after";
type DropTarget =
  | { mode: "sort"; targetId: string; position: DropPosition }
  | { mode: "move"; targetId: string | null };

type MenuAction = "connect" | "edit" | "rename" | "duplicate" | "newConn" | "newFolder" | "delete";
type MenuItem = { key: string; label: string; disabled: boolean; action?: MenuAction; children?: MenuItem[] };
type DialogState = {
  open: boolean;
  type: "info" | "confirm" | "prompt";
  title: string;
  message: string;
  defaultValue: string;
  placeholder: string;
  confirmText: string;
  confirmDanger: boolean;
  resolve?: (value: string | boolean | null) => void;
};

const CONTEXT_MENU_WIDTH = 152;
const CONTEXT_SUBMENU_WIDTH = 132;
const CONTEXT_MENU_HEIGHT = 232;
const CONTEXT_MENU_MARGIN = 8;
/** PageUp/PageDown 一次移动的行数 */
const PAGE_STEP = 10;

/** 是否处于搜索状态（搜索时展开为扁平结果，禁用分组树交互） */
const searching = computed(() => keyword.value.trim().length > 0);

/** 连接 id -> 配置 快速索引 */
const connMap = computed(() => {
  const map = new Map<string, ConnectionConfig>();
  for (const conn of store.connections) map.set(conn.id, conn);
  return map;
});

/** 按名称比较 */
function compareName(a: string, b: string): number {
  return a.localeCompare(b, "zh-Hans-CN", { numeric: true, sensitivity: "base" });
}

/** 读取行排序值 */
function rowOrder(row: Row): number | undefined {
  return row.kind === "folder" ? row.folder.order : row.config.order;
}

/** 行类型兜底排序：旧数据无 order 时保持文件夹在连接前 */
function rowTypeRank(row: Row): number {
  return row.kind === "folder" ? 0 : 1;
}

/** 递归收集某文件夹（含自身）的全部子孙文件夹 id */
function descendantFolderIds(id: string): Set<string> {
  const ids = new Set<string>([id]);
  let changed = true;
  while (changed) {
    changed = false;
    for (const folder of store.folders) {
      if (folder.parentId && ids.has(folder.parentId) && !ids.has(folder.id)) {
        ids.add(folder.id);
        changed = true;
      }
    }
  }
  return ids;
}

/** 比较同级显示顺序 */
function compareRows(a: Row, b: Row): number {
  const orderA = rowOrder(a);
  const orderB = rowOrder(b);
  if (orderA !== undefined || orderB !== undefined) {
    const diff = (orderA ?? Number.MAX_SAFE_INTEGER) - (orderB ?? Number.MAX_SAFE_INTEGER);
    if (diff !== 0) return diff;
  }
  const typeDiff = rowTypeRank(a) - rowTypeRank(b);
  if (typeDiff !== 0) return typeDiff;
  return compareName(a.name, b.name);
}

/** 构建指定父级下的同级行 */
function siblingRows(parentId: string | null, depth: number): Row[] {
  const folderRows = store.folders
    .filter((folder) => folder.parentId === parentId)
    .map<FolderRow>((folder) => ({ kind: "folder", id: folder.id, name: folder.name, depth, parentId, folder }));
  const connRows = store.connections
    .filter((conn) => (conn.parentId ?? null) === parentId)
    .map<ConnRow>((conn) => ({ kind: "conn", id: conn.id, name: conn.name, depth, parentId, config: conn }));
  return [...folderRows, ...connRows].sort(compareRows);
}

/** 递归构建可见行，仅展开的文件夹向下展开 */
function buildRows(parentId: string | null, depth: number, out: Row[]) {
  for (const row of siblingRows(parentId, depth)) {
    out.push(row);
    if (row.kind === "folder" && expandedFolders.value.has(row.id)) buildRows(row.id, depth + 1, out);
  }
}

/** 可见行：搜索时为匹配连接的扁平列表，否则为分组树 */
const visibleRows = computed<Row[]>(() => {
  if (searching.value) {
    const kw = keyword.value.trim().toLowerCase();
    return store.connections
      .filter(
        (c) =>
          c.name.toLowerCase().includes(kw) ||
          c.host.toLowerCase().includes(kw) ||
          c.username.toLowerCase().includes(kw)
      )
      .sort((a, b) => compareName(a.name, b.name))
      .map<ConnRow>((conn) => ({ kind: "conn", id: conn.id, name: conn.name, depth: 0, parentId: conn.parentId ?? null, config: conn }));
  }
  const rows: Row[] = [];
  buildRows(null, 0, rows);
  return rows;
});

/** 可见行索引 */
const rowMap = computed(() => new Map(visibleRows.value.map((row) => [row.id, row])));

/** 当前选中的连接配置 */
const selectedConn = computed(() => (selectedId.value ? connMap.value.get(selectedId.value) : undefined));

/** 选中项是否包含连接 */
const hasConnSelected = computed(() => selectedConn.value !== undefined);

/** 右键菜单项 */
const contextMenuItems = computed<MenuItem[]>(() => {
  const single = selectedId.value;
  const singleConn = selectedConn.value;
  return [
    { key: "connect", action: "connect", label: "连接", disabled: !hasConnSelected.value },
    { key: "edit", action: "edit", label: "编辑", disabled: !singleConn },
    { key: "rename", action: "rename", label: "重命名", disabled: !single },
    { key: "duplicate", action: "duplicate", label: "复制", disabled: !singleConn },
    {
      key: "new",
      label: "新建",
      disabled: false,
      children: [
        { key: "newConn", action: "newConn", label: "连接", disabled: false },
        { key: "newFolder", action: "newFolder", label: "文件夹", disabled: false },
      ],
    },
    { key: "delete", action: "delete", label: "删除", disabled: !single },
  ];
});

// ESC 逐级关闭：先关右键菜单，再清选择，最后关闭本弹窗；内层编辑/通用弹窗在栈顶时先自行关闭
useEscClose(
  () => true,
  () => {
    if (contextMenu.open) {
      closeContextMenu();
      return;
    }
    if (selectedId.value) {
      clearSelection();
      return;
    }
    emit("close");
  }
);

/** 判断是否选中 */
function isSelected(id: string): boolean {
  return selectedId.value === id;
}

/** 清空选择 */
function clearSelection() {
  selectedId.value = "";
}

/** 单选 */
function selectSingle(id: string) {
  selectedId.value = id;
}

/** 可见行 DOM */
function rowElements(): HTMLElement[] {
  return Array.from(listRef.value?.querySelectorAll<HTMLElement>("tr.conn-row") ?? []);
}

/** 按坐标命中行，避免拖拽浮层遮挡 elementFromPoint */
function hitRow(x: number, y: number): HTMLElement | undefined {
  return rowElements().find((el) => {
    const rect = el.getBoundingClientRect();
    return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
  });
}

/** 行指针按下：单选并准备拖拽排序或移动 */
function onRowPointerDown(row: Row, event: PointerEvent) {
  if (event.button !== 0) return;
  const target = event.target as HTMLElement;
  if (target.closest("button")) return;
  selectSingle(row.id);
  if (searching.value) return;
  pointerAction = {
    id: row.id,
    startX: event.clientX,
    startY: event.clientY,
    moved: false,
  };
  window.addEventListener("pointermove", onPointerMove);
  window.addEventListener("pointerup", onPointerUp);
}

/** 空白区域按下：清空选择 */
function onBlankPointerDown(event: PointerEvent) {
  if (event.button !== 0) return;
  const target = event.target as HTMLElement;
  if (target.closest("tr.conn-row") || target.closest("thead") || target.closest("button")) return;
  clearSelection();
}

/** 指针移动：超过阈值进入拖拽排序或移动 */
function onPointerMove(event: PointerEvent) {
  if (!pointerAction) return;
  const dx = event.clientX - pointerAction.startX;
  const dy = event.clientY - pointerAction.startY;
  if (!pointerAction.moved && Math.hypot(dx, dy) < 4) return;
  pointerAction.moved = true;
  const target = findDropTarget(pointerAction.id, event.clientX, event.clientY);
  drag.active = true;
  drag.sourceId = pointerAction.id;
  drag.x = event.clientX + 12;
  drag.y = event.clientY + 12;
  drag.targetId = target ? target.targetId : null;
  drag.mode = target?.mode ?? "sort";
  drag.position = target?.mode === "sort" ? target.position : "before";
  drag.valid = target !== null;
}

/** 指针抬起：有效落点则排序或移动 */
async function onPointerUp() {
  window.removeEventListener("pointermove", onPointerMove);
  window.removeEventListener("pointerup", onPointerUp);
  if (!pointerAction) return;
  const action = pointerAction;
  pointerAction = null;
  if (drag.active) {
    const mode = drag.mode;
    const targetId = drag.targetId;
    const position = drag.position;
    const valid = drag.valid;
    drag.active = false;
    drag.sourceId = "";
    drag.targetId = null;
    drag.valid = false;
    if (valid && mode === "sort" && targetId) await reorderByDrop(action.id, targetId, position);
    else if (valid && mode === "move") await moveByDrop(action.id, targetId);
  }
}

/** 判断能否将源行移入目标文件夹或根目录 */
function canMoveToParent(source: Row, targetParentId: string | null): boolean {
  if (source.parentId === targetParentId) return false;
  if (source.kind !== "folder" || targetParentId === null) return true;
  return !descendantFolderIds(source.id).has(targetParentId);
}

/** 计算拖拽落点：同级边缘排序，文件夹中间移动，空白处移动到根目录 */
function findDropTarget(sourceId: string, x: number, y: number): DropTarget | null {
  const el = hitRow(x, y);
  const source = rowMap.value.get(sourceId);
  if (!source) return null;
  if (!el) return canMoveToParent(source, null) ? { mode: "move", targetId: null } : null;
  const targetId = el.dataset.id ?? "";
  if (!targetId || targetId === sourceId) return null;
  const target = rowMap.value.get(targetId);
  if (!target) return null;
  const rect = el.getBoundingClientRect();
  const ratio = (y - rect.top) / rect.height;
  if (source.parentId === target.parentId && (ratio <= 0.25 || ratio >= 0.75)) {
    return { mode: "sort", targetId, position: ratio <= 0.25 ? "before" : "after" };
  }
  if (target.kind === "folder" && canMoveToParent(source, target.id)) {
    return { mode: "move", targetId: target.id };
  }
  if (source.parentId === target.parentId) {
    return { mode: "sort", targetId, position: ratio < 0.5 ? "before" : "after" };
  }
  return null;
}

/** 按拖拽落点重排同级行 */
async function reorderByDrop(sourceId: string, targetId: string, position: DropPosition) {
  const source = rowMap.value.get(sourceId);
  const target = rowMap.value.get(targetId);
  if (!source || !target || source.parentId !== target.parentId) return;
  const ids = siblingRows(source.parentId, source.depth).map((row) => row.id);
  const withoutSource = ids.filter((id) => id !== sourceId);
  let targetIndex = withoutSource.indexOf(targetId);
  if (targetIndex < 0) return;
  if (position === "after") targetIndex += 1;
  const orderedIds = [
    ...withoutSource.slice(0, targetIndex),
    sourceId,
    ...withoutSource.slice(targetIndex),
  ];
  await store.reorderItems(source.parentId, orderedIds);
  scrollIntoView(sourceId);
}

/** 按拖拽落点移动到文件夹或根目录 */
async function moveByDrop(sourceId: string, targetParentId: string | null) {
  const source = rowMap.value.get(sourceId);
  if (!source || !canMoveToParent(source, targetParentId)) return;
  await store.moveItems([sourceId], targetParentId);
  if (targetParentId) expandedFolders.value = new Set(expandedFolders.value).add(targetParentId);
  scrollIntoView(sourceId);
}

/** 展开或收起文件夹 */
function toggleFolder(id: string) {
  const next = new Set(expandedFolders.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  expandedFolders.value = next;
}

/** 行双击：文件夹展开收起，连接发起连接 */
function onRowDblClick(row: Row) {
  if (row.kind === "folder") toggleFolder(row.id);
  else connectConns([row.config]);
}

/** 发起一组连接 */
function connectConns(configs: ConnectionConfig[]) {
  if (configs.length === 0) return;
  for (const config of configs) emit("connect", { ...config });
  if (closeAfterConnect.value) emit("close");
}

/** 连接当前选中的连接 */
function onConnectSelected() {
  if (selectedConn.value) connectConns([selectedConn.value]);
}

/** 打开新建连接弹窗 */
function onNewConnection(parentId: string | null) {
  editorParentId.value = parentId;
  editing.value = null;
}

/** 打开编辑弹窗 */
function onEdit() {
  if (selectedConn.value) editing.value = { ...selectedConn.value };
}

/** 保存编辑结果：新建连接落入预设文件夹 */
async function onSave(config: ConnectionConfig) {
  if (editing.value === null) config.parentId = editorParentId.value;
  await store.upsert(config);
  editing.value = undefined;
}

/** 新建文件夹 */
async function onNewFolder(parentId: string | null) {
  const name = await showPrompt("新建文件夹", "请输入文件夹名称", "文件夹名称", "");
  if (!name?.trim()) return;
  const id = await store.upsertFolder({ id: "", name: name.trim(), parentId } as ConnectionFolder);
  // 新建后展开其父文件夹以便可见
  if (parentId) expandedFolders.value = new Set(expandedFolders.value).add(parentId);
  selectSingle(id);
}

/** 重命名选中的文件夹或连接 */
async function onRename() {
  const id = selectedId.value;
  if (!id) return;
  const folder = store.folders.find((f) => f.id === id);
  const conn = connMap.value.get(id);
  const current = folder?.name ?? conn?.name ?? "";
  const name = await showPrompt("重命名", `将 [ ${current} ] 重命名为`, "新名称", current);
  if (!name?.trim() || name.trim() === current) return;
  if (folder) await store.upsertFolder({ ...folder, name: name.trim() });
  else if (conn) await store.upsert({ ...conn, name: name.trim() });
}

/** 复制选中的连接：同级生成一份 [ 原名 - 复制 ]  */
async function onDuplicate() {
  const id = selectedId.value;
  if (!id || !connMap.value.has(id)) return;
  const newId = await store.duplicateConnection(id);
  if (newId) selectSingle(newId);
}

/** 删除选中项：文件夹递归删除其全部内容 */
async function onDelete() {
  const id = selectedId.value;
  if (!id) return;
  const folder = store.folders.find((f) => f.id === id);
  const conn = connMap.value.get(id);
  if (!folder && !conn) return;
  let contentCount = 0;
  if (folder) {
    const contents = store.countFolderContents(folder.id);
    contentCount = contents.connCount + contents.folderCount;
  }
  const message = folder
    ? `是否删除文件夹 [ ${folder.name} ] ？文件夹及其内含的 ${contentCount} 个项目将一并删除`
    : `是否删除连接 [ ${conn?.name} ] ？`;
  const confirmed = await showConfirm("删除确认", message, "删除", true);
  if (!confirmed) return;
  if (folder) await store.removeFolderRecursive(folder.id);
  else if (conn) await store.remove(conn.id);
  clearSelection();
}

/** 右键行：未选中则先单选右键目标，记录新建目标文件夹 */
function onRowContextMenu(row: Row, event: MouseEvent) {
  event.preventDefault();
  if (!isSelected(row.id)) selectSingle(row.id);
  contextMenu.parentId = row.kind === "folder" ? row.id : row.parentId;
  openContextMenu(event);
}

/** 空白区域右键：新建目标为根目录 */
function onBlankContextMenu(event: MouseEvent) {
  event.preventDefault();
  const target = event.target as HTMLElement;
  if (target.closest("tr.conn-row") || target.closest("thead")) return;
  clearSelection();
  contextMenu.parentId = null;
  openContextMenu(event);
}

/** 定位右键菜单 */
function openContextMenu(event: MouseEvent) {
  contextMenu.open = true;
  contextMenu.x = Math.min(event.clientX, window.innerWidth - CONTEXT_MENU_WIDTH - CONTEXT_MENU_MARGIN);
  contextMenu.y = Math.min(event.clientY, window.innerHeight - CONTEXT_MENU_HEIGHT - CONTEXT_MENU_MARGIN);
  contextMenu.submenuLeft = contextMenu.x + CONTEXT_MENU_WIDTH + CONTEXT_SUBMENU_WIDTH > window.innerWidth - CONTEXT_MENU_MARGIN;
}

/** 关闭右键菜单 */
function closeContextMenu() {
  contextMenu.open = false;
}

/** 执行右键菜单动作 */
function runMenuAction(item: MenuItem) {
  if (item.disabled || !item.action) return;
  const parentId = contextMenu.parentId;
  closeContextMenu();
  switch (item.action) {
    case "connect":
      onConnectSelected();
      break;
    case "edit":
      onEdit();
      break;
    case "rename":
      onRename();
      break;
    case "duplicate":
      onDuplicate();
      break;
    case "newConn":
      onNewConnection(parentId);
      break;
    case "newFolder":
      onNewFolder(parentId);
      break;
    case "delete":
      onDelete();
      break;
  }
}

/** 处理菜单项点击，父级菜单不执行动作，子菜单由鼠标悬停展开 */
function onMenuItemClick(item: MenuItem) {
  if (item.action) runMenuAction(item);
}

/** 点击非菜单区域关闭右键菜单 */
function onGlobalPointerDown(event: PointerEvent) {
  if (!contextMenu.open) return;
  if ((event.target as HTMLElement).closest(".context-menu")) return;
  closeContextMenu();
}

/** 全局键盘：方向键导航、回车连接、左右展开收起 */
function onKeyDown(event: KeyboardEvent) {
  if (editing.value !== undefined || dialog.open || contextMenu.open) return;
  const target = event.target as HTMLElement;
  if (target.closest?.("input, textarea, select")) return;
  const key = event.key;
  if (key === "Enter") {
    onEnterKey();
    event.preventDefault();
    return;
  }
  if (key === "ArrowDown" || key === "ArrowUp" || key === "PageDown" || key === "PageUp") {
    navVertical(key);
    event.preventDefault();
    return;
  }
  if (key === "ArrowRight" || key === "ArrowLeft") {
    navHorizontal(key);
    event.preventDefault();
  }
}

/** 回车：选中连接则连接，选中文件夹则展开收起 */
function onEnterKey() {
  if (hasConnSelected.value) {
    onConnectSelected();
    return;
  }
  const id = selectedId.value;
  if (id && store.folders.some((f) => f.id === id)) toggleFolder(id);
}

/** 上下方向键移动选中项，到首尾停止不循环 */
function navVertical(key: string) {
  const ids = visibleRows.value.map((row) => row.id);
  if (ids.length === 0) return;
  const step = key === "ArrowDown" ? 1 : key === "ArrowUp" ? -1 : key === "PageDown" ? PAGE_STEP : -PAGE_STEP;
  const current = ids.indexOf(selectedId.value);
  const next = current < 0 ? 0 : Math.max(0, Math.min(ids.length - 1, current + step));
  selectSingle(ids[next]);
  scrollIntoView(ids[next]);
}

/** 左右方向键：展开收起文件夹或在层级间跳转 */
function navHorizontal(key: string) {
  const id = selectedId.value;
  if (!id) return;
  const row = visibleRows.value.find((r) => r.id === id);
  if (!row) return;
  if (key === "ArrowRight") {
    if (row.kind === "folder" && !expandedFolders.value.has(row.id)) toggleFolder(row.id);
  } else if (row.kind === "folder" && expandedFolders.value.has(row.id)) {
    toggleFolder(row.id);
  } else if (row.parentId) {
    selectSingle(row.parentId);
    scrollIntoView(row.parentId);
  }
}

/** 将指定行滚动到可视区域 */
function scrollIntoView(id: string) {
  nextTick(() => {
    rowElements()
      .find((el) => el.dataset.id === id)
      ?.scrollIntoView({ block: "nearest" });
  });
}

/** 确认弹窗 */
function showConfirm(title: string, message: string, confirmText = "确定", danger = false): Promise<boolean> {
  return new Promise((resolve) => {
    Object.assign(dialog, {
      open: true,
      type: "confirm",
      title,
      message,
      confirmText,
      confirmDanger: danger,
      resolve: (value: string | boolean | null) => resolve(value === true),
    });
  });
}

/** 输入弹窗 */
function showPrompt(title: string, message: string, placeholder: string, defaultValue = ""): Promise<string | null> {
  return new Promise((resolve) => {
    Object.assign(dialog, {
      open: true,
      type: "prompt",
      title,
      message,
      placeholder,
      defaultValue,
      confirmText: "确定",
      confirmDanger: false,
      resolve: (value: string | boolean | null) => resolve(typeof value === "string" ? value : null),
    });
  });
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

onMounted(() => {
  window.addEventListener("keydown", onKeyDown, true);
  window.addEventListener("pointerdown", onGlobalPointerDown);
});

onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKeyDown, true);
  window.removeEventListener("pointerdown", onGlobalPointerDown);
  window.removeEventListener("pointermove", onPointerMove);
  window.removeEventListener("pointerup", onPointerUp);
});
</script>

<template>
  <div class="modal-mask">
    <div class="modal conn-mgr">
      <div class="modal-header">
        <span>连接管理器</span>
        <button class="modal-close" title="关闭" @click="emit('close')">×</button>
      </div>

      <!-- 工具栏 -->
      <div class="toolbar">
        <button class="tool-btn" title="新建连接" @click="onNewConnection(null)">
          <Icon name="plus" :size="16" />
        </button>
        <button class="tool-btn" title="新建文件夹" @click="onNewFolder(null)">
          <Icon name="folder" :size="15" />
        </button>
        <button class="tool-btn" title="编辑" :disabled="!selectedConn" @click="onEdit">
          <Icon name="edit" :size="15" />
        </button>
        <button class="tool-btn" title="删除" :disabled="!selectedId" @click="onDelete">
          <Icon name="trash" :size="15" />
        </button>
        <div class="toolbar-spacer"></div>
        <div class="search-box">
          <Icon name="search" :size="14" />
          <input v-model="keyword" placeholder="搜索" />
        </div>
      </div>

      <!-- 列表 -->
      <div class="conn-list" ref="listRef" @pointerdown="onBlankPointerDown" @contextmenu="onBlankContextMenu">
        <table>
          <colgroup>
            <col style="width: 260px" />
            <col style="width: 150px" />
            <col style="width: 60px" />
            <col style="width: 110px" />
          </colgroup>
          <thead>
            <tr>
              <th>名称</th>
              <th>主机</th>
              <th>端口</th>
              <th>用户名</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="row in visibleRows"
              :key="row.id"
              class="conn-row"
              :class="{
                selected: isSelected(row.id),
                folder: row.kind === 'folder',
                'drag-source': drag.active && drag.sourceId === row.id,
                'drop-before': drag.active && drag.valid && drag.mode === 'sort' && drag.targetId === row.id && drag.position === 'before',
                'drop-after': drag.active && drag.valid && drag.mode === 'sort' && drag.targetId === row.id && drag.position === 'after',
                'drop-into': drag.active && drag.valid && drag.mode === 'move' && drag.targetId === row.id,
              }"
              :data-id="row.id"
              :data-kind="row.kind"
              @pointerdown="onRowPointerDown(row, $event)"
              @contextmenu="onRowContextMenu(row, $event)"
              @dblclick="onRowDblClick(row)"
            >
              <td class="name-col" :title="row.name">
                <span class="name-cell" :style="{ paddingLeft: `${row.depth * 16}px` }">
                  <button
                    v-if="row.kind === 'folder'"
                    class="tree-toggle"
                    @click.stop="toggleFolder(row.id)"
                    @pointerdown.stop
                  >
                    <Icon name="chevronRight" :size="11" :class="{ expanded: expandedFolders.has(row.id) }" />
                  </button>
                  <span v-else class="toggle-spacer"></span>
                  <Icon :name="row.kind === 'folder' ? 'folder' : 'server'" :size="14" :class="row.kind === 'folder' ? 'ic-folder' : 'ic-conn'" />
                  <span class="ellipsis">{{ row.name }}</span>
                </span>
              </td>
              <td class="muted">{{ row.kind === "conn" ? row.config.host : "" }}</td>
              <td class="muted">{{ row.kind === "conn" ? row.config.port : "" }}</td>
              <td class="muted">{{ row.kind === "conn" ? row.config.username : "" }}</td>
            </tr>
            <tr v-if="visibleRows.length === 0">
              <td colspan="4" class="empty-tip">
                {{ searching ? "未找到匹配的连接" : "暂无连接，右键或点击左上角新建" }}
              </td>
            </tr>
          </tbody>
        </table>

        <!-- 拖拽浮层 -->
        <div v-if="drag.active" class="drag-badge" :style="{ left: `${drag.x}px`, top: `${drag.y}px` }">
          {{ drag.valid ? (drag.mode === "move" ? (drag.targetId ? "移动到文件夹" : "移动到根目录") : "调整顺序") : "不可放置" }}
        </div>
        <!-- 右键菜单 -->
        <div
          v-if="contextMenu.open"
          class="context-menu"
          :style="{ left: `${contextMenu.x}px`, top: `${contextMenu.y}px` }"
          @click.stop
          @pointerdown.stop
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
            <div v-if="item.children" class="context-submenu" :class="{ left: contextMenu.submenuLeft }">
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

      <!-- 底部 -->
      <div class="modal-footer conn-footer">
        <label class="check">
          <input type="checkbox" v-model="closeAfterConnect" />
          连接后关闭窗口
        </label>
        <button class="btn btn-primary" :disabled="!hasConnSelected" @click="onConnectSelected">连接</button>
      </div>
    </div>

    <ConnectionEditor
      v-if="editing !== undefined"
      :model="editing"
      @save="onSave"
      @cancel="editing = undefined"
    />

    <AppDialog
      :open="dialog.open"
      :type="dialog.type"
      :title="dialog.title"
      :message="dialog.message"
      :default-value="dialog.defaultValue"
      :placeholder="dialog.placeholder"
      :confirm-text="dialog.confirmText"
      :confirm-danger="dialog.confirmDanger"
      @confirm="onDialogConfirm"
      @cancel="onDialogCancel"
    />
  </div>
</template>

<style scoped>
.conn-mgr {
  width: 660px;
  height: 480px;
}
.toolbar {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 6px 10px;
  border-bottom: 1px solid var(--border-color);
}
.tool-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border: 1px solid transparent;
  border-radius: var(--radius);
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
}
.tool-btn:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.tool-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.toolbar-spacer {
  flex: 1;
}
.search-box {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 8px;
  height: 28px;
  border: 1px solid var(--border-light);
  border-radius: var(--radius);
  background: var(--bg-root);
  color: var(--text-muted);
}
.search-box input {
  border: none;
  background: transparent;
  color: var(--text-primary);
  outline: none;
  width: 140px;
}
.conn-list {
  flex: 1;
  position: relative;
  overflow: auto;
  margin: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius);
}
.conn-list table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
  table-layout: fixed;
}
.conn-list thead th {
  position: sticky;
  top: 0;
  height: 28px;
  padding: 0 8px;
  background: var(--bg-panel-2);
  border-bottom: 1px solid var(--border-color);
  color: var(--text-secondary);
  text-align: left;
  font-weight: 600;
  white-space: nowrap;
  z-index: 1;
}
.conn-list tbody td {
  height: 28px;
  padding: 0 8px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.03);
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.conn-list thead th:first-child,
.conn-list tbody td:first-child {
  padding-left: 6px;
}
.conn-row:hover td {
  background: var(--bg-hover);
}
.conn-row.selected td {
  background: var(--bg-active);
}
.conn-row.drag-source td {
  opacity: 0.58;
}
.conn-row.drop-before td {
  box-shadow: inset 0 2px 0 var(--accent);
}
.conn-row.drop-after td {
  box-shadow: inset 0 -2px 0 var(--accent);
}
.conn-row.drop-into td {
  background: #cfe2f6;
  box-shadow: inset 0 0 0 1px var(--accent);
}
.conn-list td.name-col {
  color: var(--text-primary);
}
.name-cell {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  overflow: hidden;
}
.name-cell .ellipsis {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
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
  color: var(--text-muted);
  cursor: pointer;
  flex: 0 0 14px;
}
.tree-toggle .expanded {
  transform: rotate(90deg);
}
.toggle-spacer {
  flex: 0 0 14px;
}
.ic-folder {
  color: #e0b64a;
  flex: 0 0 auto;
}
.ic-conn {
  color: var(--accent);
  flex: 0 0 auto;
}
.muted {
  color: var(--text-secondary);
}
.empty-tip {
  padding: 40px;
  text-align: center;
  color: var(--text-muted);
}
.conn-footer {
  justify-content: space-between;
}
.check {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--text-secondary);
  cursor: pointer;
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
  width: 132px;
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
