<script setup lang="ts">
/**
 * 连接代理配置：管理共享代理列表并为当前连接选择代理
 */
import { computed, reactive, ref } from "vue";
import type { ProxyConfig, ProxyType, SecretChange } from "../types";
import { useConnectionsStore } from "../stores/connections";
import { useProxiesStore } from "../stores/proxies";
import { useDialogDrag } from "../composables/useDialogDrag";
import { useEscClose } from "../composables/useEscClose";
import AppDialog from "./AppDialog.vue";
import Icon from "./Icon.vue";
import SecretInput from "./SecretInput.vue";

const props = defineProps<{
  /** 当前连接选择的代理 id */
  modelValue?: string | null;
}>();

const emit = defineEmits<{
  (e: "update:modelValue", value: string | null): void;
}>();

const proxiesStore = useProxiesStore();
const connectionsStore = useConnectionsStore();

const proxyTypeLabels: Record<ProxyType, string> = {
  socks4: "SOCKS4",
  socks4a: "SOCKS4A",
  socks5: "SOCKS5",
  http: "HTTP 1.1",
};

/** 代理搜索关键字 */
const keyword = ref("");
/** 代理编辑弹窗状态：undefined 关闭，null 新增，对象为编辑 */
const editing = ref<ProxyConfig | null | undefined>(undefined);
/** 代理编辑弹窗拖动控制器 */
const { dialogRef, onDialogHeaderPointerDown } = useDialogDrag();
/** 代理表单校验错误 */
const editorError = ref("");
/** 代理密码修改意图 */
const passwordChange = ref<SecretChange>({ mode: "keep" });
/** 是否正在保存代理及其系统凭据 */
const savingProxy = ref(false);
/** 待删除代理 */
const deleteTarget = ref<ProxyConfig | null>(null);

/** 代理表单默认值 */
function proxyDefaults(): ProxyConfig {
  return {
    id: "",
    name: "",
    proxyType: "socks5",
    host: "",
    port: 1080,
    username: "",
  };
}

const draft = reactive<ProxyConfig>(proxyDefaults());

/** 当前连接选择的代理 */
const selectedProxy = computed(() =>
  proxiesStore.proxies.find((proxy) => proxy.id === props.modelValue)
);

/** 当前选中代理在完整列表中的位置 */
const selectedProxyIndex = computed(() =>
  proxiesStore.proxies.findIndex((proxy) => proxy.id === props.modelValue)
);

/** 选中代理是否可上移 */
const canMoveProxyUp = computed(() => selectedProxyIndex.value > 0);

/** 选中代理是否可下移 */
const canMoveProxyDown = computed(
  () => selectedProxyIndex.value >= 0 && selectedProxyIndex.value < proxiesStore.proxies.length - 1
);

/** 按名称、协议或地址筛选代理 */
const filteredProxies = computed(() => {
  const value = keyword.value.trim().toLowerCase();
  if (!value) return proxiesStore.proxies;
  return proxiesStore.proxies.filter((proxy) => {
    const protocol = proxyTypeLabels[proxy.proxyType].toLowerCase();
    return (
      proxy.name.toLowerCase().includes(value) ||
      proxy.host.toLowerCase().includes(value) ||
      protocol.includes(value)
    );
  });
});

/** 删除代理确认文案 */
const deleteMessage = computed(() => {
  if (!deleteTarget.value) return "";
  const count = connectionsStore.countProxyReferences(deleteTarget.value.id);
  if (count > 0) {
    return `代理 [ ${deleteTarget.value.name} ] 正被 ${count} 个连接使用，删除后这些连接将改为不使用代理。`;
  }
  return `确定删除代理 [ ${deleteTarget.value.name} ] 吗？`;
});

/** 选择当前连接使用的代理 */
function selectProxy(id: string | null) {
  emit("update:modelValue", id);
}

