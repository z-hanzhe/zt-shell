<script setup lang="ts">
/**
 * 独立远程文本编辑工作区：以单个 Monaco 实例承载多个远端文件标签
 */
import {
  computed,
  markRaw,
  nextTick,
  onBeforeUnmount,
  onMounted,
  reactive,
  ref,
  shallowRef,
} from "vue";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";
import "monaco-editor/esm/nls.messages.zh-cn.js";
import * as monaco from "monaco-editor";
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import jsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker";
import cssWorker from "monaco-editor/esm/vs/language/css/css.worker?worker";
import htmlWorker from "monaco-editor/esm/vs/language/html/html.worker?worker";
import tsWorker from "monaco-editor/esm/vs/language/typescript/ts.worker?worker";
import { sftpCancelOperation, sftpCheckWritable, sftpRead, sftpWrite } from "../api";
import { hasOpenModal } from "../composables/useEscClose";
import type {
  EditorCloseSessionRequestPayload,
  EditorClosePreparedPayload,
  EditorOpenRequestPayload,
  EditorPrepareCloseRequestPayload,
  EditorReleaseClosePreparationPayload,
  EditorRequestCompletedPayload,
  TextEditorWindowOptions,
} from "../editorProtocol";
import {
  EDITOR_CANCEL_CLOSE_SESSION_EVENT,
  EDITOR_CLOSE_SESSION_EVENT,
  EDITOR_CLOSE_PREPARED_EVENT,
  EDITOR_COMMIT_CLOSE_SESSION_EVENT,
  EDITOR_OPENED_EVENT,
  EDITOR_OPEN_EVENT,
  EDITOR_PREPARE_CLOSE_EVENT,
  EDITOR_READY_EVENT,
  EDITOR_RELEASE_CLOSE_PREPARATION_EVENT,
  EDITOR_SAVED_EVENT,
  EDITOR_SESSION_CLOSE_READY_EVENT,
  EDITOR_SESSION_CLOSED_EVENT,
} from "../editorProtocol";
import { genId } from "../utils";
import AppDialog from "./AppDialog.vue";
import Icon from "./Icon.vue";
import TitleBar from "./TitleBar.vue";

type SaveFeedback = "" | "success" | "error";
type TabMenuAction =
  | "close"
  | "closeOthers"
  | "closeRight"
  | "closeSaved"
  | "closeAll";
type EditorConfirmScope = "business" | "window";

type EditorConfirmRequest = {
  /** 确认请求标识，用于切换请求时重新初始化弹窗 */
  id: number;
  /** 弹窗层级，窗口级确认优先于业务确认 */
  scope: EditorConfirmScope;
  title: string;
  message: string;
  documentKey: string;
  settled: boolean;
  resolve?: (value: boolean) => void;
};

declare global {
  interface Window {
    MonacoEnvironment?: monaco.Environment;
  }
}

interface EditorDocument extends TextEditorWindowOptions {
  /** 会话与路径组成的文档标识 */
  key: string;
  /** 标签页显示文件名 */
  fileName: string;
  /** 自动检测的语言 */
  detectedLanguage: string;
  /** 当前选择的语言 */
  language: string;
  /** 是否只读 */
  readOnly: boolean;
  /** 是否正在读取 */
  loading: boolean;
  /** 读取错误 */
  loadError: string;
  /** 是否正在保存 */
  saving: boolean;
  /** 是否存在未保存修改 */
  dirty: boolean;
  /** 最近一次成功读取或保存时的 Monaco 内容版本 */
  savedVersionId: number;
  /** Monaco 文本模型 */
  model?: monaco.editor.ITextModel;
  /** Monaco 内容变化监听 */
  changeListener?: monaco.IDisposable;
  /** 标签切换时保存的编辑器视图状态 */
  viewState?: monaco.editor.ICodeEditorViewState | null;
  /** 当前保存任务 */
  saveTask?: Promise<void>;
  /** 最近一次保存结果 */
  saveFeedback: SaveFeedback;
  /** 最近一次保存结果文案 */
  saveFeedbackMessage: string;
  /** 当前保存操作标识 */
  saveOperationId: string;
  /** 是否允许取消当前保存 */
  saveCanCancel: boolean;
  /** 是否正在发送取消请求 */
  cancellingSave: boolean;
  /** 延迟显示取消按钮的计时器 */
  saveCancelTimer?: number;
  /** 文档是否已从工作区移除 */
  closed: boolean;
  /** 是否已确认打开大文件 */
  largeFileConfirmed: boolean;
  /** 是否已确认打开疑似二进制文件 */
  binaryFileConfirmed: boolean;
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

const FILE_SIZE_CONFIRM_THRESHOLD = 1024 * 1024;
const search = new URLSearchParams(window.location.search);
const startupRequestId = search.get("startupRequestId") ?? "";
const appWindow = getCurrentWindow();

/** 编辑器挂载容器 */
const editorContainer = ref<HTMLDivElement | null>(null);
/** 选项卡滚动容器 */
const tabsContainer = ref<HTMLDivElement | null>(null);
/** 选项卡右键菜单 */
const tabMenuElement = ref<HTMLDivElement | null>(null);
/** 全工作区唯一的 Monaco 编辑器 */
const editor = shallowRef<monaco.editor.IStandaloneCodeEditor>();
const documents = ref<EditorDocument[]>([]);
const activeKey = ref("");
const startupError = ref("");
const canScrollLeft = ref(false);
const canScrollRight = ref(false);
/** 等待处理的业务确认，窗口级确认出现时保留在下层 */
const businessConfirmQueue = ref<EditorConfirmRequest[]>([]);
/** 当前窗口关闭流程的确认请求 */
const windowConfirm = ref<EditorConfirmRequest>();
const tabMenu = reactive({
  open: false,
  x: 0,
  y: 0,
  documentKey: "",
});
/** 主窗口关闭流程持有的会话编辑锁，按准备请求区分 */
const closePreparations = reactive(new Map<string, Set<string>>());
/** 已确认提交、正在移除文档的会话 */
const committingSessionIds = reactive(new Set<string>());
/** 已接收并等待主窗口提交的会话关闭请求 */
const pendingSessionCloseRequests = new Map<string, string>();

/** 是否由组件主动销毁窗口，避免再次触发关闭确认 */
let destroying = false;
/** 是否正在执行整窗关闭确认，避免两层弹窗之间重复进入 */
const closingWorkspace = ref(false);
/** 确认请求递增序号 */
let confirmRequestSequence = 0;
let documentLoadQueue = Promise.resolve();
let unlistenCloseRequested: UnlistenFn | undefined;
let unlistenOpenDocument: UnlistenFn | undefined;
let unlistenCloseSession: UnlistenFn | undefined;
let unlistenCommitCloseSession: UnlistenFn | undefined;
let unlistenCancelCloseSession: UnlistenFn | undefined;
let unlistenPrepareClose: UnlistenFn | undefined;
let unlistenReleaseClosePreparation: UnlistenFn | undefined;
let sessionCloseQueue = Promise.resolve();
let tabsResizeObserver: ResizeObserver | undefined;

/** 当前队首业务确认，窗口级确认仅在视觉层级上覆盖它 */
const activeBusinessConfirm = computed(() => businessConfirmQueue.value[0]);

const activeDocument = computed(() =>
  documents.value.find((document) => document.key === activeKey.value)
);
const hasSavingDocument = computed(() =>
  documents.value.some((document) => document.saving)
);
const tabMenuDocumentIndex = computed(() =>
  documents.value.findIndex((document) => document.key === tabMenu.documentKey)
);
const canCloseOtherDocuments = computed(
  () => tabMenuDocumentIndex.value >= 0 && documents.value.length > 1
);
const canCloseRightDocuments = computed(
  () =>
    tabMenuDocumentIndex.value >= 0 &&
    tabMenuDocumentIndex.value < documents.value.length - 1
);
const canCloseSavedDocuments = computed(() =>
  documents.value.some((document) => !document.dirty && !document.saving)
);
const editorWindowTitle = computed(() => {
  const document = activeDocument.value;
  if (!document) return "文本编辑器";
  return `${document.readOnly ? "[只读] " : ""}${document.sessionName}：${document.path}`;
});
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

/** 根据会话和路径生成稳定的文档标识 */
function documentKey(sessionId: string, path: string): string {
  return `${sessionId}\n${path}`;
}

/** 提取远端路径中的文件名 */
function fileName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() || path;
}

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

