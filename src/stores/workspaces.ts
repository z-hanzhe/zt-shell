/**
 * 主工作区选项卡 store：统一管理 SSH 会话页与可扩展工具页的顺序和激活状态。
 *
 * SSH 资源生命周期仍由 sessions store 管理，本 store 只负责界面导航状态。
 */

import { computed, ref } from "vue";
import { defineStore } from "pinia";

/** SSH 终端工作区选项卡 */
export interface SessionWorkspaceTab {
  /** 工作区选项卡唯一标识 */
  id: string;
  /** 选项卡类型 */
  type: "session";
  /** 选项卡显示名称 */
  title: string;
  /** 关联的 SSH 会话标识 */
  sessionId: string;
}

/** 系统信息工作区选项卡 */
export interface SystemInfoWorkspaceTab {
  /** 工作区选项卡唯一标识 */
  id: string;
  /** 选项卡类型 */
  type: "systemInfo";
  /** 选项卡显示名称 */
  title: string;
  /** 提供监控数据的 SSH 会话标识 */
  sessionId: string;
  /** 关联的连接配置标识，同一连接的多个会话共用一个系统信息页 */
  connectionId: string;
}

/** 主工作区支持的选项卡联合类型 */
export type WorkspaceTab = SessionWorkspaceTab | SystemInfoWorkspaceTab;

/** 生成 SSH 会话对应的工作区选项卡标识 */
export function sessionWorkspaceTabId(sessionId: string): string {
  return `session:${sessionId}`;
}

/** 生成连接系统信息对应的工作区选项卡标识 */
export function systemInfoWorkspaceTabId(connectionId: string): string {
  return `system-info:${connectionId}`;
}

export const useWorkspacesStore = defineStore("workspaces", () => {
  /** 已打开的主工作区选项卡 */
  const tabs = ref<WorkspaceTab[]>([]);
  /** 当前激活的工作区选项卡标识 */
  const activeId = ref("");

  /** 当前激活的工作区选项卡 */
  const activeTab = computed(() => tabs.value.find((tab) => tab.id === activeId.value));

  /** 打开或激活指定 SSH 会话的终端选项卡 */
  function openSession(sessionId: string, title: string): void {
    const id = sessionWorkspaceTabId(sessionId);
    const existing = tabs.value.find((tab) => tab.id === id);
    if (existing) {
      existing.title = title;
    } else {
      tabs.value.push({ id, type: "session", title, sessionId });
    }
    activeId.value = id;
  }

  /** 打开或激活指定连接的系统信息选项卡，并切换到当前来源会话 */
  function openSystemInfo(
    sessionId: string,
    connectionId: string,
    connectionName: string
  ): void {
    const id = systemInfoWorkspaceTabId(connectionId);
    const title = `系统信息 - ${connectionName.trim() || "未命名连接"}`;
    const existing = tabs.value.find((tab) => tab.id === id);
    if (existing?.type === "systemInfo") {
      existing.title = title;
      existing.sessionId = sessionId;
      activeId.value = id;
      return;
    }

    const sessionIndex = tabs.value.findIndex(
      (tab) => tab.id === sessionWorkspaceTabId(sessionId)
    );
    const insertAt = sessionIndex >= 0 ? sessionIndex + 1 : tabs.value.length;
    tabs.value.splice(insertAt, 0, {
      id,
      type: "systemInfo",
      title,
      sessionId,
      connectionId,
    });
    activeId.value = id;
  }

  /** 激活指定工作区选项卡 */
  function activate(id: string): void {
    if (tabs.value.some((tab) => tab.id === id)) activeId.value = id;
  }

  /** 激活指定 SSH 会话的终端选项卡 */
  function activateSession(sessionId: string): void {
    activate(sessionWorkspaceTabId(sessionId));
  }

  /** 移除指定选项卡集合，并在当前页被移除时激活相邻页 */
  function removeTabs(ids: ReadonlySet<string>): void {
    if (ids.size === 0) return;
    const previousTabs = tabs.value;
    const activeIndex = previousTabs.findIndex((tab) => tab.id === activeId.value);
    const activeRemoved = ids.has(activeId.value);
    const removedBeforeActive = previousTabs
      .slice(0, Math.max(activeIndex, 0))
      .filter((tab) => ids.has(tab.id)).length;
    tabs.value = previousTabs.filter((tab) => !ids.has(tab.id));
    if (!activeRemoved) return;

    const nextIndex = Math.max(activeIndex - removedBeforeActive, 0);
    const next = tabs.value[nextIndex] ?? tabs.value[nextIndex - 1];
    activeId.value = next?.id ?? "";
  }

  /** 关闭指定非资源型工作区选项卡 */
  function close(id: string): void {
    const tab = tabs.value.find((item) => item.id === id);
    if (!tab || tab.type === "session") return;
    removeTabs(new Set([id]));
  }

  /** 移除指定会话终端页，并迁移或移除以它为数据源的系统信息页 */
  function removeSession(sessionId: string, replacementSessionId?: string): void {
    const ids = new Set([sessionWorkspaceTabId(sessionId)]);
    for (const tab of tabs.value) {
      if (tab.type !== "systemInfo" || tab.sessionId !== sessionId) continue;
      if (replacementSessionId) tab.sessionId = replacementSessionId;
      else ids.add(tab.id);
    }
    removeTabs(ids);
  }

  /** 将指定工作区选项卡移动到目标下标 */
  function moveToIndex(id: string, index: number): void {
    const from = tabs.value.findIndex((tab) => tab.id === id);
    if (from < 0) return;
    const to = Math.min(Math.max(index, 0), tabs.value.length - 1);
    if (from === to) return;
    const [tab] = tabs.value.splice(from, 1);
    tabs.value.splice(to, 0, tab);
  }

  return {
    tabs,
    activeId,
    activeTab,
    openSession,
    openSystemInfo,
    activate,
    activateSession,
    close,
    removeSession,
    moveToIndex,
  };
});
