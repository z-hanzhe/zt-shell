import { getAllWebviewWindows, WebviewWindow } from "@tauri-apps/api/webviewWindow";

/** 文本编辑窗口创建参数 */
export interface TextEditorWindowOptions {
  /** 会话标识 */
  sessionId: string;
  /** 会话显示名称 */
  sessionName: string;
  /** 远端文件路径 */
  path: string;
}

const EDITOR_WINDOW_PREFIX = "editor-";

/** 根据会话和路径生成稳定且符合 Tauri 约束的窗口标签 */
async function editorWindowLabel(sessionId: string, path: string): Promise<string> {
  const source = new TextEncoder().encode(`${sessionId}\n${path}`);
  const digest = await crypto.subtle.digest("SHA-256", source);
  const pathHash = Array.from(new Uint8Array(digest))
    .slice(0, 12)
    .map((value) => value.toString(16).padStart(2, "0"))
    .join("");
  return `${EDITOR_WINDOW_PREFIX}${sessionId}-${pathHash}`;
}

/** 显示并聚焦已有编辑窗口 */
async function raiseEditorWindow(window: WebviewWindow): Promise<void> {
  await window.unminimize();
  await window.show();
  await window.setFocus();
}

/** 已存在相同会话和路径的编辑窗口时将其唤起 */
export async function focusExistingTextEditorWindow(
  sessionId: string,
  path: string
): Promise<boolean> {
  const label = await editorWindowLabel(sessionId, path);
  const existing = await WebviewWindow.getByLabel(label);
  if (!existing) return false;
  await raiseEditorWindow(existing);
  return true;
}

/** 创建独立文本编辑窗口；并发重复创建时回退为唤起已存在窗口 */
export async function openTextEditorWindow(options: TextEditorWindowOptions): Promise<void> {
  const label = await editorWindowLabel(options.sessionId, options.path);
  const existing = await WebviewWindow.getByLabel(label);
  if (existing) {
    await raiseEditorWindow(existing);
    return;
  }

  const search = new URLSearchParams({
    window: "editor",
    sessionId: options.sessionId,
    sessionName: options.sessionName,
    path: options.path,
  });
  const editorWindow = new WebviewWindow(label, {
    url: `index.html?${search.toString()}`,
    title: `${options.sessionName}：${options.path}`,
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

  await new Promise<void>((resolve, reject) => {
    editorWindow.once("tauri://created", async () => {
      try {
        await raiseEditorWindow(editorWindow);
        resolve();
      } catch (error) {
        reject(error);
      }
    });
    editorWindow.once("tauri://error", async (event) => {
      try {
        const concurrentWindow = await WebviewWindow.getByLabel(label);
        if (concurrentWindow) {
          await raiseEditorWindow(concurrentWindow);
          resolve();
          return;
        }
        reject(event.payload);
      } catch (error) {
        reject(error);
      }
    });
  });
}

/** 销毁匹配的编辑窗口，并复查是否仍有窗口残留 */
async function destroyMatchingEditorWindows(
  matches: (window: WebviewWindow) => boolean,
  failureMessage: string
): Promise<void> {
  const windows = (await getAllWebviewWindows()).filter(matches);
  await Promise.allSettled(windows.map((window) => window.destroy()));
  const remaining = (await getAllWebviewWindows()).filter(matches);
  if (remaining.length > 0) throw new Error(failureMessage);
}

/** 关闭指定会话的全部文本编辑窗口 */
export async function closeTextEditorWindowsForSession(sessionId: string): Promise<void> {
  const prefix = `${EDITOR_WINDOW_PREFIX}${sessionId}-`;
  await destroyMatchingEditorWindows(
    (window) => window.label.startsWith(prefix),
    "关闭会话所属文本编辑窗口失败"
  );
}

/** 关闭应用当前打开的全部文本编辑窗口 */
export async function closeAllTextEditorWindows(): Promise<void> {
  await destroyMatchingEditorWindows(
    (window) => window.label.startsWith(EDITOR_WINDOW_PREFIX),
    "关闭全部文本编辑窗口失败"
  );
}
