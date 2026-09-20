<script setup lang="ts">
/** 更新代理配置弹窗：隔离编辑草稿，保存后才更新页面状态。 */
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { credentialsGetProxyPassword } from "../api";
import { tryBeginModalAction } from "../composables/modalActionGuard";
import { useDialogDrag } from "../composables/useDialogDrag";
import { useEscClose } from "../composables/useEscClose";
import { useUpdatesStore } from "../stores/updates";
import type { SecretChange } from "../types";

const emit = defineEmits<{ (event: "close"): void }>();
const updates = useUpdatesStore();
const form = reactive({
  useProxy: updates.preferences.useProxy,
  proxyUrl: updates.preferences.proxyUrl,
  proxyUsername: updates.preferences.proxyUsername,
});
const password = ref("");
const initialPassword = ref("");
const loadingPassword = ref(false);
const passwordLoadFailed = ref(false);
let disposed = false;
const saving = ref(false);
const error = ref("");
const enabledInputRef = ref<HTMLInputElement | null>(null);
const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
const locked = computed(() => saving.value || loadingPassword.value || passwordLoadFailed.value || updates.busy || updates.phase === "ready");
const dirty = computed(() =>
  password.value !== initialPassword.value ||
  form.useProxy !== updates.preferences.useProxy ||
  form.proxyUrl !== updates.preferences.proxyUrl ||
  form.proxyUsername !== updates.preferences.proxyUsername
);
const { dialogRef, onDialogHeaderPointerDown } = useDialogDrag();
const { isTop: isTopModal } = useEscClose(() => true, () => close());

/** 聚焦启用开关，便于通过键盘开始配置。 */
function focusDefaultControl(): void {
  if (!isTopModal.value) return;
  (locked.value ? dialogRef.value : enabledInputRef.value)?.focus();
}

/** 将键盘导航限制在当前最上层弹窗内。 */
function trapFocus(event: KeyboardEvent): void {
  if (event.key !== "Tab" || !isTopModal.value) return;
  const controls = Array.from(
    dialogRef.value?.querySelectorAll<HTMLElement>("button:enabled, input:enabled") ?? []
  );
  const current = controls.indexOf(document.activeElement as HTMLElement);
  if (current >= 0 && (event.shiftKey ? current > 0 : current < controls.length - 1)) return;
  event.preventDefault();
  (controls[event.shiftKey ? controls.length - 1 : 0] ?? dialogRef.value)?.focus();
}

/** 打开表单时回填已保存的密码；读取失败时禁止提交空值。 */
async function loadPassword(): Promise<void> {
  if (loadingPassword.value) return;
  loadingPassword.value = true;
  passwordLoadFailed.value = false;
  error.value = "";
  focusDefaultControl();
  try {
    const id = updates.preferences.proxyCredentialId;
    const value = id ? await credentialsGetProxyPassword(id) : null;
    if (disposed) return;
    password.value = value ?? "";
    initialPassword.value = password.value;
  } catch (reason) {
    if (disposed) return;
    passwordLoadFailed.value = true;
    error.value = `读取代理密码失败：${String(reason)}`;
  } finally {
    if (!disposed) loadingPassword.value = false;
  }
  await nextTick();
  focusDefaultControl();
}

/** 取消编辑并丢弃本次草稿，保存期间不允许关闭。 */
function close(event?: Event): void {
  if (saving.value || !isTopModal.value || !tryBeginModalAction(false, event)) return;
  emit("close");
}

/** 保存网络偏好及密码，成功后关闭弹窗但不触发检查。 */
async function save(event: Event): Promise<void> {
  if (locked.value || !dirty.value || !isTopModal.value || !tryBeginModalAction(false, event)) return;
  saving.value = true;
  // 控件禁用期间仍由弹窗持有焦点，保存结束关闭时才能恢复到配置按钮。
  dialogRef.value?.focus();
  error.value = "";
  try {
    const passwordChange: SecretChange = password.value
      ? { mode: "set", value: password.value }
      : { mode: "clear" };
    await updates.savePreferences({ ...updates.preferences, ...form }, passwordChange);
    emit("close");
  } catch (reason) {
    error.value = String(reason);
  } finally {
    saving.value = false;
  }
}

