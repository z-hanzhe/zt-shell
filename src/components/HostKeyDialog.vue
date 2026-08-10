<script setup lang="ts">
/**
 * SSH 服务端主机密钥确认弹窗：首次连接允许保存，密钥变化时提供强警示更新
 */
import { computed, nextTick, ref, watch } from "vue";
import type { HostKeyChallenge } from "../types";
import { useDialogDrag } from "../composables/useDialogDrag";
import { useEscClose } from "../composables/useEscClose";
import Icon from "./Icon.vue";

const props = withDefaults(
  defineProps<{
    /** 是否显示弹窗 */
    open: boolean;
    /** 当前等待确认的主机密钥 */
    challenge: HostKeyChallenge | null;
    /** 是否正在使用确认后的密钥重新握手 */
    busy?: boolean;
  }>(),
  { busy: false }
);

const emit = defineEmits<{
  (event: "confirm"): void;
  (event: "cancel"): void;
}>();

/** 取消按钮引用，安全操作默认获得焦点 */
const cancelButton = ref<HTMLButtonElement | null>(null);
/** 主机密钥弹窗拖动控制器，每次重新显示时恢复居中 */
const { dialogRef, onDialogHeaderPointerDown } = useDialogDrag(() => props.open);
/** 当前是否属于已保存密钥发生变化 */
const changed = computed(() => props.challenge?.kind === "changed");
/** 弹窗标题 */
const title = computed(() => (changed.value ? "服务器身份发生变化" : "确认服务器身份"));
/** 主提示 */
const summary = computed(() =>
  changed.value
    ? "服务器提供的主机密钥与本机记录不一致。"
    : "这是 ZTShell 首次连接到这台服务器。"
);
/** 风险说明与核验建议 */
const guidance = computed(() =>
  changed.value
    ? "这可能是服务器重装或密钥轮换，也可能有人正在冒充该服务器。请先通过服务器管理员或云控制台核对新指纹，确认变更后再更新密钥。"
    : "请通过服务器管理员或云控制台核对下方 SHA-256 指纹。确认无误后，ZTShell 会记住该密钥并在后续连接时自动校验。"
);

/** 按目标地址格式展示主机与端口 */
function serverAddress(challenge: HostKeyChallenge): string {
  const host = challenge.host.includes(":") && !challenge.host.startsWith("[")
    ? `[${challenge.host}]`
    : challenge.host;
  return `${host}:${challenge.port}`;
}

/** 请求取消，二次握手进行中不允许关闭弹窗 */
function requestCancel() {
  if (!props.busy) emit("cancel");
}

// 弹窗出现后默认聚焦取消，避免按回车误信任主机密钥
watch(
  [() => props.open, () => props.busy],
  async ([open, busy]) => {
    if (!open || busy) return;
    await nextTick();
    cancelButton.value?.focus();
  },
  { immediate: true }
);

useEscClose(() => props.open, requestCancel);
</script>

<template>
  <div v-if="open && challenge" class="modal-mask host-key-mask">
    <div
      ref="dialogRef"
      class="modal dialog-draggable host-key-dialog"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="host-key-title"
      aria-describedby="host-key-guidance"
    >
      <div class="modal-header dialog-drag-handle" @pointerdown="onDialogHeaderPointerDown">
        <span id="host-key-title">{{ title }}</span>
        <button
          class="modal-close"
          title="取消连接"
          :disabled="busy"
          @click="requestCancel"
        >
          <Icon name="close" :size="15" />
        </button>
      </div>

      <div class="modal-body host-key-body">
        <div :class="['host-key-summary', { danger: changed }]">
          <Icon :name="changed ? 'triangleAlert' : 'shieldCheck'" :size="24" />
          <strong>{{ summary }}</strong>
        </div>

        <dl class="host-key-details">
          <div>
            <dt>服务器</dt>
            <dd>{{ serverAddress(challenge) }}</dd>
          </div>
          <div>
            <dt>密钥类型</dt>
            <dd>{{ challenge.algorithm }}</dd>
          </div>
          <div v-if="changed && challenge.knownFingerprint">
            <dt>原指纹</dt>
            <dd><code>{{ challenge.knownFingerprint }}</code></dd>
          </div>
          <div>
            <dt>{{ changed ? "新指纹" : "SHA-256 指纹" }}</dt>
            <dd><code>{{ challenge.fingerprint }}</code></dd>
          </div>
        </dl>

        <p id="host-key-guidance" class="host-key-guidance">{{ guidance }}</p>
      </div>

      <div class="modal-footer">
        <button ref="cancelButton" class="btn" :disabled="busy" @click="requestCancel">
          取消连接
        </button>
        <button
          :class="['btn', changed ? 'btn-danger' : 'btn-primary']"
          :disabled="busy"
          @click="emit('confirm')"
        >
          <span v-if="busy" class="host-key-spinner"></span>
          <Icon v-else :name="changed ? 'refresh' : 'shieldCheck'" :size="14" />
          {{ busy ? "正在重新验证" : changed ? "更新密钥并连接" : "信任并连接" }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.host-key-mask {
  z-index: 1600;
}
.host-key-dialog {
  width: min(520px, calc(100vw - 32px));
}
.host-key-dialog .modal-footer .btn:last-child {
  min-width: 138px;
}
.modal-close:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
.host-key-body {
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.host-key-summary {
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 34px;
  color: var(--accent);
  font-size: 13px;
}
.host-key-summary.danger {
  color: var(--danger);
}
.host-key-details {
  margin: 0;
}
.host-key-details > div {
  display: grid;
  grid-template-columns: 92px minmax(0, 1fr);
  gap: 10px;
  padding: 8px 0;
  border-bottom: 1px solid var(--border-light);
}
.host-key-details > div:first-child {
  border-top: 1px solid var(--border-light);
}
.host-key-details dt {
  color: var(--text-muted);
}
.host-key-details dd {
  min-width: 0;
  margin: 0;
  color: var(--text);
  word-break: break-all;
}
.host-key-details code {
  font-family: "Cascadia Mono", "Consolas", monospace;
  font-size: 12px;
  user-select: text;
}
.host-key-guidance {
  margin: 0;
  line-height: 1.65;
  color: var(--text-secondary);
}
.host-key-spinner {
  width: 13px;
  height: 13px;
  border: 2px solid rgba(255, 255, 255, 0.45);
  border-top-color: #fff;
  border-radius: 50%;
  animation: host-key-spin 0.8s linear infinite;
}
@keyframes host-key-spin {
  to {
    transform: rotate(360deg);
  }
}
@media (max-width: 540px) {
  .host-key-details > div {
    grid-template-columns: 1fr;
    gap: 4px;
  }
}
</style>