/** 从窗口查询参数读取首个文档 */
function initialDocumentOptions(): TextEditorWindowOptions | undefined {
  const sessionId = search.get("sessionId") ?? "";
  const sessionName = search.get("sessionName") || sessionId;
  const path = search.get("path") ?? "";
  if (!sessionId || !path) return undefined;
  const parsedSize = Number(search.get("size") ?? "0");
  return {
    sessionId,
    sessionName,
    path,
    size: Number.isFinite(parsedSize) ? Math.max(0, parsedSize) : 0,
  };
}

/** 初始化全工作区唯一的 Monaco 编辑器 */
function setupEditor(): void {
  if (!editorContainer.value) return;
  editor.value?.dispose();
  editor.value = monaco.editor.create(editorContainer.value, {
    model: null,
    theme: "vs-dark",
    automaticLayout: true,
    minimap: { enabled: true },
    fontFamily: 'Consolas, "Cascadia Mono", monospace',
    fontSize: 13,
    tabSize: 2,
    scrollBeyondLastLine: false,
    wordWrap: "off",
    readOnly: true,
  });
}

/** 保存当前标签的光标和滚动位置 */
function saveActiveViewState(): void {
  const document = activeDocument.value;
  if (!document?.model || editor.value?.getModel() !== document.model) return;
  const viewState = editor.value.saveViewState();
  document.viewState = viewState ? markRaw(viewState) : null;
}

/** 将当前标签的模型和只读状态应用到 Monaco */
function applyActiveDocument(): void {
  const document = activeDocument.value;
  editor.value?.setModel(document?.model ?? null);
  syncEditorReadOnly();
  if (document?.viewState) editor.value?.restoreViewState(document.viewState);
  editor.value?.focus();
}

/** 判断指定会话的编辑文档是否已被主窗口关闭流程锁定 */
function isSessionCloseLocked(sessionId: string): boolean {
  if (committingSessionIds.has(sessionId)) return true;
  for (const sessionIds of closePreparations.values()) {
    if (sessionIds.has(sessionId)) return true;
  }
  return false;
}

/** 根据文档权限和全局弹窗状态同步 Monaco 只读设置 */
function syncEditorReadOnly(): void {
  const document = activeDocument.value;
  const blocked =
    !!document?.saving ||
    businessConfirmQueue.value.length > 0 ||
    Boolean(windowConfirm.value) ||
    (!!document && isSessionCloseLocked(document.sessionId));
  editor.value?.updateOptions({ readOnly: !document || document.readOnly || blocked });
}

/** 串行执行会话文档移除，避免批量关闭时多个事件并发修改文档数组 */
function enqueueSessionClose(operation: () => Promise<void>): Promise<void> {
  const pending = sessionCloseQueue.then(operation, operation);
  sessionCloseQueue = pending.then(
    () => undefined,
    () => undefined
  );
  return pending;
}

/** 释放指定主窗口关闭准备并刷新当前文档只读状态 */
function releaseClosePreparation(requestId: string): void {
  closePreparations.delete(requestId);
  syncEditorReadOnly();
}

/** 从全部关闭准备中移除已提交关闭的会话 */
function removeSessionFromClosePreparations(sessionId: string): void {
  for (const [requestId, sessionIds] of closePreparations) {
    sessionIds.delete(sessionId);
    if (sessionIds.size === 0) closePreparations.delete(requestId);
  }
}