/** 移动当前选中代理排序 */
async function moveSelectedProxy(direction: "up" | "down") {
  if (!selectedProxy.value) return;
  await proxiesStore.move(selectedProxy.value.id, direction);
}

/** 打开新增代理弹窗 */
function openCreate() {
  Object.assign(draft, proxyDefaults());
  delete draft.password;
  passwordChange.value = { mode: "keep" };
  editorError.value = "";
  editing.value = null;
}

/** 打开选中代理的编辑弹窗 */
function openEdit() {
  if (!selectedProxy.value) return;
  Object.assign(draft, proxyDefaults(), selectedProxy.value);
  delete draft.password;
  passwordChange.value = { mode: "keep" };
  editorError.value = "";
  editing.value = { ...selectedProxy.value };
}

/** 接收固定掩码输入框的代理密码修改 */
function onPasswordChange(action: SecretChange["mode"], value?: string) {
  if (action === "set") {
    passwordChange.value = value !== undefined ? { mode: "set", value } : { mode: "keep" };
    return;
  }
  passwordChange.value = { mode: action };
}

/** 关闭代理编辑弹窗 */
function closeEditor() {
  editing.value = undefined;
  editorError.value = "";
}

/** 保存代理表单 */
async function saveProxy() {
  if (savingProxy.value) return;
  const name = draft.name.trim();
  const host = draft.host.trim();
  const username = draft.username?.trim() ?? "";
  const supportsPassword = draft.proxyType === "socks5" || draft.proxyType === "http";
  const willHavePassword = supportsPassword && (
    passwordChange.value.mode === "set"
      ? Boolean(passwordChange.value.value)
      : passwordChange.value.mode === "clear"
        ? false
        : draft.hasPassword === true
  );
  if (!name) {
    editorError.value = "请填写代理名称";
    return;
  }
  if (!host) {
    editorError.value = "请填写代理服务器地址";
    return;
  }
  if (!Number.isInteger(draft.port) || draft.port < 1 || draft.port > 65535) {
    editorError.value = "代理端口必须在 1 到 65535 之间";
    return;
  }
  if (
    supportsPassword &&
    Boolean(username) !== willHavePassword
  ) {
    editorError.value = "代理用户名和密码必须同时填写或同时留空";
    return;
  }

  editorError.value = "";
  savingProxy.value = true;
  try {
    const secretChange: SecretChange = supportsPassword ? passwordChange.value : { mode: "clear" };
    const id = await proxiesStore.upsert({
      id: draft.id,
      name,
      proxyType: draft.proxyType,
      host,
      port: draft.port,
      username: username || undefined,
      hasPassword: draft.hasPassword,
    }, secretChange);
    selectProxy(id);
    closeEditor();
  } catch (error) {
    editorError.value = `保存代理失败：${String(error)}`;
  } finally {
    savingProxy.value = false;
  }
}

/** 请求删除选中的代理 */
function requestDelete() {
  deleteTarget.value = selectedProxy.value ? { ...selectedProxy.value } : null;
}

/** 确认删除代理并清除全部连接引用 */
async function confirmDelete() {
  const target = deleteTarget.value;
  if (!target) return;
  deleteTarget.value = null;
  try {
    await proxiesStore.remove(target.id);
    await connectionsStore.clearProxyReferences(target.id);
    if (props.modelValue === target.id) selectProxy(null);
  } catch (error) {
    alert(`删除代理失败：${String(error)}`);
  }
}

// 代理编辑弹窗位于连接编辑器之上，ESC 仅关闭最上层弹窗
const { isTop: isTopModal } = useEscClose(
  () => editing.value !== undefined,
  () => {
    if (!savingProxy.value) closeEditor();
  }
);
</script>

