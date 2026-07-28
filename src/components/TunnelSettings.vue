<script setup lang="ts">
/**
 * 连接隧道配置：维护当前连接独立的 SSH 隧道列表
 */
import { computed, reactive, ref } from "vue";
import type { TunnelConfig, TunnelType } from "../types";
import { genId } from "../utils";
import { useEscClose } from "../composables/useEscClose";
import AppDialog from "./AppDialog.vue";
import Icon from "./Icon.vue";

const props = defineProps<{
  /** 当前连接的隧道列表 */
  modelValue?: TunnelConfig[];
}>();

const emit = defineEmits<{
  (e: "update:modelValue", value: TunnelConfig[]): void;
}>();

const tunnelTypeLabels: Record<TunnelType, string> = {
  local: "本地（拨出）",
  remote: "远程（传入）",
  dynamic: "动态（SOCKS4/5）",
  dynamicHttp: "动态（HTTP）",
};

/** 当前选中的隧道 id */
const selectedId = ref("");
/** 隧道编辑弹窗状态：undefined 关闭，null 新增，对象为编辑 */
const editing = ref<TunnelConfig | null | undefined>(undefined);
/** 隧道表单校验错误 */
const editorError = ref("");
/** 待删除隧道 */
const deleteTarget = ref<TunnelConfig | null>(null);

/** 当前隧道列表 */
const tunnels = computed(() => props.modelValue ?? []);

/** 当前选中的隧道 */
const selectedTunnel = computed(() =>
  tunnels.value.find((tunnel) => tunnel.id === selectedId.value)
);

/** 当前选中隧道在列表中的位置 */
const selectedIndex = computed(() =>
  tunnels.value.findIndex((tunnel) => tunnel.id === selectedId.value)
);

/** 选中隧道是否可上移 */
const canMoveUp = computed(() => selectedIndex.value > 0);

/** 选中隧道是否可下移 */
const canMoveDown = computed(
  () => selectedIndex.value >= 0 && selectedIndex.value < tunnels.value.length - 1
);

/** 隧道表单默认值 */
function tunnelDefaults(): TunnelConfig {
  return {
    id: "",
    name: "",
    tunnelType: "local",
    enabled: false,
    localOnly: true,
    listenPort: 8080,
    targetHost: "127.0.0.1",
    targetPort: 80,
  };
}

const draft = reactive<TunnelConfig>(tunnelDefaults());

/** 发出更新后的隧道列表 */
function updateTunnels(next: TunnelConfig[]) {
  emit("update:modelValue", next.map((tunnel) => ({ ...tunnel })));
}

/** 判断端口是否有效 */
function isValidPort(port: number | undefined): boolean {
  return typeof port === "number" && Number.isInteger(port) && port >= 1 && port <= 65535;
}

/** 是否为不需要固定目标的动态隧道 */
function isDynamicTunnel(type: TunnelType): boolean {
  return type === "dynamic" || type === "dynamicHttp";
}

/** 隧道目标展示文本 */
function targetText(tunnel: TunnelConfig): string {
  if (tunnel.tunnelType === "dynamic") return "SOCKS4/5";
  if (tunnel.tunnelType === "dynamicHttp") return "HTTP";
  return `${tunnel.targetHost ?? ""}:${tunnel.targetPort ?? ""}`;
}

/** 打开新增隧道弹窗 */
function openCreate() {
  Object.assign(draft, tunnelDefaults());
  editorError.value = "";
  editing.value = null;
}

/** 打开选中隧道的编辑弹窗 */
function openEdit() {
  if (!selectedTunnel.value) return;
  Object.assign(draft, tunnelDefaults(), selectedTunnel.value);
  editorError.value = "";
  editing.value = { ...selectedTunnel.value };
}

/** 关闭隧道编辑弹窗 */
function closeEditor() {
  editing.value = undefined;
  editorError.value = "";
}

/** 保存隧道表单 */
function saveTunnel() {
  const name = draft.name.trim();
  const targetHost = draft.targetHost?.trim() ?? "";
  if (!isValidPort(draft.listenPort)) {
    editorError.value = "监听端口必须在 1 到 65535 之间";
    return;
  }
  if (!isDynamicTunnel(draft.tunnelType)) {
    if (!targetHost) {
      editorError.value = "请填写目标主机";
      return;
    }
    if (!isValidPort(draft.targetPort)) {
      editorError.value = "目标端口必须在 1 到 65535 之间";
      return;
    }
  }

  const id = draft.id || genId();
  const normalized: TunnelConfig = {
    id,
    name: name || `${tunnelTypeLabels[draft.tunnelType]} ${draft.listenPort}`,
    tunnelType: draft.tunnelType,
    enabled: draft.enabled,
    localOnly: draft.localOnly,
    listenPort: draft.listenPort,
    targetHost: isDynamicTunnel(draft.tunnelType) ? undefined : targetHost,
    targetPort: isDynamicTunnel(draft.tunnelType) ? undefined : draft.targetPort,
  };
  const index = tunnels.value.findIndex((tunnel) => tunnel.id === id);
  const next = tunnels.value.map((tunnel) => ({ ...tunnel }));
  if (index >= 0) next[index] = normalized;
  else next.push(normalized);
  updateTunnels(next);
  selectedId.value = id;
  closeEditor();
}

