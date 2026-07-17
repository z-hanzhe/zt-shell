<script setup lang="ts">
/**
 * 独立远程文本编辑窗口：负责读取、编辑和保存单个会话中的远端文件
 */
import {
  computed,
  nextTick,
  onBeforeUnmount,
  onMounted,
  reactive,
  ref,
  shallowRef,
  watch,
} from "vue";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";
import * as monaco from "monaco-editor";
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import jsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker";
import cssWorker from "monaco-editor/esm/vs/language/css/css.worker?worker";
import htmlWorker from "monaco-editor/esm/vs/language/html/html.worker?worker";
import tsWorker from "monaco-editor/esm/vs/language/typescript/ts.worker?worker";
import { sftpCheckWritable, sftpRead, sftpWrite } from "../api";
import { useEscClose } from "../composables/useEscClose";
import TitleBar from "./TitleBar.vue";

declare global {
  interface Window {
    MonacoEnvironment?: monaco.Environment;
  }
}

window.MonacoEnvironment = {
  getWorker(_: string, label: string) {
    if (label === "json") return new jsonWorker();
    if (["css", "scss", "less"].includes(label)) return new cssWorker();
    if (["html", "handlebars", "razor"].includes(label)) return new htmlWorker();
    if (["typescript", "javascript"].includes(label)) return new tsWorker();
    return new editorWorker();
  },
};

const search = new URLSearchParams(window.location.search);
const sessionId = search.get("sessionId") ?? "";
const sessionName = search.get("sessionName") || sessionId;
const path = search.get("path") ?? "";
const appWindow = getCurrentWindow();

/** 编辑器挂载容器 */
const editorContainer = ref<HTMLDivElement | null>(null);
const editor = shallowRef<monaco.editor.IStandaloneCodeEditor>();
/** 最近一次成功读取或保存的内容基线 */
const savedContent = ref("");
const selectedLanguage = ref("plaintext");
const readOnly = ref(false);
const loading = ref(true);
const loadError = ref("");
const saving = ref(false);
const confirmDialog = reactive({
  open: false,
  title: "",
  message: "",
  resolve: undefined as ((value: boolean) => void) | undefined,
});
/** 保存结果弹窗：成功时主按钮退出编辑器，叉号仅关闭弹窗 */
const resultDialog = reactive({ open: false, title: "", message: "", exit: false });
/** 是否由组件主动销毁窗口，避免再次触发关闭确认 */
let destroying = false;
let unlistenCloseRequested: UnlistenFn | undefined;

useEscClose(() => resultDialog.open, () => dismissResult());
useEscClose(() => confirmDialog.open, () => confirmAction(false));

const detectedLanguage = computed(() => detectLanguage(path));
const editorWindowTitle = computed(
  () => `${readOnly.value ? "[只读] " : ""}${sessionName}：${path}`
);
const languageOptions = [
  { value: "plaintext", label: "Plain Text" },
  { value: "json", label: "JSON" },
  { value: "yaml", label: "YAML" },
  { value: "java", label: "Java" },
  { value: "javascript", label: "JavaScript" },
  { value: "typescript", label: "TypeScript" },
  { value: "css", label: "CSS" },
  { value: "html", label: "HTML" },
  { value: "xml", label: "XML" },
  { value: "markdown", label: "Markdown" },
  { value: "shell", label: "Shell" },
  { value: "python", label: "Python" },
  { value: "rust", label: "Rust" },
  { value: "sql", label: "SQL" },
];

watch(selectedLanguage, (language) => {
  const model = editor.value?.getModel();
  if (model) monaco.editor.setModelLanguage(model, language);
});

/** 根据扩展名识别 Monaco 语言 */
function detectLanguage(filePath: string): string {
  const ext = filePath.split(".").pop()?.toLowerCase() ?? "";
  const map: Record<string, string> = {
    json: "json",
    yml: "yaml",
    yaml: "yaml",
    java: "java",
    js: "javascript",
    jsx: "javascript",
    ts: "typescript",
    tsx: "typescript",
    css: "css",
    scss: "scss",
    less: "less",
    html: "html",
    htm: "html",
    xml: "xml",
    md: "markdown",
    sh: "shell",
    bash: "shell",
    zsh: "shell",
    py: "python",
    rs: "rust",
    sql: "sql",
  };
  return map[ext] ?? "plaintext";
}

/** 根据内容粗略判断二进制文件 */
function isLikelyBinary(bytes: number[]): boolean {
  if (bytes.length === 0) return false;
  const sample = bytes.slice(0, Math.min(bytes.length, 4096));
  if (sample.includes(0)) return true;
  const controlCount = sample.filter(
    (byte) => byte < 32 && ![9, 10, 13].includes(byte)
  ).length;
  return controlCount / sample.length > 0.08;
}