<template>
  <section class="proxy-settings">
    <h3>代理配置</h3>

    <div class="proxy-toolbar">
      <button class="proxy-tool-btn" title="新增代理" aria-label="新增代理" @click="openCreate">
        <Icon name="plus" :size="15" />
      </button>
      <button
        class="proxy-tool-btn"
        title="编辑代理"
        aria-label="编辑代理"
        :disabled="!selectedProxy"
        @click="openEdit"
      >
        <Icon name="edit" :size="14" />
      </button>
      <button
        class="proxy-tool-btn"
        title="删除代理"
        aria-label="删除代理"
        :disabled="!selectedProxy"
        @click="requestDelete"
      >
        <Icon name="trash" :size="14" />
      </button>
      <button
        class="proxy-tool-btn"
        title="上移代理"
        aria-label="上移代理"
        :disabled="!canMoveProxyUp"
        @click="moveSelectedProxy('up')"
      >
        <Icon name="arrowUp" :size="14" />
      </button>
      <button
        class="proxy-tool-btn"
        title="下移代理"
        aria-label="下移代理"
        :disabled="!canMoveProxyDown"
        @click="moveSelectedProxy('down')"
      >
        <Icon name="arrowDown" :size="14" />
      </button>
      <div class="proxy-search">
        <Icon name="search" :size="13" />
        <input v-model="keyword" placeholder="搜索代理" />
      </div>
    </div>

    <div class="proxy-list" role="radiogroup" aria-label="连接代理">
      <div class="proxy-list-header">
        <span></span>
        <span>名称</span>
        <span>协议</span>
        <span>代理服务器</span>
      </div>
      <label class="proxy-row" :class="{ selected: !modelValue }">
        <input
          type="radio"
          name="connection-proxy"
          :checked="!modelValue"
          @change="selectProxy(null)"
        />
        <span class="proxy-name">不使用代理</span>
        <span class="proxy-muted">直连</span>
        <span class="proxy-muted">-</span>
      </label>
      <label
        v-for="proxy in filteredProxies"
        :key="proxy.id"
        class="proxy-row"
        :class="{ selected: modelValue === proxy.id }"
        :title="`${proxy.name} - ${proxy.host}:${proxy.port}`"
      >
        <input
          type="radio"
          name="connection-proxy"
          :checked="modelValue === proxy.id"
          @change="selectProxy(proxy.id)"
        />
        <span class="proxy-name">{{ proxy.name }}</span>
        <span class="proxy-muted">{{ proxyTypeLabels[proxy.proxyType] }}</span>
        <span class="proxy-address">{{ proxy.host }}:{{ proxy.port }}</span>
      </label>
      <div v-if="filteredProxies.length === 0" class="proxy-empty">
        {{ proxiesStore.proxies.length === 0 ? "暂无代理配置" : "未找到匹配的代理" }}
      </div>
    </div>

    <div
      v-if="editing !== undefined"
      :class="['modal-mask', 'proxy-editor-mask', { 'modal-top-mask': isTopModal }]"
      :inert="!isTopModal"
      :aria-hidden="isTopModal ? undefined : 'true'"
    >
      <div
        ref="dialogRef"
        class="modal dialog-draggable proxy-editor"
        role="dialog"
        :aria-modal="isTopModal ? 'true' : 'false'"
      >
        <div class="modal-header dialog-drag-handle" @pointerdown="onDialogHeaderPointerDown">
          <span>{{ editing ? "编辑代理" : "新增代理" }}</span>
          <button class="modal-close" title="关闭" @click="closeEditor">×</button>
        </div>
        <div class="modal-body proxy-form">
          <label>名称</label>
          <input class="input" v-model="draft.name" placeholder="代理名称" />

          <label>协议</label>
          <select class="input" v-model="draft.proxyType">
            <option value="socks4">SOCKS4</option>
            <option value="socks4a">SOCKS4A</option>
            <option value="socks5">SOCKS5</option>
            <option value="http">HTTP 1.1</option>
          </select>

          <label>服务器</label>
          <input class="input" v-model="draft.host" placeholder="IP 或域名" />

          <label>端口</label>
          <input class="input" type="number" min="1" max="65535" v-model.number="draft.port" />

          <template v-if="draft.proxyType === 'socks4' || draft.proxyType === 'socks4a'">
            <label>用户标识</label>
            <input class="input" v-model="draft.username" placeholder="选填" />
          </template>
          <template v-else>
            <label>用户名</label>
            <input class="input" v-model="draft.username" placeholder="选填" />

            <label>密码</label>
            <SecretInput
              :has-secret="draft.hasPassword === true"
              :reset-key="`${draft.id}:proxy-password`"
              placeholder="选填"
              @change="onPasswordChange"
            />
          </template>

          <div v-if="editorError" class="proxy-error">{{ editorError }}</div>
        </div>
        <div class="modal-footer">
          <button class="btn" :disabled="savingProxy" @click="closeEditor">取消</button>
          <button class="btn btn-primary" :disabled="savingProxy" @click="saveProxy">
            {{ savingProxy ? "保存中" : "保存" }}
          </button>
        </div>
      </div>
    </div>

    <AppDialog
      :open="deleteTarget !== null"
      type="confirm"
      title="删除代理"
      :message="deleteMessage"
      confirm-text="删除"
      :confirm-danger="true"
      @confirm="confirmDelete"
      @cancel="deleteTarget = null"
    />
  </section>