/** 更新选项卡栏左右滚动按钮状态 */
function updateTabScrollState(): void {
  const container = tabsContainer.value;
  if (!container) {
    canScrollLeft.value = false;
    canScrollRight.value = false;
    return;
  }
  canScrollLeft.value = container.scrollLeft > 1;
  canScrollRight.value =
    container.scrollLeft + container.clientWidth < container.scrollWidth - 1;
}

/** 将选项卡栏向指定方向滚动 */
function scrollTabs(direction: -1 | 1): void {
  const container = tabsContainer.value;
  if (!container) return;
  container.scrollBy({ left: direction * container.clientWidth * 0.5, behavior: "smooth" });
}

/** 激活指定文档并恢复其编辑位置 */
function activateDocument(key: string): void {
  if (activeKey.value === key) {
    applyActiveDocument();
    return;
  }
  saveActiveViewState();
  activeKey.value = key;
  applyActiveDocument();
  void nextTick(() => {
    const index = documents.value.findIndex((document) => document.key === key);
    const tab = tabsContainer.value?.children[index] as HTMLElement | undefined;
    tab?.scrollIntoView({ block: "nearest", inline: "nearest" });
    updateTabScrollState();
  });
}

/** 清除指定文档的保存结果反馈 */
function clearSaveFeedback(document: EditorDocument): void {
  document.saveFeedback = "";
  document.saveFeedbackMessage = "";
}

/** 记录指定文档的保存结果 */
function showSaveFeedback(
  document: EditorDocument,
  status: Exclude<SaveFeedback, "">,
  message: string
): void {
  clearSaveFeedback(document);
  document.saveFeedback = status;
  document.saveFeedbackMessage = message;
}

/** 清理指定文档的保存操作控制状态 */
function clearSaveOperationState(document: EditorDocument, operationId = ""): void {
  if (operationId && document.saveOperationId !== operationId) return;
  if (document.saveCancelTimer !== undefined) {
    window.clearTimeout(document.saveCancelTimer);
    document.saveCancelTimer = undefined;
  }
  document.saveOperationId = "";
  document.saveCanCancel = false;
  document.cancellingSave = false;
}

/** 释放指定文档持有的 Monaco 资源 */
function disposeDocumentModel(document: EditorDocument): void {
  clearSaveFeedback(document);
  clearSaveOperationState(document);
  document.changeListener?.dispose();
  document.changeListener = undefined;
  document.model?.dispose();
  document.model = undefined;
  document.viewState = undefined;
}

/** 从工作区移除文档，必要时销毁空工作区窗口 */
async function removeDocument(key: string, destroyWhenEmpty = true): Promise<void> {
  const index = documents.value.findIndex((document) => document.key === key);
  if (index < 0) return;
  const document = documents.value[index];
  const wasActive = activeKey.value === key;
  if (wasActive) editor.value?.setModel(null);
  const relatedConfirm = businessConfirmQueue.value.find(
    (request) => request.documentKey === key
  );
  if (relatedConfirm) settleConfirm(relatedConfirm, false);
  document.closed = true;
  documents.value.splice(index, 1);
  disposeDocumentModel(document);
  if (tabMenu.documentKey === key) closeTabMenu();

  if (documents.value.length === 0) {
    activeKey.value = "";
    if (destroyWhenEmpty) await destroyWindow();
    return;
  }
  if (wasActive) {
    const next = documents.value[index] ?? documents.value[index - 1];
    activeKey.value = next.key;
    applyActiveDocument();
  }
  await nextTick();
  updateTabScrollState();
}

/** 创建确认请求并放入对应层级 */
function createConfirmRequest(
  scope: EditorConfirmScope,
  title: string,
  message: string,
  documentKey = ""
): Promise<boolean> {
  return new Promise((resolve) => {
    const request: EditorConfirmRequest = {
      id: ++confirmRequestSequence,
      scope,
      title,
      message,
      documentKey,
      settled: false,
      resolve,
    };
    if (scope === "window") windowConfirm.value = request;
    else businessConfirmQueue.value.push(request);
    syncEditorReadOnly();
  });
}

/** 显示业务确认弹窗，并按需关联对应文档 */
function showConfirm(title: string, message: string, documentKey = ""): Promise<boolean> {
  return createConfirmRequest("business", title, message, documentKey);
}

/** 显示覆盖整个编辑窗口的窗口级确认 */
function showWindowConfirm(title: string, message: string): Promise<boolean> {
  return createConfirmRequest("window", title, message);
}

/** 幂等结算指定确认请求 */
function settleConfirm(request: EditorConfirmRequest, value: boolean): void {
  if (request.settled) return;
  request.settled = true;
  if (request.scope === "window") {
    if (windowConfirm.value === request) windowConfirm.value = undefined;
  } else {
    const index = businessConfirmQueue.value.indexOf(request);
    if (index >= 0) businessConfirmQueue.value.splice(index, 1);
  }
  const resolve = request.resolve;
  request.resolve = undefined;
  syncEditorReadOnly();
  resolve?.(value);
}

/** 结算当前队首业务确认 */
function settleActiveBusinessConfirm(value: boolean): void {
  const request = activeBusinessConfirm.value;
  if (request) settleConfirm(request, value);
}

/** 结算当前窗口级确认 */
function settleWindowConfirm(value: boolean): void {
  const request = windowConfirm.value;
  if (request) settleConfirm(request, value);
}

/** 取消全部尚未结算的确认请求 */
function cancelAllConfirms(): void {
  const requests = [windowConfirm.value, ...businessConfirmQueue.value].filter(
    (request): request is EditorConfirmRequest => Boolean(request)
  );
  requests.forEach((request) => settleConfirm(request, false));
}