/** 初始化 Monaco 编辑器 */
function setupEditor() {
  if (!editorContainer.value) return;
  editor.value?.dispose();
  editor.value = monaco.editor.create(editorContainer.value, {
    value: savedContent.value,
    language: selectedLanguage.value,
    theme: "vs-dark",
    automaticLayout: true,
    minimap: { enabled: true },
    fontFamily: 'Consolas, "Cascadia Mono", monospace',
    fontSize: 13,
    tabSize: 2,
    scrollBeyondLastLine: false,
    wordWrap: "off",
    readOnly: readOnly.value,
  });
}

/** 从当前 SSH 会话读取远端文件并初始化编辑器 */
async function loadFile() {
  if (!sessionId || !path) {
    loading.value = false;
    loadError.value = "编辑窗口参数不完整";
    return;
  }
  loading.value = true;
  loadError.value = "";
  try {
    const bytes = await sftpRead(sessionId, path);
    if (isLikelyBinary(bytes)) {
      const confirmed = await showConfirm(
        "编辑确认",
        "文件可能不是文本文件，是否继续打开编辑？"
      );
      if (!confirmed) {
        await destroyWindow();
        return;
      }
    }
    let writable = true;
    try {
      writable = await sftpCheckWritable(sessionId, path);
    } catch {
      // 检测失败按可写处理，避免误锁文件
      writable = true;
    }
    readOnly.value = !writable;
    savedContent.value = new TextDecoder().decode(new Uint8Array(bytes));
    selectedLanguage.value = detectedLanguage.value;
    loading.value = false;
    await nextTick();
    setupEditor();
  } catch (error) {
    loading.value = false;
    loadError.value = String(error);
  }
}

/** 显示确认弹窗 */
function showConfirm(title: string, message: string): Promise<boolean> {
  return new Promise((resolve) => {
    Object.assign(confirmDialog, { open: true, title, message, resolve });
  });
}

/** 确认二次弹窗 */
function confirmAction(value: boolean) {
  const resolve = confirmDialog.resolve;
  confirmDialog.open = false;
  confirmDialog.resolve = undefined;
  resolve?.(value);
}

/** 判断编辑内容是否有未保存修改 */
function hasChanged(): boolean {
  return (editor.value?.getValue() ?? savedContent.value) !== savedContent.value;
}

/** 主动销毁当前编辑窗口 */
async function destroyWindow() {
  destroying = true;
  await appWindow.destroy();
}

/** 请求关闭编辑器 */
async function requestClose() {
  if (saving.value || confirmDialog.open) return;
  if (!hasChanged()) {
    await destroyWindow();
    return;
  }
  if (
    await showConfirm(
      "关闭确认",
      "文件内容已修改，是否关闭文本编辑器？未保存的修改将丢失。"
    )
  ) {
    await destroyWindow();
  }
}

/** 保存当前编辑器内容 */
async function save() {
  if (readOnly.value || saving.value) return;
  saving.value = true;
  const value = editor.value?.getValue() ?? savedContent.value;
  try {
    await sftpWrite(sessionId, path, Array.from(new TextEncoder().encode(value)));
    savedContent.value = value;
    try {
      await emit("editor://saved", { sessionId, path });
    } catch (error) {
      console.warn("通知主窗口刷新文件列表失败", error);
    }
    Object.assign(resultDialog, {
      open: true,
      title: "保存成功",
      message: "文件内容已保存",
      exit: true,
    });
  } catch (error) {
    Object.assign(resultDialog, {
      open: true,
      title: "保存失败",
      message: String(error),
      exit: false,
    });
  } finally {
    saving.value = false;
  }
}

/** 结果弹窗主按钮：保存成功时退出编辑器，失败时仅关闭弹窗 */
async function confirmResult() {
  resultDialog.open = false;
  if (resultDialog.exit) await destroyWindow();
}

/** 结果弹窗右上角叉号：仅关闭弹窗，不退出编辑器 */
function dismissResult() {
  resultDialog.open = false;
}

/** 阻止编辑窗口中的浏览器默认快捷键，并提供保存快捷键 */
function preventBrowserShortcut(event: KeyboardEvent) {
  const key = event.key.toLowerCase();
  const ctrlOrMeta = event.ctrlKey || event.metaKey;
  if (ctrlOrMeta && key === "s") {
    event.preventDefault();
    event.stopPropagation();
    save();
    return;
  }
  if (event.key === "F5" || (ctrlOrMeta && ["p", "r", "u"].includes(key))) {
    event.preventDefault();
    event.stopPropagation();
  }
}