</template>

<style scoped>
.proxy-settings {
  display: flex;
  flex-direction: column;
  min-height: 100%;
}
.proxy-settings h3 {
  margin: 0 0 12px;
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border-light);
  color: var(--text-primary);
  font-size: 13px;
  font-weight: 600;
}
.proxy-toolbar {
  display: flex;
  align-items: center;
  gap: 4px;
  margin-bottom: 8px;
}
.proxy-tool-btn {
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
.proxy-tool-btn:hover:not(:disabled) {
  border-color: var(--border);
  background: var(--bg-hover);
  color: var(--accent);
}
.proxy-tool-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.proxy-search {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 170px;
  height: 28px;
  margin-left: auto;
  padding: 0 8px;
  border: 1px solid var(--border-light);
  border-radius: var(--radius);
  color: var(--text-muted);
}
.proxy-search input {
  min-width: 0;
  width: 100%;
  border: none;
  outline: none;
  background: transparent;
  color: var(--text-primary);
  font-size: 12px;
}
.proxy-list {
  flex: 1;
  min-height: 0;
  overflow: auto;
  border: 1px solid var(--border);
  border-radius: var(--radius);
}
.proxy-list-header,
.proxy-row {
  display: grid;
  grid-template-columns: 24px minmax(96px, 1fr) 78px minmax(130px, 1.25fr);
  align-items: center;
  min-width: 430px;
}
.proxy-list-header {
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
.proxy-row {
  height: 32px;
  padding: 0 8px;
  border-bottom: 1px solid var(--border-light);
  color: var(--text-primary);
  cursor: pointer;
}
.proxy-row:hover {
  background: var(--bg-hover);
}
.proxy-row.selected {
  background: var(--bg-active);
}
.proxy-row input {
  margin: 0;
}
.proxy-name,
.proxy-address,
.proxy-muted {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.proxy-muted,
.proxy-address {
  color: var(--text-secondary);
}
.proxy-address {
  font-family: "Consolas", "Cascadia Mono", monospace;
}
.proxy-empty {
  padding: 36px 12px;
  color: var(--text-muted);
  text-align: center;
}
.proxy-editor-mask {
  z-index: 1010;
}
.proxy-editor {
  width: min(440px, calc(100vw - 32px));
}
.proxy-form {
  display: grid;
  grid-template-columns: 72px minmax(0, 1fr);
  align-items: center;
  gap: 12px;
}
.proxy-form label {
  color: var(--text-secondary);
  text-align: right;
}
.proxy-form .input {
  width: 100%;
  min-width: 0;
}
.proxy-error {
  grid-column: 2;
  color: var(--danger);
  line-height: 1.5;
}
</style>
