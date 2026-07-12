<script setup lang="ts">
/**
 * 远程文本文件编辑器弹窗：集成 Monaco Editor 提供 VS Code 风格编辑体验
 */
import { computed, nextTick, onBeforeUnmount, reactive, ref, shallowRef, watch } from "vue";
import * as monaco from "monaco-editor";
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import jsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker";
import cssWorker from "monaco-editor/esm/vs/language/css/css.worker?worker";
import htmlWorker from "monaco-editor/esm/vs/language/html/html.worker?worker";
import tsWorker from "monaco-editor/esm/vs/language/typescript/ts.worker?worker";

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

const props = defineProps<{
  /** 是否显示编辑器 */
  open: boolean;
  /** 远程文件路径 */
  path: string;
  /** 文件文本内容 */
  content: string;
}>();

const emit = defineEmits<{
  (e: "save", value: string, done: () => void): void;
  (e: "close"): void;
}>();

/** 编辑器挂载容器 */
const editorContainer = ref<HTMLDivElement | null>(null);
const editor = shallowRef<monaco.editor.IStandaloneCodeEditor>();
const selectedLanguage = ref("plaintext");
const confirmDialog = reactive({ open: false, title: "", message: "", resolve: undefined as ((value: boolean) => void) | undefined });
const successDialog = reactive({ open: false, title: "", message: "" });

const detectedLanguage = computed(() => detectLanguage(props.path));
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

watch(
  () => props.open,
  async (open) => {
    if (!open) {
      disposeEditor();
      return;
    }
    selectedLanguage.value = detectedLanguage.value;
    await nextTick();
    setupEditor();
  }
);

watch(
  () => [props.content, selectedLanguage.value] as const,
  ([content, lang]) => {
    if (!editor.value) return;
    const model = editor.value.getModel();
    if (model) monaco.editor.setModelLanguage(model, lang);
    if (editor.value.getValue() !== content) editor.value.setValue(content);
  }
);

watch(selectedLanguage, (lang) => {
  const model = editor.value?.getModel();
  if (model) monaco.editor.setModelLanguage(model, lang);
});

/** 根据扩展名识别 Monaco 语言 */
function detectLanguage(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
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

/** 初始化 Monaco 编辑器 */
function setupEditor() {
  if (!editorContainer.value) return;
  if (!editor.value) {
    editor.value = monaco.editor.create(editorContainer.value, {
      value: props.content,
      language: selectedLanguage.value,
      theme: "vs-dark",
      automaticLayout: true,
      minimap: { enabled: true },
      fontFamily: 'Consolas, "Cascadia Mono", monospace',
      fontSize: 13,
      tabSize: 2,
      scrollBeyondLastLine: false,
      wordWrap: "off",
    });
    return;
  }
  const model = editor.value.getModel();
  if (model) monaco.editor.setModelLanguage(model, selectedLanguage.value);
  editor.value.setValue(props.content);
  editor.value.layout();
}

/** 关闭弹窗时销毁编辑器，避免复用已脱离 DOM 的实例 */
function disposeEditor() {
  editor.value?.dispose();
  editor.value = undefined;
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
  resolve?.(value);
}

/** 判断编辑内容是否有未保存修改 */
function hasChanged(): boolean {
  return (editor.value?.getValue() ?? props.content) !== props.content;
}

/** 请求关闭编辑器 */
async function requestClose() {
  if (!hasChanged()) {
    emit("close");
    return;
  }
  if (await showConfirm("关闭确认", "文件内容已修改，是否关闭文本编辑器？未保存的修改将丢失。")) emit("close");
}

/** 保存当前编辑器内容 */
async function save() {
  if (!(await showConfirm("保存确认", "是否保存当前文件内容？"))) return;
  emit("save", editor.value?.getValue() ?? props.content, () => {
    Object.assign(successDialog, { open: true, title: "保存成功", message: "文件内容已保存" });
  });
}

/** 确认保存成功提示并关闭编辑器 */
function confirmSaveSuccess() {
  successDialog.open = false;
  emit("close");
}

onBeforeUnmount(() => {
  disposeEditor();
});
</script>

<template>
  <div v-if="open" class="modal-mask editor-mask">
    <div class="modal editor-modal" role="dialog" aria-modal="true">
      <div class="modal-header">
        <span>编辑文本：{{ path }}</span>
        <button class="editor-close" title="关闭" @click="requestClose">×</button>
      </div>
      <div ref="editorContainer" class="editor-body"></div>
      <div class="modal-footer">
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
        <button class="btn btn-primary" @click="save">保存</button>
      </div>
    </div>
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
    <div v-if="successDialog.open" class="modal-mask editor-confirm-mask">
      <div class="modal editor-confirm" role="dialog" aria-modal="true">
        <div class="modal-header">{{ successDialog.title }}</div>
        <div class="modal-body editor-confirm-body">{{ successDialog.message }}</div>
        <div class="modal-footer">
          <button class="btn btn-primary" @click="confirmSaveSuccess">确定</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.editor-modal {
  width: min(1080px, 94vw);
  height: min(760px, 88vh);
}
.editor-body {
  flex: 1;
  min-height: 0;
  overflow: hidden;
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
.editor-close {
  width: 24px;
  height: 24px;
  border: none;
  border-radius: 3px;
  background: transparent;
  color: #667;
  font-size: 20px;
  line-height: 20px;
  cursor: pointer;
}
.editor-close:hover {
  background: var(--row-hover);
  color: var(--danger);
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
</style>