/** 请求批量关闭文档，未保存文档只统一确认一次 */
async function requestCloseDocuments(targets: EditorDocument[]): Promise<void> {
  if (
    targets.some((document) => document.saving) ||
    targets.some((document) => isSessionCloseLocked(document.sessionId)) ||
    businessConfirmQueue.value.length > 0 ||
    Boolean(windowConfirm.value) ||
    closingWorkspace.value
  ) {
    return;
  }
  const keys = new Set(
    targets
      .filter((document) => !document.closed)
      .map((document) => document.key)
  );
  if (keys.size === 0) return;

  const dirtyCount = documents.value.filter(
    (document) => keys.has(document.key) && document.dirty
  ).length;
  if (dirtyCount > 0) {
    const message =
      dirtyCount === 1
        ? "文件内容已修改，是否关闭？未保存的修改将丢失。"
        : `有 ${dirtyCount} 个文件尚未保存，是否关闭？未保存的修改将丢失。`;
    if (!(await showConfirm("关闭确认", message))) return;
  }

  for (const key of keys) {
    await removeDocument(key, false);
  }
  if (documents.value.length === 0) await destroyWindow();
}

/** 请求关闭单个文档标签 */
async function requestCloseDocument(key: string): Promise<void> {
  const document = documents.value.find((item) => item.key === key);
  if (!document) return;
  await requestCloseDocuments([document]);
}

/** 打开指定选项卡的右键菜单并校正窗口边缘位置 */
function openTabMenu(key: string, event: MouseEvent): void {
  event.preventDefault();
  event.stopPropagation();
  activateDocument(key);
  Object.assign(tabMenu, {
    open: true,
    x: event.clientX,
    y: event.clientY,
    documentKey: key,
  });
  void nextTick(() => {
    const element = tabMenuElement.value;
    if (!element || !tabMenu.open) return;
    const rect = element.getBoundingClientRect();
    tabMenu.x = Math.max(4, Math.min(tabMenu.x, window.innerWidth - rect.width - 4));
    tabMenu.y = Math.max(4, Math.min(tabMenu.y, window.innerHeight - rect.height - 4));
  });
}

/** 关闭选项卡右键菜单 */
function closeTabMenu(): void {
  tabMenu.open = false;
  tabMenu.documentKey = "";
}

/** 执行选项卡右键菜单关闭动作 */
async function handleTabMenuAction(action: TabMenuAction): Promise<void> {
  const index = tabMenuDocumentIndex.value;
  const target = index >= 0 ? documents.value[index] : undefined;
  let targets: EditorDocument[] = [];
  switch (action) {
    case "close":
      if (target) targets = [target];
      break;
    case "closeOthers":
      if (target) targets = documents.value.filter((document) => document !== target);
      break;
    case "closeRight":
      if (target) targets = documents.value.slice(index + 1);
      break;
    case "closeSaved":
      targets = documents.value.filter((document) => !document.dirty && !document.saving);
      break;
    case "closeAll":
      targets = [...documents.value];
      break;
  }
  closeTabMenu();
  await requestCloseDocuments(targets);
}

/** 点击菜单外部时关闭选项卡右键菜单 */
function handleGlobalPointerDown(event: PointerEvent): void {
  if (!tabMenu.open) return;
  const target = event.target as HTMLElement;
  if (target.closest(".editor-tab-context-menu")) return;
  closeTabMenu();
}

/** 禁用 WebView 原生右键菜单，同时保留组件自己的右键交互 */
function preventNativeContextMenu(event: MouseEvent): void {
  event.preventDefault();
}

/** 串行安排远端文件读取，避免多个确认弹窗相互覆盖 */
function enqueueDocumentLoad(document: EditorDocument): void {
  documentLoadQueue = documentLoadQueue.then(
    () => loadDocument(document),
    () => loadDocument(document)
  );
}

/** 从远端读取文档并创建 Monaco 文本模型 */
async function loadDocument(document: EditorDocument): Promise<void> {
  if (document.closed) return;
  document.loading = true;
  document.loadError = "";
  try {
    if (!document.largeFileConfirmed) {
      const confirmed = await showConfirm(
        "编辑确认",
        "文件大于 1MB，是否继续打开编辑？",
        document.key
      );
      if (!confirmed) {
        await removeDocument(document.key);
        return;
      }
      document.largeFileConfirmed = true;
    }

    const bytes = await sftpRead(document.sessionId, document.path);
    if (document.closed) return;
    if (!document.binaryFileConfirmed && isLikelyBinary(bytes)) {
      const confirmed = await showConfirm(
        "编辑确认",
        "文件可能不是文本文件，是否继续打开编辑？",
        document.key
      );
      if (!confirmed) {
        await removeDocument(document.key);
        return;
      }
      document.binaryFileConfirmed = true;
    }

    let writable = true;
    try {
      writable = await sftpCheckWritable(document.sessionId, document.path);
    } catch {
      // 检测失败按可写处理，避免误锁文件
      writable = true;
    }
    if (document.closed) return;

    document.readOnly = !writable;
    const content = new TextDecoder().decode(new Uint8Array(bytes));
    const model = markRaw(
      monaco.editor.createModel(content, document.language)
    );
    document.model = model;
    document.savedVersionId = model.getAlternativeVersionId();
    document.changeListener = markRaw(
      model.onDidChangeContent(() => {
        const dirty = model.getAlternativeVersionId() !== document.savedVersionId;
        if (document.saveFeedback) clearSaveFeedback(document);
        document.dirty = dirty;
      })
    );
    document.loading = false;
    if (activeKey.value === document.key) applyActiveDocument();
  } catch (error) {
    if (document.closed) return;
    document.loading = false;
    document.loadError = String(error);
  }
}

/** 重新读取当前加载失败的文档 */
function retryLoad(document: EditorDocument): void {
  if (document.loading || document.closed) return;
  enqueueDocumentLoad(document);
}

