<script setup lang="ts">
/**
 * 右下文件管理器：SFTP 目录浏览（左目录树 + 右文件列表），支持上传下载增删改
 */
import { computed, ref, watch } from "vue";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import Icon from "./Icon.vue";
import {
  sftpList,
  sftpHome,
  sftpRemoveFile,
  sftpRemoveDir,
  sftpCreateDir,
  sftpRename,
  sftpUpload,
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

/** 目录树仅展示子目录 */
const dirs = computed(() => entries.value.filter((e) => e.isDir));

/** 依据文件条目推断类型描述 */
function fileType(entry: FileEntry): string {
  if (entry.isDir) return "文件夹";
  if (entry.isSymlink) return "链接";
  const dot = entry.name.lastIndexOf(".");
  if (dot > 0) return `${entry.name.slice(dot + 1).toUpperCase()} 文件`;
  return "文件";
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
  } catch (e) {
    error.value = String(e);
    entries.value = [];
  } finally {
    loading.value = false;
  }
}

/** 进入目录 */
function enterDir(name: string) {
  cwd.value = joinPath(cwd.value, name);
  refresh();
}

/** 双击条目：目录则进入 */
function onOpen(entry: FileEntry) {
  if (entry.isDir) enterDir(entry.name);
}

/** 返回上级目录 */
function goUp() {
  if (cwd.value === "/") return;
  cwd.value = parentPath(cwd.value);
  refresh();
}

/** 手动跳转到输入的路径 */
function goPath(path: string) {
  cwd.value = path.trim() || "/";
  refresh();
}

/** 上传文件 */
async function onUpload() {
  const localPath = await openDialog({ multiple: false, directory: false });
  if (!localPath || typeof localPath !== "string") return;
  const name = localPath.replace(/\\/g, "/").split("/").pop()!;
  try {
    await sftpUpload(props.sessionId, localPath, joinPath(cwd.value, name));
    refresh();
  } catch (e) {
    alert(`上传失败：${e}`);
  }
}

/** 下载选中文件 */
async function onDownload(entry: FileEntry) {
  if (entry.isDir) {
    alert("暂不支持下载目录");
    return;
  }
  const localPath = await saveDialog({ defaultPath: entry.name });
  if (!localPath) return;
  try {
    await sftpDownload(props.sessionId, joinPath(cwd.value, entry.name), localPath);
  } catch (e) {
    alert(`下载失败：${e}`);
  }
}

/** 删除选中项 */
async function onDelete(entry: FileEntry) {
  if (!confirm(`确定删除「${entry.name}」？`)) return;
  const path = joinPath(cwd.value, entry.name);
  try {
    if (entry.isDir) {
      await sftpRemoveDir(props.sessionId, path);
    } else {
      await sftpRemoveFile(props.sessionId, path);
    }
    refresh();
  } catch (e) {
    alert(`删除失败：${e}`);
  }
}

/** 新建目录 */
async function onNewDir() {
  const name = prompt("请输入新目录名称");
  if (!name) return;
  try {
    await sftpCreateDir(props.sessionId, joinPath(cwd.value, name));
    refresh();
  } catch (e) {
    alert(`创建失败：${e}`);
  }
}

/** 重命名选中项 */
async function onRename(entry: FileEntry) {
  const newName = prompt("请输入新名称", entry.name);
  if (!newName || newName === entry.name) return;
  try {
    await sftpRename(
      props.sessionId,
      joinPath(cwd.value, entry.name),
      joinPath(cwd.value, newName)
    );
    refresh();
  } catch (e) {
    alert(`重命名失败：${e}`);
  }
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
      refresh();
    } else {
      entries.value = [];
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
      <input
        class="path-input"
        :value="cwd"
        @keyup.enter="goPath(($event.target as HTMLInputElement).value)"
      />
      <button class="ic" title="上传" @click="onUpload">
        <Icon name="upload" :size="14" />
      </button>
      <button class="ic" title="新建目录" @click="onNewDir">
        <Icon name="plus" :size="14" />
      </button>
    </div>

    <div class="file-body">
      <!-- 目录树 -->
      <div class="dir-tree">
        <div class="dir-item up" @click="goUp" v-if="cwd !== '/'">
          <Icon name="arrowUp" :size="13" /> ..
        </div>
        <div
          v-for="d in dirs"
          :key="d.name"
          class="dir-item"
          @click="enterDir(d.name)"
        >
          <Icon name="folder" :size="13" class="ic-folder" />
          <span class="ellipsis">{{ d.name }}</span>
        </div>
      </div>

      <!-- 文件列表 -->
      <div class="file-list">
        <div v-if="!connected" class="fm-tip">未连接会话</div>
        <div v-else-if="loading" class="fm-tip">加载中…</div>
        <div v-else-if="error" class="fm-tip error">{{ error }}</div>
        <table v-else>
          <thead>
            <tr>
              <th style="width: 32%">文件名</th>
              <th style="width: 11%">大小</th>
              <th style="width: 14%">类型</th>
              <th style="width: 17%">修改时间</th>
              <th style="width: 12%">权限</th>
              <th style="width: 14%">用户/组</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="entry in entries"
              :key="entry.name"
              :class="{ selected: selected === entry.name }"
              @click="selected = entry.name"
              @dblclick="onOpen(entry)"
            >
              <td class="name">
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
                  <button title="重命名" @click.stop="onRename(entry)">
                    <Icon name="edit" :size="12" />
                  </button>
                  <button class="danger" title="删除" @click.stop="onDelete(entry)">
                    <Icon name="trash" :size="12" />
                  </button>
                </span>
              </td>
              <td class="size">{{ entry.isDir ? "" : formatShort(entry.size) }}</td>
              <td>{{ fileType(entry) }}</td>
              <td>{{ formatTime(entry.modified) }}</td>
              <td class="perm mono">{{ entry.permissionsStr }}</td>
              <td class="owner">{{ entry.owner }}/{{ entry.group }}</td>
            </tr>
            <tr v-if="entries.length === 0">
              <td colspan="6" class="fm-tip">空目录</td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
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
  flex: 0 0 150px;
  width: 150px;
  border-right: 1px solid var(--border);
  overflow-y: auto;
  background: #fbfcfd;
  font-size: 12px;
}
.dir-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
  color: #3a3f45;
  cursor: pointer;
  white-space: nowrap;
}
.dir-item:hover {
  background: var(--row-hover);
}
.dir-item.up {
  color: var(--text-muted);
}
.ic-folder {
  color: #e0b64a;
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
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
}
.file-list thead th {
  position: sticky;
  top: 0;
  background: linear-gradient(var(--table-head-top), var(--table-head-bottom));
  color: #3a3f45;
  font-weight: 600;
  text-align: left;
  padding: 5px 10px;
  border-bottom: 1px solid var(--border);
  border-right: 1px solid var(--border-light);
  white-space: nowrap;
  z-index: 1;
}
.file-list tbody td {
  height: 26px;
  padding: 0 10px;
  border-bottom: 1px solid var(--border-light);
  color: #444;
  white-space: nowrap;
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
