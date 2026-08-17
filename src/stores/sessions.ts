/**
 * 会话 store：管理右上终端选项卡对应的活动会话
 *
 * 每个会话对应一个后端 SSH 连接（sessionId 即后端标识），
 * 同时驱动左侧监控面板与右下文件管理器的数据来源
 */

import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type {
  ConnectionConfig,
  ConnectOutcome,
  ExtensionEntry,
  HostKeyApproval,
  HostKeyChallenge,
} from "../types";
import { sshConnect, sshDisconnect } from "../api";
import { genId } from "../utils";
import { useMonitorStore } from "./monitor";
import { useProxiesStore } from "./proxies";
import {
  sessionWorkspaceTabId,
  useWorkspacesStore,
} from "./workspaces";
import { closeTextEditorWindowsForSession } from "../editorWindows";

/**
 * 会话连接状态：
 * connecting 连接中 / verifying 等待主机密钥确认 / connected 已连接 /
 * error 首次连接失败 / disconnected 连接后掉线或退出
 */
export type SessionStatus =
  | "connecting"
  | "verifying"
  | "connected"
  | "error"
  | "disconnected";

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
  /** 本次连接使用的代理与隧道条目，非空时终端右侧显示连接信息按钮 */
  extensions?: ExtensionEntry[];
  /** 连接信息按钮相对终端区域顶部的垂直偏移（像素），拖拽后保留 */
  extensionOffsetY?: number;
  /** 本次连接是否已手动关闭异常闪烁，重连后重置 */
  extensionBlinkMuted?: boolean;
  /** 等待用户确认的服务端主机密钥 */
  hostKeyChallenge?: HostKeyChallenge;
  /** 主机密钥确认通过后是否需要在现有终端中重开通道 */
  reopenAfterHostKeyApproval?: boolean;
}

/** 按连接配置解析当前最新的共享代理快照 */
function resolveProxy(config: ConnectionConfig) {
  if (!config.proxyId) return undefined;
  const proxy = useProxiesStore().proxies.find((item) => item.id === config.proxyId);
  return proxy ? { ...proxy } : undefined;
}

