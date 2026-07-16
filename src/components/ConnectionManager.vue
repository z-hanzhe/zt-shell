<script setup lang="ts">
/**
 * 连接管理器弹窗：展示已保存连接列表，支持新建/编辑/删除/连接（参考 conn.png）
 */
import { computed, ref } from "vue";
import Icon from "./Icon.vue";
import ConnectionEditor from "./ConnectionEditor.vue";
import { useConnectionsStore } from "../stores/connections";
import type { ConnectionConfig } from "../types";

const emit = defineEmits<{
  (e: "connect", config: ConnectionConfig): void;
  (e: "close"): void;
}>();

const store = useConnectionsStore();

/** 搜索关键字 */
const keyword = ref("");
/** 当前选中的连接 id */
const selectedId = ref("");
/** 编辑弹窗状态：undefined 关闭，null 新增，对象为编辑 */
const editing = ref<ConnectionConfig | null | undefined>(undefined);
/** 连接后关闭窗口 */
const closeAfterConnect = ref(true);

/** 关键字过滤后的连接列表 */
const filtered = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  if (!kw) return store.connections;
  return store.connections.filter(
    (c) =>
      c.name.toLowerCase().includes(kw) ||
      c.host.toLowerCase().includes(kw) ||
      c.username.toLowerCase().includes(kw)
  );
});

/** 打开新建弹窗 */
function onNew() {
  editing.value = null;
}

/** 打开编辑弹窗 */
function onEdit(config: ConnectionConfig) {
  editing.value = { ...config };
}

/** 保存编辑结果 */
async function onSave(config: ConnectionConfig) {
  await store.upsert(config);
  editing.value = undefined;
}

/** 删除选中连接 */
async function onDelete() {
  const target = store.connections.find((c) => c.id === selectedId.value);
  if (!target) return;
  if (confirm(`确定删除连接「${target.name}」？`)) {
    await store.remove(target.id);
    selectedId.value = "";
  }
}

/** 发起连接 */
function onConnect(config: ConnectionConfig) {
  emit("connect", config);
  if (closeAfterConnect.value) emit("close");
}
</script>

<template>
  <div class="modal-mask" @mousedown.self="emit('close')">
    <div class="modal conn-mgr">
      <div class="modal-header">
        <span>连接管理器</span>
        <button class="modal-close" title="关闭" @click="emit('close')">×</button>
      </div>

      <!-- 工具栏 -->
      <div class="toolbar">
        <button class="tool-btn" title="新建" @click="onNew">
          <Icon name="plus" :size="16" />
        </button>
        <button
          class="tool-btn"
          title="编辑"
          :disabled="!selectedId"
          @click="onEdit(store.connections.find((c) => c.id === selectedId)!)"
        >
          <Icon name="edit" :size="15" />
        </button>
        <button
          class="tool-btn"
          title="删除"
          :disabled="!selectedId"
          @click="onDelete"
        >
          <Icon name="trash" :size="15" />
        </button>
        <div class="toolbar-spacer"></div>
        <div class="search-box">
          <Icon name="search" :size="14" />
          <input v-model="keyword" placeholder="搜索" />
        </div>
      </div>

      <!-- 列表 -->
      <div class="conn-list">
        <div class="list-header">
          <span class="col-name">名称</span>
          <span class="col-host">主机</span>
          <span class="col-port">端口</span>
          <span class="col-user">用户名</span>
        </div>
        <div class="list-body">
          <div
            v-for="c in filtered"
            :key="c.id"
            :class="['list-row', { selected: selectedId === c.id }]"
            @click="selectedId = c.id"
            @dblclick="onConnect(c)"
          >
            <span class="col-name">
              <Icon name="server" :size="14" />
              {{ c.name }}
            </span>
            <span class="col-host">{{ c.host }}</span>
            <span class="col-port">{{ c.port }}</span>
            <span class="col-user">{{ c.username }}</span>
          </div>
          <div v-if="filtered.length === 0" class="empty-tip">
            暂无连接，点击左上角 + 新建
          </div>
        </div>
      </div>

      <!-- 底部 -->
      <div class="modal-footer conn-footer">
        <label class="check">
          <input type="checkbox" v-model="closeAfterConnect" />
          连接后关闭窗口
        </label>
        <button
          class="btn btn-primary"
          :disabled="!selectedId"
          @click="onConnect(store.connections.find((c) => c.id === selectedId)!)"
        >
          连接
        </button>
      </div>
    </div>

    <ConnectionEditor
      v-if="editing !== undefined"
      :model="editing"
      @save="onSave"
      @cancel="editing = undefined"
    />
  </div>
</template>

<style scoped>
.conn-mgr {
  width: 640px;
  height: 460px;
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
  display: flex;
  flex-direction: column;
  overflow: hidden;
  margin: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius);
}
.list-header {
  display: flex;
  padding: 6px 10px;
  background: var(--bg-panel-2);
  border-bottom: 1px solid var(--border-color);
  color: var(--text-secondary);
  font-size: 12px;
}
.list-body {
  flex: 1;
  overflow: auto;
}
.list-row {
  display: flex;
  align-items: center;
  padding: 7px 10px;
  cursor: pointer;
  border-bottom: 1px solid rgba(255, 255, 255, 0.03);
}
.list-row:hover {
  background: var(--bg-hover);
}
.list-row.selected {
  background: var(--bg-active);
}
.col-name {
  flex: 2;
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--text-primary);
}
.col-host {
  flex: 2;
  color: var(--text-secondary);
}
.col-port {
  flex: 1;
  color: var(--text-secondary);
}
.col-user {
  flex: 1.5;
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
</style>
