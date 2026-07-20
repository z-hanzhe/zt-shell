<script setup lang="ts">
/**
 * 连接编辑弹窗：新增或编辑一条连接配置
 */
import { reactive, watch } from "vue";
import type { ConnectionConfig } from "../types";
import { genId } from "../utils";
import { useEscClose } from "../composables/useEscClose";

const props = defineProps<{
  /** 待编辑的连接，null 表示新增 */
  model: ConnectionConfig | null;
}>();

const emit = defineEmits<{
  (e: "save", config: ConnectionConfig): void;
  (e: "cancel"): void;
}>();

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
  },
  { immediate: true }
);

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
    <div class="modal" style="width: 460px">
      <div class="modal-header">
        <span>{{ model ? "编辑连接" : "新建连接" }}</span>
        <button class="modal-close" title="关闭" @click="emit('cancel')">×</button>
      </div>
      <div class="modal-body">
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
              :class="['auth-tab', { active: form.authType === 'password' }]"
              @click="form.authType = 'password'"
            >
              密码
            </button>
            <button
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
            <input
              class="input"
              v-model="form.privateKeyPath"
              placeholder="如 C:\Users\me\.ssh\id_rsa"
            />
            <label>私钥口令</label>
            <input
              class="input"
              type="password"
              v-model="form.passphrase"
              placeholder="无口令可留空"
            />
          </template>
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
.form-grid {
  display: grid;
  grid-template-columns: 72px 1fr;
  gap: 10px 12px;
  align-items: center;
}
.form-grid label {
  color: var(--text-secondary);
  text-align: right;
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
</style>
