import { emitTo, listen } from "@tauri-apps/api/event";
import { getAllWebviewWindows, WebviewWindow } from "@tauri-apps/api/webviewWindow";
import type {
  EditorCloseSessionRequestPayload,
  EditorOpenRequestPayload,
  EditorReadyPayload,
  EditorRequestCompletedPayload,
  TextEditorWindowOptions,
} from "./editorProtocol";
import {
  EDITOR_CLOSE_SESSION_EVENT,
  EDITOR_OPENED_EVENT,
  EDITOR_OPEN_EVENT,
  EDITOR_READY_EVENT,
  EDITOR_SESSION_CLOSED_EVENT,
  EDITOR_WINDOW_LABEL,
} from "./editorProtocol";

export type { TextEditorWindowOptions } from "./editorProtocol";

const EDITOR_WINDOW_PREFIX = "editor-";
const EDITOR_RESPONSE_TIMEOUT = 8000;
let editorOperationQueue = Promise.resolve();
let requestSequence = 0;

/** 生成当前主窗口内唯一的编辑器请求标识 */
function nextRequestId(): string {
  requestSequence += 1;
  return `${Date.now()}-${requestSequence}`;
}

/** 显示并聚焦编辑器工作区窗口 */
async function raiseEditorWindow(window: WebviewWindow): Promise<void> {
  await window.unminimize();
  await window.show();
  await window.setFocus();
}

/** 串行执行编辑器窗口操作，避免创建、打开和关闭请求相互越过 */
function enqueueEditorOperation(operation: () => Promise<void>): Promise<void> {
  const pending = editorOperationQueue.then(operation, operation);
  editorOperationQueue = pending.then(
    () => undefined,
    () => undefined
  );
  return pending;
}

/** 等待指定请求的编辑器响应，并在超时后释放事件监听 */
async function waitForEditorResponse<T extends EditorRequestCompletedPayload>(
  eventName: string,
  requestId: string,
  failureMessage: string,
  action: () => Promise<void>
): Promise<T> {
  let resolveResponse: (payload: T) => void = () => undefined;
  let rejectResponse: (reason: Error) => void = () => undefined;
  const response = new Promise<T>((resolve, reject) => {
    resolveResponse = resolve;
    rejectResponse = reject;
  });
  const unlisten = await listen<T>(eventName, (event) => {
    if (event.payload.requestId === requestId) resolveResponse(event.payload);
  });
  const timeout = window.setTimeout(
    () => rejectResponse(new Error(failureMessage)),
    EDITOR_RESPONSE_TIMEOUT
  );
  try {
    const [, payload] = await Promise.all([action(), response]);
    return payload;
  } finally {
    window.clearTimeout(timeout);
    unlisten();
  }
}

/** 等待动态 Webview 窗口创建完成 */
function waitForWindowCreated(editorWindow: WebviewWindow): Promise<void> {
  return new Promise((resolve, reject) => {
    editorWindow.once("tauri://created", () => resolve());
    editorWindow.once("tauri://error", (event) => reject(event.payload));
  });
}

/** 创建单例编辑器工作区，并以查询参数传入首个文档 */
async function createEditorWindow(options: TextEditorWindowOptions): Promise<void> {
  const requestId = nextRequestId();
  const search = new URLSearchParams({
    window: "editor",
    startupRequestId: requestId,
    sessionId: options.sessionId,
    sessionName: options.sessionName,
    path: options.path,
    size: String(options.size),
  });
  let editorWindow: WebviewWindow | undefined;

  try {
    await waitForEditorResponse<EditorReadyPayload>(
      EDITOR_READY_EVENT,
      requestId,
      "文本编辑器启动超时",
      async () => {
        editorWindow = new WebviewWindow(EDITOR_WINDOW_LABEL, {
          url: `index.html?${search.toString()}`,
          title: "ZTShell 文本编辑器",
          width: 1000,
          height: 720,
          minWidth: 640,
          minHeight: 420,
          center: true,
          focus: true,
          visible: false,
          decorations: false,
          resizable: true,
          minimizable: true,
          maximizable: true,
          closable: true,
        });
        await waitForWindowCreated(editorWindow);
      }
    );
  } catch (error) {
    await editorWindow?.destroy().catch(() => undefined);
    throw error;
  }

  if (!editorWindow) throw new Error("文本编辑器窗口创建失败");
  await raiseEditorWindow(editorWindow);
}

/** 在已有工作区打开文档，并等待编辑器确认已接收 */
async function openDocumentInExistingWindow(
  editorWindow: WebviewWindow,
  options: TextEditorWindowOptions
): Promise<void> {
  const requestId = nextRequestId();
  const payload: EditorOpenRequestPayload = { requestId, document: options };
  await waitForEditorResponse<EditorRequestCompletedPayload>(
    EDITOR_OPENED_EVENT,
    requestId,
    "文本编辑器打开文件超时",
    () =>
      emitTo(
        { kind: "WebviewWindow", label: EDITOR_WINDOW_LABEL },
        EDITOR_OPEN_EVENT,
        payload
      )
  );
  await raiseEditorWindow(editorWindow);
}

/** 在单例编辑器工作区打开远端文档 */
export function openTextEditorWindow(options: TextEditorWindowOptions): Promise<void> {
  return enqueueEditorOperation(async () => {
    const existing = await WebviewWindow.getByLabel(EDITOR_WINDOW_LABEL);
    if (existing) {
      await openDocumentInExistingWindow(existing, options);
      return;
    }
    await createEditorWindow(options);
  });
}

/** 关闭指定会话在编辑器工作区中的全部文档 */
export function closeTextEditorWindowsForSession(sessionId: string): Promise<void> {
  return enqueueEditorOperation(async () => {
    const existing = await WebviewWindow.getByLabel(EDITOR_WINDOW_LABEL);
    if (!existing) return;
    const requestId = nextRequestId();
    const payload: EditorCloseSessionRequestPayload = { requestId, sessionId };
    await waitForEditorResponse<EditorRequestCompletedPayload>(
      EDITOR_SESSION_CLOSED_EVENT,
      requestId,
      "关闭会话所属文本编辑标签超时",
      () =>
        emitTo(
          { kind: "WebviewWindow", label: EDITOR_WINDOW_LABEL },
          EDITOR_CLOSE_SESSION_EVENT,
          payload
        )
    );
  });
}

/** 关闭应用当前打开的全部文本编辑窗口 */
export async function closeAllTextEditorWindows(): Promise<void> {
  await editorOperationQueue;
  const windows = (await getAllWebviewWindows()).filter((window) =>
    window.label.startsWith(EDITOR_WINDOW_PREFIX)
  );
  await Promise.allSettled(windows.map((window) => window.destroy()));
  const remaining = (await getAllWebviewWindows()).filter((window) =>
    window.label.startsWith(EDITOR_WINDOW_PREFIX)
  );
  if (remaining.length > 0) throw new Error("关闭全部文本编辑窗口失败");
}
