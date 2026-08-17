// @ts-nocheck

import assert from "node:assert/strict";
import test from "node:test";
import { createPinia, setActivePinia } from "pinia";

import {
  sessionWorkspaceTabId,
  systemInfoWorkspaceTabId,
  useWorkspacesStore,
} from "./stores/workspaces.ts";

/** 创建相互隔离的工作区 store */
function createStore() {
  setActivePinia(createPinia());
  return useWorkspacesStore();
}

test("同一连接的多个会话共用系统信息选项卡并切换数据来源", () => {
  const store = createStore();
  store.openSession("session-a", "主机 A");
  store.openSession("session-a-2", "主机 A（2）");
  store.openSession("session-b", "主机 B");

  store.openSystemInfo("session-a", "connection-a", "生产环境");
  store.openSystemInfo("session-a-2", "connection-a", "生产环境");

  const systemInfoTabs = store.tabs.filter((tab) => tab.type === "systemInfo");
  assert.equal(systemInfoTabs.length, 1);
  assert.equal(store.activeId, systemInfoWorkspaceTabId("connection-a"));
  assert.equal(store.activeTab.title, "系统信息 - 生产环境");
  assert.equal(store.activeTab.sessionId, "session-a-2");
});

test("关闭工具选项卡不会移除关联 SSH 会话", () => {
  const store = createStore();
  store.openSession("session-a", "主机 A");
  store.openSystemInfo("session-a", "connection-a", "生产环境");

  store.close(systemInfoWorkspaceTabId("connection-a"));

  assert.deepEqual(store.tabs.map((tab) => tab.id), [sessionWorkspaceTabId("session-a")]);
  assert.equal(store.activeId, sessionWorkspaceTabId("session-a"));
});

test("通用关闭入口拒绝直接移除 SSH 会话页", () => {
  const store = createStore();
  store.openSession("session-a", "主机 A");

  store.close(sessionWorkspaceTabId("session-a"));

  assert.deepEqual(store.tabs.map((tab) => tab.id), [sessionWorkspaceTabId("session-a")]);
});

test("移除系统信息来源会话时优先迁移到同连接的其他会话", () => {
  const store = createStore();
  store.openSession("session-a", "主机 A");
  store.openSession("session-a-2", "主机 A（2）");
  store.openSystemInfo("session-a", "connection-a", "生产环境");
  store.activate(systemInfoWorkspaceTabId("connection-a"));

  store.removeSession("session-a", "session-a-2");

  assert.equal(store.activeId, systemInfoWorkspaceTabId("connection-a"));
  assert.equal(store.activeTab.sessionId, "session-a-2");
  assert.equal(
    store.tabs.some((tab) => tab.id === sessionWorkspaceTabId("session-a")),
    false
  );
});

test("移除连接的最后一个会话时同步移除系统信息页", () => {
  const store = createStore();
  store.openSession("session-a", "主机 A");
  store.openSystemInfo("session-a", "connection-a", "生产环境");
  store.openSession("session-b", "主机 B");
  store.activate(systemInfoWorkspaceTabId("connection-a"));

  store.removeSession("session-a");

  assert.deepEqual(store.tabs.map((tab) => tab.id), [sessionWorkspaceTabId("session-b")]);
  assert.equal(store.activeId, sessionWorkspaceTabId("session-b"));
});