/** 将文档加入工作区，重复打开时只激活已有标签 */
function openDocument(options: TextEditorWindowOptions): void {
  const key = documentKey(options.sessionId, options.path);
  const existing = documents.value.find((document) => document.key === key);
  if (existing) {
    activateDocument(key);
    return;
  }

  const detectedLanguage = detectLanguage(options.path);
  documents.value.push({
    ...options,
    key,
    fileName: fileName(options.path),
    detectedLanguage,
    language: detectedLanguage,
    readOnly: false,
    loading: true,
    loadError: "",
    saving: false,
    dirty: false,
    savedVersionId: 0,
    saveFeedback: "",
    saveFeedbackMessage: "",
    saveOperationId: "",
    saveCanCancel: false,
    cancellingSave: false,
    closed: false,
    largeFileConfirmed: options.size <= FILE_SIZE_CONFIRM_THRESHOLD,
    binaryFileConfirmed: false,
  });
  const document = documents.value[documents.value.length - 1];
  activateDocument(key);
  enqueueDocumentLoad(document);
  void nextTick(updateTabScrollState);
}

/** 判断保存错误是否来自用户主动取消 */
function isSaveCancelled(error: unknown): boolean {
  return String(error).includes("文件操作已中断");
}

/** 执行单个文档的远端保存并更新内容基线 */
async function performSave(document: EditorDocument, operationId: string): Promise<void> {
  const value = document.model?.getValue() ?? "";
  const savedVersionId = document.model?.getAlternativeVersionId() ?? 0;
  try {
    await sftpWrite(
      document.sessionId,
      document.path,
      Array.from(new TextEncoder().encode(value)),
      operationId
    );
    if (document.closed) return;
    document.savedVersionId = savedVersionId;
    document.dirty =
      (document.model?.getAlternativeVersionId() ?? 0) !== savedVersionId;
    try {
      await emit(EDITOR_SAVED_EVENT, {
        sessionId: document.sessionId,
        path: document.path,
      });
    } catch (error) {
      console.warn("通知主窗口刷新文件列表失败", error);
    }
    showSaveFeedback(document, "success", "保存成功");
  } catch (error) {
    if (document.closed) return;
    showSaveFeedback(
      document,
      "error",
      isSaveCancelled(error) ? "保存已取消" : `保存失败：${String(error)}`
    );
  } finally {
    if (document.saveOperationId === operationId) {
      clearSaveOperationState(document, operationId);
      document.saving = false;
      document.saveTask = undefined;
    }
    syncEditorReadOnly();
  }
}

/** 保存指定文档，并复用尚未结束的保存任务 */
function saveDocument(document: EditorDocument): Promise<void> {
  if (
    document.readOnly ||
    document.loading ||
    document.loadError ||
    document.closed ||
    isSessionCloseLocked(document.sessionId)
  ) {
    return Promise.resolve();
  }
  if (document.saveTask) return document.saveTask;
  clearSaveFeedback(document);
  const operationId = genId();
  document.saving = true;
  document.saveOperationId = operationId;
  document.saveCanCancel = false;
  document.cancellingSave = false;
  document.saveCancelTimer = window.setTimeout(() => {
    if (
      document.saving &&
      !document.closed &&
      document.saveOperationId === operationId
    ) {
      document.saveCanCancel = true;
    }
    document.saveCancelTimer = undefined;
  }, 10_000);
  syncEditorReadOnly();
  const task = performSave(document, operationId);
  document.saveTask = markRaw(task);
  return task;
}

/** 保存当前激活文档 */
async function saveActiveDocument(): Promise<void> {
  const document = activeDocument.value;
  if (!document) return;
  await saveDocument(document);
}

/** 请求取消指定文档的长时间保存操作 */
async function cancelDocumentSave(document: EditorDocument): Promise<void> {
  const operationId = document.saveOperationId;
  if (
    !operationId ||
    !document.saving ||
    !document.saveCanCancel ||
    document.cancellingSave
  ) {
    return;
  }
  document.cancellingSave = true;
  try {
    const accepted = await sftpCancelOperation(document.sessionId, operationId);
    if (document.saveOperationId !== operationId) return;
    if (!accepted) {
      document.cancellingSave = false;
      document.saveCanCancel = false;
    }
  } catch (error) {
    if (document.saveOperationId !== operationId) return;
    document.cancellingSave = false;
    showSaveFeedback(document, "error", `取消保存失败：${String(error)}`);
  }
}

/** 切换当前文档的 Monaco 语言 */
function changeLanguage(event: Event): void {
  const document = activeDocument.value;
  if (!document) return;
  const language = (event.target as HTMLSelectElement).value;
  document.language = language;
  if (document.model) monaco.editor.setModelLanguage(document.model, language);
}

/** 释放工作区持有的全部 Monaco 资源 */
function disposeWorkspace(): void {
  editor.value?.setModel(null);
  documents.value.forEach((document) => {
    document.closed = true;
    disposeDocumentModel(document);
  });
  documents.value = [];
  editor.value?.dispose();
  editor.value = undefined;
}

/** 主动销毁当前编辑器工作区窗口 */
async function destroyWindow(): Promise<void> {
  if (destroying) return;
  destroying = true;
  cancelAllConfirms();
  disposeWorkspace();
  await appWindow.destroy();
}

/** 请求关闭整个编辑器工作区 */
async function requestCloseWorkspace(): Promise<void> {
  if (
    hasSavingDocument.value ||
    closePreparations.size > 0 ||
    committingSessionIds.size > 0 ||
    Boolean(windowConfirm.value) ||
    closingWorkspace.value
  ) {
    return;
  }
  closingWorkspace.value = true;
  closeTabMenu();
  try {
    if (
      documents.value.length > 1 &&
      !(await showWindowConfirm(
        "关闭窗口",
        `当前打开了 ${documents.value.length} 个文件，是否关闭编辑器窗口？`
      ))
    ) {
      return;
    }

    const dirtyCount = documents.value.filter((document) => document.dirty).length;
    if (dirtyCount > 0) {
      const message = `有 ${dirtyCount} 个文件尚未保存，是否关闭编辑器窗口？`;
      if (!(await showWindowConfirm("未保存确认", message))) return;
    }
    await destroyWindow();
  } finally {
    if (!destroying) closingWorkspace.value = false;
  }
}