export const useSessionsStore = defineStore("sessions", () => {
  /** 活动会话列表 */
  const sessions = ref<Session[]>([]);
  /** 当前激活的会话 id */
  const activeId = ref<string>("");
  /** 主工作区导航状态，SSH 资源仍由当前 store 管理 */
  const workspaces = useWorkspacesStore();
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

  /** 按稳定标识取得当前未关闭的响应式会话对象 */
  function currentSession(session: Session): Session | undefined {
    return sessions.value.find((item) => item.id === session.id);
  }

  /** 应用后端建连结果，返回是否需要在已有终端中重开通道 */
  async function applyConnectOutcome(
    session: Session,
    outcome: ConnectOutcome,
    reopenInPlace: boolean
  ): Promise<boolean> {
    const current = currentSession(session);
    if (!current) {
      // 连接期间选项卡已关闭时，清理由迟到结果建立的后端会话
      if (outcome.status === "connected") {
        try {
          await sshDisconnect(session.id);
        } catch {
          // 会话可能已由关闭动作清理
        }
      }
      return false;
    }

    if (outcome.status === "hostKeyConfirmationRequired") {
      current.hostKeyChallenge = outcome.challenge;
      current.reopenAfterHostKeyApproval = reopenInPlace;
      setStatus(current.id, "verifying");
      return false;
    }

    current.hostKeyChallenge = undefined;
    current.reopenAfterHostKeyApproval = false;
    current.extensions = outcome.result.extensions;
    setStatus(current.id, "connected");
    current.activity = false;
    // 连接成功后启动持续监控，与激活的选项卡无关
    useMonitorStore().start(current.id);
    return reopenInPlace;
  }

  /** 发起一次建连，并统一处理已连接或等待主机密钥确认两类结果 */
  async function connectSession(
    session: Session,
    approval?: HostKeyApproval,
    reopenInPlace = false
  ): Promise<boolean> {
    const proxy = resolveProxy(session.config);
    const outcome = await sshConnect(
      {
        ...session.config,
        id: session.id,
        proxy,
      },
      session.config.id,
      proxy?.id ?? null,
      approval
    );
    return applyConnectOutcome(session, outcome, reopenInPlace);
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
      extensions: [],
      extensionBlinkMuted: false,
    };
    sessions.value.push(session);
    activeId.value = id;
    workspaces.openSession(id, session.name);

    try {
      // 后端以该会话 id 建立连接，前端与后端共用同一标识
      await connectSession(session);
    } catch (e) {
      if (currentSession(session)) setStatus(id, "error", String(e));
    }
  }

  /** 信任当前展示的主机密钥，并使用完整公钥约束下一次握手 */
  async function approveHostKey(id: string): Promise<boolean> {
    const session = sessions.value.find((item) => item.id === id);
    const challenge = session?.hostKeyChallenge;
    if (!session || !challenge) return false;
    const reopenInPlace = session.reopenAfterHostKeyApproval === true;
    let waitForTerminalReopen = false;
    try {
      waitForTerminalReopen = await connectSession(
        session,
        {
          publicKey: challenge.publicKey,
          replaceExisting: challenge.kind === "changed",
        },
        reopenInPlace
      );
      return waitForTerminalReopen;
    } catch (error) {
      const current = currentSession(session);
      if (current) {
        current.hostKeyChallenge = undefined;
        current.reopenAfterHostKeyApproval = false;
        setStatus(id, "error", String(error));
      }
      return false;
    } finally {
      if (!waitForTerminalReopen && session.status !== "verifying") {
        reconnecting.delete(id);
      }
    }
  }

  /** 取消主机密钥确认并终止本次连接 */
  function rejectHostKey(id: string) {
    const session = sessions.value.find((item) => item.id === id);
    if (!session?.hostKeyChallenge) return;
    session.hostKeyChallenge = undefined;
    session.reopenAfterHostKeyApproval = false;
    reconnecting.delete(id);
    setStatus(id, "error", "已取消连接：未信任服务器主机密钥");
  }

  /** 关闭并断开指定会话 */
  async function close(id: string) {
    const idx = sessions.value.findIndex((s) => s.id === id);
    if (idx < 0) return;
    // 编辑窗口从属于选项卡，会话移除前先强制关闭其全部编辑窗口
    // 工作区模式下只移除该会话的文件标签，其他会话标签继续保留
    await closeTextEditorWindowsForSession(id);
    // 停止该会话监控
    useMonitorStore().stop(id);
    const [removed] = sessions.value.splice(idx, 1);
    const replacement = sessions.value.find(
      (session) => session.config.id === removed.config.id
    );
    workspaces.removeSession(id, replacement?.id);
    reconnecting.delete(id);
    // 关闭后激活相邻选项卡
    if (activeId.value === id) {
      const next = sessions.value[idx] ?? sessions.value[idx - 1];
      const workspaceSessionId = workspaces.activeTab?.sessionId;
      activeId.value = sessions.value.some((session) => session.id === workspaceSessionId)
        ? workspaceSessionId ?? ""
        : next?.id ?? "";
    }
    try {
      await sshDisconnect(removed.id);
    } catch {
      // 断开失败忽略，前端会话已移除
    }
  }

  /** 更新当前会话上下文，不改变主工作区正在展示的工具页 */
  function setActiveContext(id: string) {
    if (sessions.value.some((session) => session.id === id)) activeId.value = id;
  }

  /** 激活指定会话，并清除其未读输出提示 */
  function activate(id: string) {
    const s = sessions.value.find((x) => x.id === id);
    if (s) s.activity = false;
    activeId.value = id;
    workspaces.activateSession(id);
  }

  /**
   * 将指定会话移动到目标下标位置（指针拖拽排序用）
   */
  function moveToIndex(id: string, index: number) {
    workspaces.moveToIndex(sessionWorkspaceTabId(id), index);
  }

  /**
   * 重连指定会话：复用同一 sessionId 重建后端连接
   * 返回 true 表示终端组件已挂载、需由调用方在现有终端上重开通道以保留历史；
   * 返回 false 表示走全新连接（终端将随状态变为 connected 后自动挂载）
   */
  async function reconnect(id: string): Promise<boolean> {
    const s = sessions.value.find((x) => x.id === id);
    if (!s || reconnecting.has(id)) return false;
    // connected/disconnected 时终端组件仍挂载，可原地重开通道保留历史
    const hadTerminal = s.status === "connected" || s.status === "disconnected";
    reconnecting.add(id);
    // 重连视为新一次连接，扩展条目与闪烁抑制一并重置，拖拽位置保留
    s.extensions = [];
    s.extensionBlinkMuted = false;
    s.hostKeyChallenge = undefined;
    s.reopenAfterHostKeyApproval = false;
    useMonitorStore().stop(id);
    // 全新连接（首次失败或连接中）先置连接中以显示进度并触发终端挂载
    if (!hadTerminal) setStatus(id, "connecting");
    try {
      // 重连只释放旧 SSH 资源，保留传输任务供新连接恢复
      await sshDisconnect(id, true);
    } catch {
      // 旧连接可能已断开，忽略
    }
    let waitForTerminalReopen = false;
    try {
      waitForTerminalReopen = await connectSession(s, undefined, hadTerminal);
      return waitForTerminalReopen;
    } catch (e) {
      if (currentSession(s)) setStatus(id, "error", String(e));
      return false;
    } finally {
      if (!waitForTerminalReopen && s.status !== "verifying") {
        reconnecting.delete(id);
      }
    }
  }

  /** 终端通道完成重开后结束对应会话的重连保护 */
  function finishReconnect(id: string) {
    reconnecting.delete(id);
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
    const activeWorkspace = workspaces.activeTab;
    if (activeWorkspace?.type === "session" && activeWorkspace.sessionId === id) return;
    const s = sessions.value.find((x) => x.id === id);
    if (s && s.status === "connected") s.activity = true;
  }

  return {
    sessions,
    activeId,
    activeSession,
    open,
    close,
    setActiveContext,
    activate,
    moveToIndex,
    reconnect,
    finishReconnect,
    approveHostKey,
    rejectHostKey,
    markDisconnected,
    markActivity,
  };
});