/** 切换隧道启用状态 */
function toggleEnabled(id: string, enabled: boolean) {
  updateTunnels(
    tunnels.value.map((tunnel) =>
      tunnel.id === id ? { ...tunnel, enabled } : { ...tunnel }
    )
  );
}

/** 移动选中隧道排序 */
function moveSelected(direction: "up" | "down") {
  const index = selectedIndex.value;
  const targetIndex = direction === "up" ? index - 1 : index + 1;
  if (index < 0 || targetIndex < 0 || targetIndex >= tunnels.value.length) return;
  const next = tunnels.value.map((tunnel) => ({ ...tunnel }));
  const [item] = next.splice(index, 1);
  next.splice(targetIndex, 0, item);
  updateTunnels(next);
}

/** 请求删除选中的隧道 */
function requestDelete() {
  deleteTarget.value = selectedTunnel.value ? { ...selectedTunnel.value } : null;
}

/** 确认删除隧道 */
function confirmDelete() {
  const target = deleteTarget.value;
  if (!target) return;
  updateTunnels(tunnels.value.filter((tunnel) => tunnel.id !== target.id));
  if (selectedId.value === target.id) selectedId.value = "";
  deleteTarget.value = null;
}

// 隧道编辑弹窗位于连接编辑器之上，ESC 仅关闭最上层弹窗
useEscClose(
  () => editing.value !== undefined,
  closeEditor
);
</script>

<template>
  <section class="tunnel-settings">
    <h3>隧道管理</h3>

    <div class="tunnel-toolbar">
      <button class="tunnel-tool-btn" title="新增隧道" aria-label="新增隧道" @click="openCreate">
        <Icon name="plus" :size="15" />
      </button>
      <button
        class="tunnel-tool-btn"
        title="编辑隧道"
        aria-label="编辑隧道"
        :disabled="!selectedTunnel"
        @click="openEdit"
      >
        <Icon name="edit" :size="14" />
      </button>
      <button
        class="tunnel-tool-btn"
        title="删除隧道"
        aria-label="删除隧道"
        :disabled="!selectedTunnel"
        @click="requestDelete"
      >
        <Icon name="trash" :size="14" />
      </button>
      <button
        class="tunnel-tool-btn"
        title="上移隧道"
        aria-label="上移隧道"
        :disabled="!canMoveUp"
        @click="moveSelected('up')"
      >
        <Icon name="arrowUp" :size="14" />
      </button>
      <button
        class="tunnel-tool-btn"
        title="下移隧道"
        aria-label="下移隧道"
        :disabled="!canMoveDown"
        @click="moveSelected('down')"
      >
        <Icon name="arrowDown" :size="14" />
      </button>
      <span class="tunnel-tip">勾选后才会随 SSH 会话启用</span>
    </div>

    <div class="tunnel-list">
      <div class="tunnel-list-header">
        <span aria-hidden="true"></span>
        <span>名称</span>
        <span>类型</span>
        <span>监听端口</span>
        <span>目标</span>
        <span>限制</span>
      </div>
      <div
        v-for="tunnel in tunnels"
        :key="tunnel.id"
        class="tunnel-row"
        :class="{ selected: selectedId === tunnel.id }"
        :title="`${tunnel.name} - ${targetText(tunnel)}`"
        @click="selectedId = tunnel.id"
      >
        <input
          type="checkbox"
          :checked="tunnel.enabled"
          @click.stop
          @change="toggleEnabled(tunnel.id, ($event.target as HTMLInputElement).checked)"
        />
        <span class="tunnel-name">{{ tunnel.name }}</span>
        <span class="tunnel-muted">{{ tunnelTypeLabels[tunnel.tunnelType] }}</span>
        <span class="tunnel-address">{{ tunnel.listenPort }}</span>
        <span class="tunnel-address">{{ targetText(tunnel) }}</span>
        <span class="tunnel-muted">{{ tunnel.localOnly ? "仅本机" : "所有来源" }}</span>
      </div>
      <div v-if="tunnels.length === 0" class="tunnel-empty">
        暂无隧道配置
      </div>
    </div>

    <div v-if="editing !== undefined" class="modal-mask tunnel-editor-mask">
      <div class="modal tunnel-editor" role="dialog" aria-modal="true">
        <div class="modal-header">
          <span>{{ editing ? "编辑隧道" : "新增隧道" }}</span>
          <button class="modal-close" title="关闭" @click="closeEditor">×</button>
        </div>
        <div class="modal-body tunnel-form">
          <label>名称</label>
          <input class="input" v-model="draft.name" placeholder="隧道名称（选填）" />

          <label>类型</label>
          <select class="input" v-model="draft.tunnelType">
            <option value="local">本地（拨出）</option>
            <option value="remote">远程（传入）</option>
            <option value="dynamic">动态（SOCKS4/5）</option>
            <option value="dynamicHttp">动态（HTTP）</option>
          </select>

          <label>监听端口</label>
          <input class="input" type="number" min="1" max="65535" v-model.number="draft.listenPort" />

          <template v-if="!isDynamicTunnel(draft.tunnelType)">
            <label>目标主机</label>
            <input class="input" v-model="draft.targetHost" placeholder="IP 或域名" />

            <label>目标端口</label>
            <input class="input" type="number" min="1" max="65535" v-model.number="draft.targetPort" />
          </template>

          <label>连接限制</label>
          <label class="check-line">
            <input type="checkbox" v-model="draft.localOnly" />
            仅接受本地连接
          </label>

          <div v-if="editorError" class="tunnel-error">{{ editorError }}</div>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="closeEditor">取消</button>
          <button class="btn btn-primary" @click="saveTunnel">保存</button>
        </div>
      </div>
    </div>

    <AppDialog
      :open="deleteTarget !== null"
      type="confirm"
      title="删除隧道"
      :message="deleteTarget ? `确定删除隧道 [ ${deleteTarget.name} ] 吗？` : ''"
      confirm-text="删除"
      :confirm-danger="true"
      @confirm="confirmDelete"
      @cancel="deleteTarget = null"
    />
  </section>
