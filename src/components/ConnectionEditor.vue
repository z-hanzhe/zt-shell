<script setup lang="ts">
/**
 * 连接编辑弹窗：新增或编辑一条连接配置
 */
import { computed, reactive, ref, watch } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { ConnectionConfig } from "../types";
import { genId } from "../utils";
import { useEscClose } from "../composables/useEscClose";
import Icon from "./Icon.vue";

const props = defineProps<{
  /** 待编辑的连接，null 表示新增 */
  model: ConnectionConfig | null;
}>();

const emit = defineEmits<{
  (e: "save", config: ConnectionConfig): void;
  (e: "cancel"): void;
}>();

type SettingsSectionId = "connection" | "proxy" | "tunnel" | "more";

const settingSections: Array<{ id: SettingsSectionId; label: string }> = [
  { id: "connection", label: "连接配置" },
  { id: "proxy", label: "代理配置" },
  { id: "tunnel", label: "隧道管理" },
  { id: "more", label: "更多设置" },
];

/** 当前编辑分组 */
const activeSection = ref<SettingsSectionId>("connection");
/** 当前编辑分组名称 */
const activeSectionLabel = computed(
  () =>
    settingSections.find((section) => section.id === activeSection.value)?.label ?? "连接配置"
);

/** 表单默认值 */
function defaults(): ConnectionConfig {
  return {
    id: "",
    name: "",
    host: "",
    port: 22,
    username: "root",
    authType: "password",
    password: "",
    privateKeyPath: "",
    passphrase: "",
    parentId: null,
    order: undefined,
  };
}

const form = reactive<ConnectionConfig>(defaults());

// 打开时载入待编辑数据
watch(
  () => props.model,
  (m) => {
    Object.assign(form, defaults(), m ?? {});
    activeSection.value = "connection";
  },
  { immediate: true }
);

/** 打开系统文件选择框并回填私钥路径 */
async function selectPrivateKey() {
  try {
    const picked = await openDialog({
      title: "选择私钥文件",
      defaultPath: form.privateKeyPath?.trim() || undefined,
    });
    if (picked) form.privateKeyPath = picked;
  } catch (error) {
    alert(`选择私钥文件失败：${String(error)}`);
  }
}

/** 提交保存 */
function submit() {
  if (!form.host.trim()) {
    alert("请填写主机地址");
    return;
  }
  if (!form.id) form.id = genId();
  if (!form.name.trim()) form.name = form.host;
  emit("save", { ...form });
}

// ESC 关闭：始终随组件挂载而生效（嵌套于连接管理器之上，栈顶优先关闭本弹窗）
useEscClose(
  () => true,
  () => emit("cancel")
);
</script>

<template>
  <div class="modal-mask">
    <div class="modal connection-editor">
      <div class="modal-header">
        <span>{{ model ? "编辑连接" : "新建连接" }}</span>
        <button class="modal-close" title="关闭" @click="emit('cancel')">×</button>
      </div>
      <div class="editor-body">
        <nav class="settings-sidebar" aria-label="连接配置分组">
          <button
            v-for="section in settingSections"
            :key="section.id"
            class="settings-nav-item"
            :class="{ active: activeSection === section.id }"
            :aria-current="activeSection === section.id ? 'page' : undefined"
            @click="activeSection = section.id"
          >
            {{ section.label }}
          </button>
        </nav>

        <div class="settings-content">
          <section v-if="activeSection === 'connection'" class="setting-pane">
            <h3>连接配置</h3>
            <div class="form-grid">
              <label>名称</label>
              <input class="input" v-model="form.name" placeholder="连接名称（选填）" />

              <label>主机</label>
              <input class="input" v-model="form.host" placeholder="IP 或域名" />

              <label>端口</label>
              <input class="input" type="number" v-model.number="form.port" />

              <label>用户名</label>
              <input class="input" v-model="form.username" />

              <label>认证方式</label>
              <div class="auth-tabs">
                <button
                  type="button"
                  :class="['auth-tab', { active: form.authType === 'password' }]"
                  @click="form.authType = 'password'"
                >
                  密码
                </button>
                <button
                  type="button"
                  :class="['auth-tab', { active: form.authType === 'privateKey' }]"
                  @click="form.authType = 'privateKey'"
                >
                  私钥
                </button>
              </div>

              <template v-if="form.authType === 'password'">
                <label>密码</label>
                <input class="input" type="password" v-model="form.password" />
              </template>

              <template v-else>
                <label>私钥路径</label>
                <div class="path-field">
                  <input
                    class="input"
                    v-model="form.privateKeyPath"
                    placeholder="如 C:\Users\me\.ssh\id_rsa"
                  />
                  <button
                    class="path-picker"
                    type="button"
                    title="选择私钥文件"
                    aria-label="选择私钥文件"
                    @click="selectPrivateKey"
                  >
                    <Icon name="folder" :size="15" />
                  </button>
                </div>
                <label>私钥口令</label>
                <input
                  class="input"
                  type="password"
                  v-model="form.passphrase"
                  placeholder="无口令可留空"
                />
              </template>
            </div>
          </section>

          <section v-else class="setting-pane empty-pane" :aria-label="activeSectionLabel">
            <h3>{{ activeSectionLabel }}</h3>
          </section>
        </div>
      </div>
      <div class="modal-footer">
        <button class="btn" @click="emit('cancel')">取消</button>
        <button class="btn btn-primary" @click="submit">保存</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.connection-editor {
  width: min(680px, calc(100vw - 32px));
  height: min(500px, calc(100vh - 32px));
}
.editor-body {
  display: flex;
  flex: 1;
  min-height: 0;
  overflow: hidden;
}
.settings-sidebar {
  flex: 0 0 142px;
  padding: 12px 8px;
  border-right: 1px solid var(--border);
  background: var(--bg-panel);
}
.settings-nav-item {
  width: 100%;
  height: 32px;
  padding: 0 12px;
  border: none;
  border-radius: var(--radius);
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  text-align: left;
  cursor: pointer;
}
.settings-nav-item:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.settings-nav-item.active {
  background: var(--bg-active);
  color: var(--accent);
  font-weight: 600;
}
.settings-content {
  flex: 1;
  min-width: 0;
  padding: 20px 24px;
  overflow: auto;
}
.setting-pane {
  min-height: 100%;
}
.setting-pane h3 {
  margin: 0 0 18px;
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border-light);
  color: var(--text-primary);
  font-size: 13px;
  font-weight: 600;
}
.empty-pane {
  min-height: 100%;
}
.form-grid {
  display: grid;
  grid-template-columns: 76px minmax(0, 1fr);
  gap: 12px;
  align-items: center;
  max-width: 460px;
}
.form-grid label {
  color: var(--text-secondary);
  text-align: right;
}
.form-grid > .input {
  box-sizing: border-box;
  width: 100%;
  min-width: 0;
}
.auth-tabs {
  display: flex;
  gap: 6px;
}
.auth-tab {
  flex: 1;
  height: 30px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: #fff;
  color: var(--text-secondary);
  cursor: pointer;
}
.auth-tab.active {
  background: linear-gradient(#5e86ad, #4a739c);
  border-color: #4a739c;
  color: #fff;
}
.path-field {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 30px;
  gap: 6px;
  min-width: 0;
}
.path-field .input {
  box-sizing: border-box;
  width: 100%;
  min-width: 0;
}
.path-picker {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 28px;
  padding: 0;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-panel);
  color: var(--text-secondary);
  cursor: pointer;
}
.path-picker:hover {
  border-color: var(--accent);
  background: var(--bg-hover);
  color: var(--accent);
}
</style>