/** 强制移除指定会话的全部文档 */
async function closeSessionDocuments(sessionId: string): Promise<boolean> {
  const targets = documents.value.filter((document) => document.sessionId === sessionId);
  for (const document of targets) {
    await removeDocument(document.key, false);
  }
  removeSessionFromClosePreparations(sessionId);
  return documents.value.length === 0;
}

/** 阻止编辑窗口中的浏览器默认快捷键，并提供保存快捷键 */
function preventBrowserShortcut(event: KeyboardEvent): void {
  if (event.key === "Escape" && tabMenu.open && !hasOpenModal()) {
    event.preventDefault();
    event.stopPropagation();
    closeTabMenu();
    return;
  }
  const key = event.key.toLowerCase();
  const ctrlOrMeta = event.ctrlKey || event.metaKey;
  if (ctrlOrMeta && key === "s") {
    event.preventDefault();
    event.stopPropagation();
    if (hasOpenModal()) return;
    void saveActiveDocument();
    return;
  }
  if (event.key === "F5" || (ctrlOrMeta && ["p", "r", "u"].includes(key))) {
    event.preventDefault();
    event.stopPropagation();
  }
}

onMounted(async () => {
  window.addEventListener("keydown", preventBrowserShortcut, true);
  window.addEventListener("contextmenu", preventNativeContextMenu);
  window.addEventListener("pointerdown", handleGlobalPointerDown);
  window.addEventListener("resize", closeTabMenu);
  window.addEventListener("blur", closeTabMenu);
  setupEditor();

  try {
    unlistenOpenDocument = await listen<EditorOpenRequestPayload>(
      EDITOR_OPEN_EVENT,
      async (event) => {
        if (!isSessionCloseLocked(event.payload.document.sessionId)) {
          openDocument(event.payload.document);
        }
        await emit(EDITOR_OPENED_EVENT, { requestId: event.payload.requestId });
      }
    );
    unlistenPrepareClose = await listen<EditorPrepareCloseRequestPayload>(
      EDITOR_PREPARE_CLOSE_EVENT,
      async (event) => {
        const sessionIds = new Set(event.payload.sessionIds.filter(Boolean));
        closePreparations.set(event.payload.requestId, sessionIds);
        syncEditorReadOnly();
        const response: EditorClosePreparedPayload = {
          requestId: event.payload.requestId,
          dirtyCount: documents.value.filter(
            (document) => sessionIds.has(document.sessionId) && document.dirty
          ).length,
        };
        await emit(EDITOR_CLOSE_PREPARED_EVENT, response);
      }
    );
    unlistenReleaseClosePreparation = await listen<EditorReleaseClosePreparationPayload>(
      EDITOR_RELEASE_CLOSE_PREPARATION_EVENT,
      (event) => releaseClosePreparation(event.payload.requestId)
    );
    unlistenCloseSession = await listen<EditorCloseSessionRequestPayload>(
      EDITOR_CLOSE_SESSION_EVENT,
      async (event) => {
        pendingSessionCloseRequests.set(
          event.payload.requestId,
          event.payload.sessionId
        );
        const response: EditorRequestCompletedPayload = {
          requestId: event.payload.requestId,
        };
        await emit(EDITOR_SESSION_CLOSE_READY_EVENT, response);
      }
    );
    unlistenCancelCloseSession = await listen<EditorCloseSessionRequestPayload>(
      EDITOR_CANCEL_CLOSE_SESSION_EVENT,
      (event) => pendingSessionCloseRequests.delete(event.payload.requestId)
    );
    unlistenCommitCloseSession = await listen<EditorCloseSessionRequestPayload>(
      EDITOR_COMMIT_CLOSE_SESSION_EVENT,
      (event) => {
        const { requestId, sessionId } = event.payload;
        if (pendingSessionCloseRequests.get(requestId) !== sessionId) return;
        pendingSessionCloseRequests.delete(requestId);
        committingSessionIds.add(sessionId);
        syncEditorReadOnly();
        void enqueueSessionClose(async () => {
          try {
            const shouldDestroy = await closeSessionDocuments(sessionId);
            const response: EditorRequestCompletedPayload = { requestId };
            await emit(EDITOR_SESSION_CLOSED_EVENT, response).catch((error) =>
              console.warn("通知主窗口文本编辑标签已关闭失败", error)
            );
            if (shouldDestroy) await destroyWindow();
          } finally {
            committingSessionIds.delete(sessionId);
            syncEditorReadOnly();
          }
        }).catch((error) => console.warn("关闭会话所属文本编辑标签失败", error));
      }
    );
  } catch (error) {
    console.warn("监听文本编辑工作区事件失败", error);
  }

  try {
    unlistenCloseRequested = await appWindow.onCloseRequested((event) => {
      if (destroying) return;
      event.preventDefault();
      void requestCloseWorkspace();
    });
  } catch (error) {
    console.warn("监听文本编辑窗口关闭事件失败", error);
  }

  const initialDocument = initialDocumentOptions();
  if (initialDocument) openDocument(initialDocument);
  else startupError.value = "编辑窗口参数不完整";

  if (startupRequestId) {
    try {
      await emit(EDITOR_READY_EVENT, { requestId: startupRequestId });
    } catch (error) {
      console.warn("通知主窗口文本编辑器已就绪失败", error);
    }
  }

  if (tabsContainer.value) {
    tabsResizeObserver = new ResizeObserver(updateTabScrollState);
    tabsResizeObserver.observe(tabsContainer.value);
  }
  await nextTick();
  updateTabScrollState();
});