onMounted(loadPassword);
watch(isTopModal, async (isTop) => {
  if (!isTop) return;
  await nextTick();
  focusDefaultControl();
});
onBeforeUnmount(() => {
  disposed = true;
  password.value = "";
  initialPassword.value = "";
  if (dialogRef.value?.contains(document.activeElement) && previousFocus?.isConnected) {
    previousFocus.focus();
  }
});
</script>

<template>
  <Teleport to="body">
    <div
      :class="['modal-mask', { 'modal-top-mask': isTopModal }]"
      :inert="!isTopModal"
      :aria-hidden="isTopModal ? undefined : 'true'"
    >
      <form
        ref="dialogRef"
        class="modal dialog-draggable update-proxy-dialog"
        role="dialog"
        aria-labelledby="update-proxy-dialog-title"
        :aria-modal="isTopModal ? 'true' : 'false'"
        tabindex="-1"
        @keydown="trapFocus"
        @submit.prevent="save"
      >
        <div class="modal-header dialog-drag-handle" @pointerdown="onDialogHeaderPointerDown">
          <span id="update-proxy-dialog-title">网络代理设置</span>
          <button class="modal-close" type="button" title="关闭" :disabled="saving" @click="close">×</button>
        </div>
        <div class="modal-body">
          <label class="proxy-enabled-row">
            <span>使用代理</span>
            <span class="settings-toggle">
              <input ref="enabledInputRef" v-model="form.useProxy" type="checkbox" role="switch" :disabled="locked" />
              <span>{{ form.useProxy ? "启用" : "关闭" }}</span>
            </span>
          </label>
          <fieldset class="proxy-fields" :disabled="locked">
            <label class="proxy-field">
              <span>代理 URL</span>
              <input v-model="form.proxyUrl" class="input" type="url" placeholder="http://127.0.0.1:7897" :required="form.useProxy" />
              <small>支持 HTTP / HTTPS 代理，用于检查和下载更新</small>
            </label>
            <label class="proxy-field">
              <span>代理用户名</span>
              <input v-model="form.proxyUsername" class="input" placeholder="请输入代理用户名" autocomplete="off" />
              <small>选填，代理无需认证时留空</small>
            </label>
            <label class="proxy-field">
              <span>代理密码</span>
              <input v-model="password" class="input" type="password" placeholder="请输入代理密码" autocomplete="off" />
              <small>选填，无需密码时留空</small>
            </label>
          </fieldset>
          <p v-if="error" class="proxy-error" role="alert">{{ error }}</p>
          <button v-if="passwordLoadFailed" class="btn password-retry" type="button" :disabled="loadingPassword" @click="loadPassword">重新读取</button>
        </div>
        <div class="modal-footer">
          <button class="btn" type="button" :disabled="saving" @click="close">取消</button>
          <button class="btn btn-primary" type="submit" :disabled="locked || !dirty">{{ saving ? "保存中" : "保存" }}</button>
        </div>
      </form>
    </div>
  </Teleport>
</template>

<style scoped>
.update-proxy-dialog {
  width: min(460px, calc(100vw - 32px));
  max-height: calc(100vh - var(--titlebar-height) - 24px);
}
.proxy-enabled-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px;
  border: 1px solid var(--border-light);
  border-radius: 5px;
  background: var(--bg-panel);
}
.proxy-fields {
  display: grid;
  gap: 16px;
  min-width: 0;
  margin: 18px 0 0;
  padding: 0;
  border: 0;
}
.proxy-field {
  display: flex;
  flex-direction: column;
  gap: 7px;
  min-width: 0;
}
.proxy-field .input {
  width: 100%;
  height: 32px;
  border-radius: 5px;
}
.password-retry { margin-top: 8px; }
.proxy-field small { color: var(--text-muted); line-height: 1.5; }
.proxy-error { margin: 14px 0 0; color: var(--danger); line-height: 1.6; overflow-wrap: anywhere; }
.update-proxy-dialog :focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
