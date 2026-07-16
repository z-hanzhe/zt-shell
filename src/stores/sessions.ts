/**
 * 会话 store：管理右上终端选项卡对应的活动会话
 *
 * 每个会话对应一个后端 SSH 连接（sessionId 即后端标识），
 * 同时驱动左侧监控面板与右下文件管理器的数据来源
 */

import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { ConnectionConfig } from "../types";
import { sshConnect, sshDisconnect } from "../api";
import { genId } from "../utils";
import { useMonitorStore } from "./monitor";

/**
 * 会话连接状态：
 * connecting 连接中 / connected 已连接 / error 首次连接失败 / disconnected 连接后掉线或退出
 */
export type SessionStatus = "connecting" | "connected" | "error" | "disconnected";

/** 一个活动会话 */
export interface Session {
  /** 会话唯一标识（同时作为后端 sessionId） */
  id: string;
  /** 选项卡显示名称 */
  name: string;
  /** 连接配置 */
  config: ConnectionConfig;
  /** 连接状态 */
  status: SessionStatus;
  /** 错误信息（status 为 error 时） */
  error?: string;
  /** 未选中时有新输出的提示标记（仿 xshell 叹号提示） */
  activity?: boolean;
}

export const useSessionsStore = defineStore("sessions", () => {
  /** 活动会话列表 */
  const sessions = ref<Session[]>([]);
  /** 当前激活的会话 id */
  const activeId = ref<string>("");
  // 正在执行受控重连的会话，用于抑制重连过程中后端断开触发的掉线标记
  const reconnecting = new Set<string>();

  /** 当前激活的会话对象 */
  const activeSession = computed(() =>
    sessions.value.find((s) => s.id === activeId.value)
  );

  /** 按 id 更新会话状态，通过响应式数组元素修改以确保视图刷新 */
  function setStatus(id: string, status: SessionStatus, error?: string) {
    const s = sessions.value.find((x) => x.id === id);
    if (s) {
      s.status = status;
      s.error = error;
    }
  }

  /** 依据连接配置打开新会话并发起连接 */
  async function open(config: ConnectionConfig) {
    const id = genId();
    const session: Session = {
      id,
      name: config.name || config.host,
      config,
      status: "connecting",
      activity: false,
    };
    sessions.value.push(session);
    activeId.value = id;

    try {
      // 后端以该会话 id 建立连接，前端与后端共用同一标识
      await sshConnect({ ...config, id });
      setStatus(id, "connected");
      // 连接成功后启动持续监控，与激活的选项卡无关
      useMonitorStore().start(id);
    } catch (e) {
      setStatus(id, "error", String(e));
    }
  }

  /** 关闭并断开指定会话 */
  async function close(id: string) {
    const idx = sessions.value.findIndex((s) => s.id === id);
    if (idx < 0) return;
    // 停止该会话监控
    useMonitorStore().stop(id);
    const [removed] = sessions.value.splice(idx, 1);
    try {
      await sshDisconnect(removed.id);
    } catch {
      // 断开失败忽略，前端会话已移除
    }
    // 关闭后激活相邻选项卡
    if (activeId.value === id) {
      const next = sessions.value[idx] ?? sessions.value[idx - 1];
      activeId.value = next ? next.id : "";
    }
  }

  /** 激活指定会话，并清除其未读输出提示 */
  function activate(id: string) {
    const s = sessions.value.find((x) => x.id === id);
    if (s) s.activity = false;
    activeId.value = id;
  }

  /**
   * 将指定会话移动到目标下标位置（指针拖拽排序用）
   */
  function moveToIndex(id: string, index: number) {
    const from = sessions.value.findIndex((s) => s.id === id);
    if (from < 0) return;
    const to = Math.min(Math.max(index, 0), sessions.value.length - 1);
    if (from === to) return;
    const [item] = sessions.value.splice(from, 1);
    sessions.value.splice(to, 0, item);
  }

  /**
   * 重连指定会话：复用同一 sessionId 重建后端连接
   * 返回 true 表示终端组件已挂载、需由调用方在现有终端上重开通道以保留历史；
   * 返回 false 表示走全新连接（终端将随状态变为 connected 后自动挂载）
   */
  async function reconnect(id: string): Promise<boolean> {
    const s = sessions.value.find((x) => x.id === id);
    if (!s) return false;
    // connected/disconnected 时终端组件仍挂载，可原地重开通道保留历史
    const hadTerminal = s.status === "connected" || s.status === "disconnected";
    reconnecting.add(id);
    useMonitorStore().stop(id);
    // 全新连接（首次失败或连接中）先置连接中以显示进度并触发终端挂载
    if (!hadTerminal) setStatus(id, "connecting");
    try {
      await sshDisconnect(id);
    } catch {
      // 旧连接可能已断开，忽略
    }
    try {
      await sshConnect({ ...s.config, id });
      setStatus(id, "connected");
      s.activity = false;
      useMonitorStore().start(id);
      return hadTerminal;
    } catch (e) {
      setStatus(id, "error", String(e));
      return false;
    } finally {
      reconnecting.delete(id);
    }
  }

  /**
   * 标记会话掉线：由终端连接关闭事件驱动（远端 exit、网络断开等）
   * 仅对处于已连接状态且非受控重连中的会话生效
   */
  function markDisconnected(id: string) {
    if (reconnecting.has(id)) return;
    const s = sessions.value.find((x) => x.id === id);
    if (!s || s.status !== "connected") return;
    setStatus(id, "disconnected");
    useMonitorStore().stop(id);
  }

  /** 标记未选中会话有新输出（激活中的会话不标记） */
  function markActivity(id: string) {
    if (id === activeId.value) return;
    const s = sessions.value.find((x) => x.id === id);
    if (s && s.status === "connected") s.activity = true;
  }

  return {
    sessions,
    activeId,
    activeSession,
    open,
    close,
    activate,
    moveToIndex,
    reconnect,
    markDisconnected,
    markActivity,
  };
});
