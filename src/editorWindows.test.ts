// @ts-nocheck

import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import test from "node:test";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

// 测试直接执行生产模块，为项目使用的无扩展名相对导入补齐 TypeScript 扩展名。
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier.startsWith(".") && !/\.[a-z]+$/i.test(specifier)) {
      return nextResolve(`${specifier}.ts`, context);
    }
    return nextResolve(specifier, context);
  },
});
globalThis.window = {
  crypto: globalThis.crypto,
  setTimeout: (callback, delay) => setTimeout(callback, delay === 8000 ? 300 : delay),
  clearTimeout,
};
mockWindows("main");
const editor = await import("./editorWindows.ts");
const protocol = await import("./editorProtocol.ts");
const documentOptions = { sessionId: "会话一", sessionName: "测试", path: "/文本.txt", size: 12 };

/** 模拟登记表与原生窗口分别存在，覆盖失效登记、销毁延迟和事件丢失。 */
function setup({ live = true, stale = false, onEvent, onExists } = {}) {
  const originalLabel = protocol.EDITOR_WINDOW_LABEL;
  const windows = new Map(stale || live ? [[originalLabel, live]] : []);
  const listeners = new Map();
  const events = [];
  const created = [];
  const destroyed = [];
  let sequence = 0;

  /** 将编辑器响应投递给真实生产代码注册的监听器。 */
  function respond(event, payload) {
    for (const [id, entry] of listeners) {
      if (entry.event === event) {
        window.__TAURI_INTERNALS__.runCallback(entry.handler, { id, event, payload });
      }
    }
  }

  const state = { windows, events, created, destroyed, respond, originalLabel, listeners };
  mockIPC(async (command, args) => {
    switch (command) {
      case "plugin:window|get_all_windows": return ["main", ...windows.keys()];
      case "text_editor_window_exists":
        onExists?.(args.label, state);
        return windows.get(args.label) === true;
      case "plugin:event|listen": {
        const id = ++sequence;
        listeners.set(id, args);
        return id;
      }
      case "plugin:event|unlisten": listeners.delete(args.eventId); return;
      case "plugin:window|unminimize":
      case "plugin:window|show":
      case "plugin:window|set_focus":
        events.push(command);
        return;
      case "plugin:webview|create_webview_window": {
        const { label, url } = args.options;
        assert.equal(windows.has(label), false, "创建时不得复用残留标签");
        created.push(label);
        windows.set(label, true);
        const requestId = new URL(url, "http://localhost").searchParams.get("startupRequestId");
        setImmediate(() => respond(protocol.EDITOR_READY_EVENT, { requestId }));
        return;
      }
      case "plugin:window|destroy":
        destroyed.push(args.label);
        // 故意保留登记表记录，原生销毁发生在命令返回之后。
        setTimeout(() => windows.set(args.label, false), 10);
        return;
      case "plugin:event|emit_to": {
        events.push(args.event);
        if (onEvent && await onEvent(args, state)) return;
        const { requestId } = args.payload;
        if (args.event === protocol.EDITOR_OPEN_EVENT) {
          respond(protocol.EDITOR_OPENED_EVENT, { requestId });
        } else if (args.event === protocol.EDITOR_PREPARE_CLOSE_EVENT) {
          respond(protocol.EDITOR_CLOSE_PREPARED_EVENT, { requestId, dirtyCount: 1 });
        } else if (args.event === protocol.EDITOR_CLOSE_SESSION_EVENT) {
          respond(protocol.EDITOR_SESSION_CLOSE_READY_EVENT, { requestId });
        } else if (args.event === protocol.EDITOR_COMMIT_CLOSE_SESSION_EVENT) {
          respond(protocol.EDITOR_SESSION_CLOSED_EVENT, { requestId, windowClosing: false });
        }
        return;
      }
      default: throw new Error(`未模拟的命令：${command}`);
    }
  });
  return state;
}

test("原生窗口已消失而登记仍存在时，使用新标识重建编辑器", async () => {
  const state = setup({ live: false, stale: true });
  await editor.openTextEditorWindow(documentOptions);
  assert.equal(state.created.length, 1);
  assert.notEqual(state.created[0], state.originalLabel);
  assert.equal(state.events.includes(protocol.EDITOR_OPEN_EVENT), false);
  assert.equal(state.listeners.size, 0);
});