onBeforeUnmount(() => {
  cancelAllConfirms();
  disposeWorkspace();
  tabsResizeObserver?.disconnect();
  unlistenOpenDocument?.();
  unlistenPrepareClose?.();
  unlistenReleaseClosePreparation?.();
  unlistenCloseSession?.();
  unlistenCommitCloseSession?.();
  unlistenCancelCloseSession?.();
  unlistenCloseRequested?.();
  window.removeEventListener("keydown", preventBrowserShortcut, true);
  window.removeEventListener("contextmenu", preventNativeContextMenu);
  window.removeEventListener("pointerdown", handleGlobalPointerDown);
  window.removeEventListener("resize", closeTabMenu);
  window.removeEventListener("blur", closeTabMenu);
});
</script>

<template>
  <div class="editor-window-root">
    <TitleBar :title="editorWindowTitle" :show-settings="false" />

    <div class="editor-tabbar">
      <button
        v-show="canScrollLeft"
        class="editor-tab-scroll"
        title="向左滚动"
        @click="scrollTabs(-1)"
      >
        <Icon name="chevronLeft" :size="14" />
      </button>
      <div ref="tabsContainer" class="editor-tabs" @scroll="updateTabScrollState">
        <div
          v-for="document in documents"
          :key="document.key"
          :class="['editor-tab', { active: activeKey === document.key }]"
          :title="`${document.sessionName}：${document.path}`"
          @click="activateDocument(document.key)"
          @contextmenu="openTabMenu(document.key, $event)"
        >
          <span v-if="document.loading" class="editor-tab-spinner"></span>
          <span
            v-else-if="document.loadError"
            class="editor-tab-error"
            title="打开失败"
          ></span>
          <span v-else-if="document.dirty" class="editor-tab-dirty" title="未保存"></span>
          <Icon v-else name="file" :size="13" />
          <span class="editor-tab-session">{{ document.sessionName }}</span>
          <span class="editor-tab-name">{{ document.fileName }}</span>
          <button
            class="editor-tab-close"
            title="关闭"
            @click.stop="requestCloseDocument(document.key)"
          >
            <Icon name="close" :size="11" />
          </button>
        </div>
      </div>
      <button
        v-show="canScrollRight"
        class="editor-tab-scroll"
        title="向右滚动"
        @click="scrollTabs(1)"
      >
        <Icon name="chevronRight" :size="14" />
      </button>
    </div>

    <div
      v-if="tabMenu.open"
      ref="tabMenuElement"
      class="editor-tab-context-menu"
      :style="{ left: tabMenu.x + 'px', top: tabMenu.y + 'px' }"
      role="menu"
    >
      <button type="button" role="menuitem" @click="handleTabMenuAction('close')">
        关闭
      </button>
      <button
        type="button"
        role="menuitem"
        :disabled="!canCloseOtherDocuments"
        @click="handleTabMenuAction('closeOthers')"
      >
        关闭其他
      </button>
      <button
        type="button"
        role="menuitem"
        :disabled="!canCloseRightDocuments"
        @click="handleTabMenuAction('closeRight')"
      >
        关闭右侧
      </button>
      <button
        type="button"
        role="menuitem"
        :disabled="!canCloseSavedDocuments"
        @click="handleTabMenuAction('closeSaved')"
      >
        关闭已保存
      </button>
      <button type="button" role="menuitem" @click="handleTabMenuAction('closeAll')">
        关闭全部
      </button>
    </div>

    <div class="editor-main">
      <div ref="editorContainer" class="editor-body"></div>
      <div v-if="startupError" class="editor-state editor-error-state">
        <div class="editor-error-title">打开失败</div>
        <div class="editor-error-message">{{ startupError }}</div>
        <div class="editor-error-actions">
          <button class="btn" @click="destroyWindow">关闭</button>
        </div>
      </div>
      <div v-else-if="activeDocument?.loading" class="editor-state">
        <span class="editor-spinner"></span>
        <span>正在打开文件，请稍候…</span>
      </div>
      <div
        v-else-if="activeDocument?.loadError"
        class="editor-state editor-error-state"
      >
        <div class="editor-error-title">打开失败</div>
        <div class="editor-error-message">{{ activeDocument.loadError }}</div>
        <div class="editor-error-actions">
          <button class="btn" @click="requestCloseDocument(activeDocument.key)">关闭</button>
          <button class="btn btn-primary" @click="retryLoad(activeDocument)">重新加载</button>
        </div>
      </div>
    </div>

    <div
      v-if="activeDocument && !activeDocument.loading && !activeDocument.loadError"
      class="editor-footer"
    >
      <label class="editor-lang">
        语言
        <select
          :value="activeDocument.language"
          class="editor-lang-select"
          @change="changeLanguage"
        >
          <option v-for="option in languageOptions" :key="option.value" :value="option.value">
            {{ option.label }}
          </option>
        </select>
        <span>自动：{{ activeDocument.detectedLanguage }}</span>
      </label>
      <transition name="editor-feedback">
        <span
          v-if="activeDocument.saveFeedback"
          :class="['editor-save-feedback', activeDocument.saveFeedback]"
          :title="activeDocument.saveFeedbackMessage"
          aria-live="polite"
        >
          {{ activeDocument.saveFeedbackMessage }}
        </span>
      </transition>
      <button
        class="btn btn-primary editor-save-button"
        :disabled="
          activeDocument.readOnly ||
          activeDocument.saving ||
          isSessionCloseLocked(activeDocument.sessionId)
        "
        :title="
          isSessionCloseLocked(activeDocument.sessionId)
            ? '会话正在关闭'
            : activeDocument.readOnly
            ? '只读文件，无写入权限'
            : activeDocument.saving
              ? '正在保存'
              : ''
        "
        :aria-busy="activeDocument.saving"
        @click="saveActiveDocument"
      >
        <span
          v-if="activeDocument.saving"
          class="editor-button-spinner"
          aria-hidden="true"
        ></span>
        <span>{{ activeDocument.saving ? "保存中" : "保存" }}</span>
      </button>
      <button
        v-if="activeDocument.saving && activeDocument.saveCanCancel"
        class="btn editor-save-cancel"
        :disabled="activeDocument.cancellingSave"
        title="取消当前文件保存"
        @click="cancelDocumentSave(activeDocument)"
      >
        {{ activeDocument.cancellingSave ? "取消中" : "取消" }}
      </button>
    </div>

    <AppDialog
      :key="activeBusinessConfirm?.id ?? 0"
      :open="Boolean(activeBusinessConfirm)"
      type="confirm"
      :title="activeBusinessConfirm?.title ?? ''"
      :message="activeBusinessConfirm?.message ?? ''"
      @confirm="settleActiveBusinessConfirm(true)"
      @cancel="settleActiveBusinessConfirm(false)"
    />

    <AppDialog
      :key="windowConfirm?.id ?? 0"
      :open="Boolean(windowConfirm)"
      type="confirm"
      :title="windowConfirm?.title ?? ''"
      :message="windowConfirm?.message ?? ''"
      window-modal
      @confirm="settleWindowConfirm(true)"
      @cancel="settleWindowConfirm(false)"
    />
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
.editor-tabbar {
  height: 32px;
  display: flex;
  align-items: flex-end;
  padding: 0 6px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-panel-alt);
  flex: 0 0 auto;
}
.editor-tabs {
  height: 100%;
  display: flex;
  align-items: flex-end;
  flex: 1;
  min-width: 0;
  overflow-x: auto;
  overflow-y: hidden;
}
.editor-tabs::-webkit-scrollbar {
  height: 0;
}
.editor-tab-context-menu {
  position: fixed;
  z-index: 1200;
  min-width: 132px;
  padding: 4px 0;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: #fff;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.18);
}
.editor-tab-context-menu button {
  width: 100%;
  padding: 6px 14px;
  border: none;
  background: transparent;
  color: var(--text);
  font-family: inherit;
  font-size: 12px;
  text-align: left;
  white-space: nowrap;
  cursor: pointer;
}
.editor-tab-context-menu button:hover:not(:disabled) {
  background: var(--row-hover);
}
.editor-tab-context-menu button:disabled {
  color: #bbb;
  cursor: default;
}
.editor-tab {
  height: 27px;
  min-width: 128px;
  max-width: 220px;
  display: flex;
  align-items: center;
  gap: 6px;
  margin-right: 3px;
  padding: 0 7px 0 9px;
  border: 1px solid var(--border);
  border-bottom: none;
  border-radius: 4px 4px 0 0;
  background: #e4e9ee;
  color: var(--text-secondary);
  cursor: pointer;
  user-select: none;
  flex: 0 0 auto;
}
.editor-tab:hover {
  background: #edf1f5;
}
.editor-tab.active {
  height: 29px;
  background: var(--bg-window);
  color: var(--text);
}
.editor-tab-session {
  max-width: 62px;
  overflow: hidden;
  color: var(--text-muted);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 0 1 auto;
}
.editor-tab-session::after {
  content: "·";
  margin-left: 5px;
}
.editor-tab-name {
  min-width: 0;
  overflow: hidden;
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
}
.editor-tab-close,
.editor-tab-scroll {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  flex: 0 0 auto;
}
.editor-tab-close {
  width: 18px;
  height: 18px;
  padding: 0;
  border-radius: 2px;
}
.editor-tab-close:hover {
  background: #d7dee5;
  color: var(--text);
}
.editor-tab-scroll {
  width: 22px;
  height: 28px;
  padding: 0;
}
.editor-tab-scroll:hover {
  color: var(--accent);
}
.editor-tab-dirty,
.editor-tab-error,
.editor-tab-spinner {
  width: 9px;
  height: 9px;
  flex: 0 0 auto;
}
.editor-tab-dirty {
  border-radius: 50%;
  background: var(--warning);
}
.editor-tab-error {
  border-radius: 50%;
  background: var(--danger);
}
.editor-tab-spinner {
  border: 1px solid #aeb9c4;
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: editor-spin 0.8s linear infinite;
}
.editor-main {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  background: #1e1e1e;
}
.editor-body {
  width: 100%;
  height: 100%;
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
  min-width: 0;
  margin-right: auto;
  color: var(--text-muted);
  font-size: 12px;
}
.editor-lang-select {
  height: 24px;
  flex: 0 0 auto;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: #fff;
  color: var(--text);
  font-size: 12px;
}
.editor-lang > span {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.editor-save-feedback {
  max-width: 45%;
  overflow: hidden;
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.editor-save-feedback.success {
  color: var(--success);
}
.editor-save-feedback.error {
  color: var(--danger);
}
.editor-save-button {
  width: 82px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
}
.editor-save-cancel {
  width: 66px;
  flex: 0 0 auto;
}
.editor-button-spinner {
  width: 12px;
  height: 12px;
  border: 2px solid rgba(255, 255, 255, 0.45);
  border-top-color: #fff;
  border-radius: 50%;
  animation: editor-spin 0.8s linear infinite;
  flex: 0 0 auto;
}
.editor-feedback-enter-active,
.editor-feedback-leave-active {
  transition: opacity 0.2s;
}
.editor-feedback-enter-from,
.editor-feedback-leave-to {
  opacity: 0;
}
.editor-state {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  background: var(--bg-window);
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
@media (max-width: 720px) {
  .editor-tab-session {
    display: none;
  }
  .editor-tab {
    min-width: 110px;
    max-width: 170px;
  }
}
</style>
