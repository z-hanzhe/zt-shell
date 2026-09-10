// @ts-nocheck

import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import test from "node:test";
import { createPinia, setActivePinia } from "pinia";
import { nextTick } from "vue";
import { mockIPC } from "@tauri-apps/api/mocks";

// 仅在当前测试进程中补全应用模块的 TypeScript 扩展名。
registerHooks({
  /** 兼容前端源码由 Vite 解析的无扩展名相对导入 */
  resolve(specifier, context, nextResolve) {
    try {
      return nextResolve(specifier, context);
    } catch (error) {
      if (error.code === "ERR_MODULE_NOT_FOUND" && specifier.startsWith(".")) {
        return nextResolve(`${specifier}.ts`, context);
      }
      throw error;
    }
  },
});

const { useSessionsStore } = await import("./stores/sessions.ts");
const { useMonitorStore } = await import("./stores/monitor.ts");
const { useSettingsStore } = await import("./stores/settings.ts");
const { useConnectionsStore } = await import("./stores/connections.ts");

/** 构造不会连接真实服务器的测试配置 */
function connection(overrides = {}) {
  return {
    id: "feature-test",
    name: "功能开关测试",
    host: "test.invalid",
    port: 22,
    username: "tester",
    authType: "password",
    ...overrides,
  };
}

/** 隔离会话状态、IPC 记录和监控计时器，不创建真实网络或定时任务 */
function setup() {
  setActivePinia(createPinia());
  const calls = [];
  const timers = new Map();
  let timerId = 0;
  globalThis.window = {
    crypto: globalThis.crypto,
    /** 记录计时器而不启动真实轮询 */
    setInterval(callback) {
      timers.set(++timerId, callback);
      return timerId;
    },
    /** 移除对应会话的计时器 */
    clearInterval(id) {
      timers.delete(id);
    },
  };
  mockIPC((command, args) => {
    calls.push({ command, args });
    if (command === "ssh_connect") {
      return { status: "connected", result: { sessionId: args.config.id, extensions: [] } };
    }
    if (command === "monitor_collect") return { netInterfaces: [] };
    if (command === "plugin:window|get_all_windows") return [];
  });
  return { store: useSessionsStore(), monitor: useMonitorStore(), calls, timers };
}

test("本机持久化及复制连接保留功能开关", async () => {
  setup();
  const persisted = new Map();
  const invoke = window.__TAURI_INTERNALS__.invoke;
  window.__TAURI_INTERNALS__.invoke = async (command, args) => {
    if (command === "plugin:store|load") return 1;
    if (command === "plugin:store|get") return [persisted.get(args.key), persisted.has(args.key)];
    if (command === "plugin:store|set") {
      persisted.set(args.key, structuredClone(args.value));
      return;
    }
    return invoke(command, args);
  };
  const store = useConnectionsStore();
  await store.init();
  await store.upsert(connection({ monitorEnabled: false, sftpEnabled: false }));
  const copyId = await store.duplicateConnection("feature-test");
  store.connections = [];
  await store.init();
  assert.equal(store.connections.length, 2);
  assert.ok(store.connections.some((item) => item.id === copyId));
  for (const config of store.connections) {
    assert.equal(config.monitorEnabled, false);
    assert.equal(config.sftpEnabled, false);
  }
});

test("旧连接默认启用两个功能，并启动一次监控采集", async () => {
  const { store, monitor, calls, timers } = setup();
  await store.open(connection());
  const session = store.activeSession;
  assert.equal(session.config.monitorEnabled, true);
  assert.equal(session.config.sftpEnabled, true);
  assert.equal(calls[0].args.config.monitorEnabled, true);
  assert.equal(calls[0].args.config.sftpEnabled, true);
  assert.equal(calls.filter((call) => call.command === "monitor_collect").length, 1);
  assert.equal(timers.size, 1);
  assert.ok(monitor.state(session.id));
});

test("关闭监控后建连、重连、手动刷新及调整周期均不创建采集任务", async () => {
  const { store, monitor, calls, timers } = setup();
  await store.open(connection({ monitorEnabled: false, sftpEnabled: false }));
  const id = store.activeId;
  assert.equal(store.activeSession.status, "connected");
  await monitor.refresh(id);
  useSettingsStore().settings.monitorInterval = 10;
  await nextTick();
  await store.reconnect(id);
  store.finishReconnect(id);
  assert.equal(store.activeId, id);
  assert.equal(monitor.state(id), null);
  assert.equal(timers.size, 0);
  assert.equal(calls.some((call) => call.command === "monitor_collect"), false);
  for (const call of calls.filter((call) => call.command === "ssh_connect")) {
    assert.equal(call.args.config.monitorEnabled, false);
    assert.equal(call.args.config.sftpEnabled, false);
  }
});

test("两个功能开关互不影响", async () => {
  const { store, monitor, calls } = setup();
  await store.open(connection({ monitorEnabled: true, sftpEnabled: false }));
  const monitoredId = store.activeId;
  await store.open(connection({ monitorEnabled: false, sftpEnabled: true }));
  assert.ok(monitor.state(monitoredId));
  assert.equal(monitor.state(store.activeId), null);
  const connects = calls.filter((call) => call.command === "ssh_connect");
  assert.equal(connects[0].args.config.sftpEnabled, false);
  assert.equal(connects[1].args.config.sftpEnabled, true);
  assert.equal(calls.filter((call) => call.command === "monitor_collect").length, 1);
});

test("编辑已保存配置不改变现有会话及其重连使用的功能快照", async () => {
  const { store, calls } = setup();
  const config = connection({ monitorEnabled: false, sftpEnabled: false });
  await store.open(config);
  const existingId = store.activeId;
  config.monitorEnabled = true;
  config.sftpEnabled = true;
  await store.reconnect(existingId);
  store.finishReconnect(existingId);
  await store.open(config);
  const connects = calls.filter((call) => call.command === "ssh_connect");
  assert.equal(connects[1].args.config.monitorEnabled, false);
  assert.equal(connects[1].args.config.sftpEnabled, false);
  assert.equal(connects[2].args.config.monitorEnabled, true);
  assert.equal(connects[2].args.config.sftpEnabled, true);
});

test("主机密钥确认后继续遵守功能开关", async () => {
  const { store, monitor, calls, timers } = setup();
  const invoke = window.__TAURI_INTERNALS__.invoke;
  window.__TAURI_INTERNALS__.invoke = async (command, args) => {
    if (command === "ssh_connect" && !args.hostKeyApproval) {
      return {
        status: "hostKeyConfirmationRequired",
        challenge: { publicKey: "测试公钥", kind: "unknown" },
      };
    }
    return invoke(command, args);
  };
  await store.open(connection({ monitorEnabled: false, sftpEnabled: false }));
  const id = store.activeId;
  assert.equal(store.activeSession.status, "verifying");
  await store.approveHostKey(id, false);
  assert.equal(store.activeSession.status, "connected");
  assert.equal(calls.find((call) => call.command === "ssh_connect").args.config.sftpEnabled, false);
  assert.equal(monitor.state(id), null);
  assert.equal(timers.size, 0);
});

test("会话掉线后清理监控状态与计时器", async () => {
  const { store, monitor, timers } = setup();
  await store.open(connection());
  const id = store.activeId;
  store.markDisconnected(id);
  assert.equal(monitor.state(id), null);
  assert.equal(timers.size, 0);
});