test("失效登记不会阻挡会话关闭准备与会话文档关闭", async () => {
  const state = setup({ live: false, stale: true });
  const preparation = await editor.prepareTextEditorClose([documentOptions.sessionId]);
  assert.equal(preparation.dirtyCount, 0);
  await editor.closeTextEditorWindowsForSession(documentOptions.sessionId);
  await preparation.release();
  assert.deepEqual(state.events, []);
});

test("窗口在打开请求期间销毁时重建，存活但不响应时保留文档并报错", async () => {
  const vanished = setup({ onEvent(args, state) {
    if (args.event !== protocol.EDITOR_OPEN_EVENT) return false;
    state.windows.set(state.originalLabel, false);
    return true;
  } });
  await editor.openTextEditorWindow(documentOptions);
  assert.equal(vanished.created.length, 1);

  const unresponsive = setup({ onEvent: (args) => args.event === protocol.EDITOR_OPEN_EVENT });
  await assert.rejects(editor.openTextEditorWindow(documentOptions), /文本编辑器打开文件超时/);
  assert.deepEqual(unresponsive.created, []);
  assert.deepEqual(unresponsive.destroyed, []);
  assert.equal(unresponsive.listeners.size, 0);
});

test("唤起已有窗口后才发送打开请求", async () => {
  const state = setup();
  await editor.openTextEditorWindow(documentOptions);
  assert.ok(state.events.indexOf("plugin:window|show") < state.events.indexOf(protocol.EDITOR_OPEN_EVENT));
  assert.deepEqual(state.created, []);
});

test("关闭提交等待文档完成及原生销毁后才允许下一个打开请求", async () => {
  const state = setup({ onEvent(args, state) {
    if (args.event !== protocol.EDITOR_COMMIT_CLOSE_SESSION_EVENT) return false;
    setTimeout(() => {
      state.respond(protocol.EDITOR_SESSION_CLOSED_EVENT, {
        requestId: args.payload.requestId,
        windowClosing: true,
      });
      setTimeout(() => state.windows.set(state.originalLabel, false), 10);
    }, 10);
    return true;
  } });
  await Promise.all([
    editor.closeTextEditorWindowsForSession(documentOptions.sessionId),
    editor.openTextEditorWindow(documentOptions),
  ]);
  assert.equal(state.created.length, 1);
  assert.equal(state.events.includes(protocol.EDITOR_OPEN_EVENT), false);
  assert.equal(state.listeners.size, 0);
});

test("关闭准备释放失败也释放本地锁，后续打开操作可以继续", async () => {
  const state = setup({ onEvent(args) {
    if (args.event === protocol.EDITOR_RELEASE_CLOSE_PREPARATION_EVENT) {
      throw new Error("模拟释放事件发送失败");
    }
    return false;
  } });
  const preparation = await editor.prepareTextEditorClose([documentOptions.sessionId]);
  assert.equal(preparation.dirtyCount, 1);
  await assert.rejects(preparation.release(), /模拟释放事件发送失败/);
  await editor.openTextEditorWindow(documentOptions);
  assert.ok(state.events.includes(protocol.EDITOR_OPEN_EVENT));
});

test("仍存活的编辑器关闭准备超时必须阻止关闭，失败后释放本地锁", async () => {
  const state = setup({ onEvent: (args) => args.event === protocol.EDITOR_PREPARE_CLOSE_EVENT });
  await assert.rejects(editor.prepareTextEditorClose([documentOptions.sessionId]), /准备关闭文本编辑超时/);
  assert.deepEqual(state.destroyed, []);
  assert.ok(state.events.includes(protocol.EDITOR_RELEASE_CLOSE_PREPARATION_EVENT));
  await editor.openTextEditorWindow(documentOptions);
  assert.equal(state.listeners.size, 0);
});

test("退出等待真实销毁完成，残留登记不算关闭失败", async () => {
  const state = setup();
  await editor.closeAllTextEditorWindows();
  assert.deepEqual(state.destroyed, [state.originalLabel]);
  assert.equal(state.windows.get(state.originalLabel), false);
});