onMounted(async () => {
  window.addEventListener("keydown", preventBrowserShortcut, true);
  try {
    unlistenCloseRequested = await appWindow.onCloseRequested((event) => {
      if (destroying) return;
      event.preventDefault();
      requestClose();
    });
  } catch (error) {
    console.warn("监听文本编辑窗口关闭事件失败", error);
  }
  await loadFile();
});

onBeforeUnmount(() => {
  editor.value?.dispose();
  editor.value = undefined;
  unlistenCloseRequested?.();
  window.removeEventListener("keydown", preventBrowserShortcut, true);
});
</script>

<template>
  <div class="editor-window-root">
    <TitleBar :title="editorWindowTitle" :show-settings="false" />
    <div v-if="loading" class="editor-state">
      <span class="editor-spinner"></span>
      <span>正在打开文件，请稍候…</span>
    </div>
    <div v-else-if="loadError" class="editor-state editor-error-state">
      <div class="editor-error-title">打开失败</div>
      <div class="editor-error-message">{{ loadError }}</div>
      <div class="editor-error-actions">
        <button class="btn" @click="destroyWindow">关闭</button>
        <button class="btn btn-primary" @click="loadFile">重新加载</button>
      </div>
    </div>
    <template v-else>
      <div ref="editorContainer" class="editor-body"></div>
      <div class="editor-footer">
        <label class="editor-lang">
          语言
          <select v-model="selectedLanguage" class="editor-lang-select">
            <option v-for="option in languageOptions" :key="option.value" :value="option.value">
              {{ option.label }}
            </option>
          </select>
          <span>自动：{{ detectedLanguage }}</span>
        </label>
        <button class="btn" @click="requestClose">取消</button>
        <button
          class="btn btn-primary"
          :disabled="readOnly"
          :title="readOnly ? '只读文件，无写入权限' : ''"
          @click="save"
        >
          保存
        </button>
      </div>
    </template>

    <div v-if="confirmDialog.open" class="modal-mask editor-confirm-mask">
      <div class="modal editor-confirm" role="dialog" aria-modal="true">
        <div class="modal-header">{{ confirmDialog.title }}</div>
        <div class="modal-body editor-confirm-body">{{ confirmDialog.message }}</div>
        <div class="modal-footer">
          <button class="btn" @click="confirmAction(false)">取消</button>
          <button class="btn btn-primary" @click="confirmAction(true)">确定</button>
        </div>
      </div>
    </div>
    <div v-if="saving" class="modal-mask editor-confirm-mask">
      <div class="modal editor-confirm" role="dialog" aria-modal="true">
        <div class="modal-header">保存中</div>
        <div class="modal-body editor-confirm-body editor-saving">
          <span class="editor-spinner"></span>
          <span>正在保存，请稍候…</span>
        </div>
      </div>
    </div>
    <div v-if="resultDialog.open" class="modal-mask editor-confirm-mask">
      <div class="modal editor-confirm" role="dialog" aria-modal="true">
        <div class="modal-header">
          <span>{{ resultDialog.title }}</span>
          <button class="modal-close" title="关闭" @click="dismissResult">×</button>
        </div>
        <div class="modal-body editor-confirm-body">{{ resultDialog.message }}</div>
        <div class="modal-footer">
          <button class="btn btn-primary" @click="confirmResult">
            {{ resultDialog.exit ? "退出编辑器" : "确定" }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.editor-window-root {
  width: 100%;
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: var(--bg-window);
}
.editor-body {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}
.editor-footer {
  height: 42px;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 10px;
  border-top: 1px solid var(--border);
  background: var(--bg-panel);
  flex: 0 0 auto;
}
.editor-lang {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-right: auto;
  color: var(--text-muted);
  font-size: 12px;
}
.editor-lang-select {
  height: 24px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: #fff;
  color: var(--text);
  font-size: 12px;
}
.editor-state {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: var(--text-secondary);
}
.editor-error-state {
  flex-direction: column;
  padding: 32px;
}
.editor-error-title {
  color: var(--danger);
  font-size: 16px;
  font-weight: 700;
}
.editor-error-message {
  max-width: 720px;
  line-height: 1.6;
  word-break: break-word;
}
.editor-error-actions {
  display: flex;
  gap: 8px;
  margin-top: 8px;
}
.editor-confirm-mask {
  z-index: 1001;
}
.editor-confirm {
  width: 360px;
}
.editor-confirm-body {
  line-height: 1.6;
  color: var(--text-secondary);
}
.editor-saving {
  display: flex;
  align-items: center;
  gap: 10px;
}
.editor-spinner {
  width: 16px;
  height: 16px;
  flex: 0 0 auto;
  border: 2px solid #c9d6e4;
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: editor-spin 0.8s linear infinite;
}
@keyframes editor-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