</template>

<style scoped>
.tunnel-settings {
  display: flex;
  flex-direction: column;
  min-height: 100%;
}
.tunnel-settings h3 {
  margin: 0 0 12px;
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border-light);
  color: var(--text-primary);
  font-size: 13px;
  font-weight: 600;
}
.tunnel-toolbar {
  display: flex;
  align-items: center;
  gap: 4px;
  margin-bottom: 8px;
}
.tunnel-tool-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  padding: 0;
  border: 1px solid transparent;
  border-radius: var(--radius);
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
}
.tunnel-tool-btn:hover:not(:disabled) {
  border-color: var(--border);
  background: var(--bg-hover);
  color: var(--accent);
}
.tunnel-tool-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.tunnel-tip {
  margin-left: auto;
  color: var(--text-muted);
  font-size: 12px;
}
.tunnel-list {
  flex: 1;
  min-height: 0;
  overflow: auto;
  border: 1px solid var(--border);
  border-radius: var(--radius);
}
.tunnel-list-header,
.tunnel-row {
  display: grid;
  grid-template-columns: 48px minmax(96px, 1fr) 112px 72px minmax(130px, 1.1fr) 72px;
  align-items: center;
  min-width: 620px;
}
.tunnel-list-header {
  position: sticky;
  top: 0;
  z-index: 1;
  height: 28px;
  padding: 0 8px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-panel-2);
  color: var(--text-secondary);
  font-weight: 600;
}
.tunnel-row {
  height: 32px;
  padding: 0 8px;
  border-bottom: 1px solid var(--border-light);
  color: var(--text-primary);
  cursor: pointer;
}
.tunnel-row:hover {
  background: var(--bg-hover);
}
.tunnel-row.selected {
  background: var(--bg-active);
}
.tunnel-row input {
  margin: 0;
}
.tunnel-name,
.tunnel-address,
.tunnel-muted {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.tunnel-muted,
.tunnel-address {
  color: var(--text-secondary);
}
.tunnel-address {
  font-family: "Consolas", "Cascadia Mono", monospace;
}
.tunnel-empty {
  padding: 36px 12px;
  color: var(--text-muted);
  text-align: center;
}
.tunnel-editor-mask {
  z-index: 1010;
}
.tunnel-editor {
  width: min(460px, calc(100vw - 32px));
}
.tunnel-form {
  display: grid;
  grid-template-columns: 82px minmax(0, 1fr);
  align-items: center;
  gap: 12px;
}
.tunnel-form label {
  color: var(--text-secondary);
  text-align: right;
}
.tunnel-form .input {
  width: 100%;
  min-width: 0;
}
.check-line {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--text-secondary);
  text-align: left;
}
.tunnel-error {
  grid-column: 2;
  color: var(--danger);
  line-height: 1.5;
}
</style>
