/** 文本编辑文档打开参数 */
export interface TextEditorWindowOptions {
  /** 会话标识 */
  sessionId: string;
  /** 会话显示名称 */
  sessionName: string;
  /** 远端文件路径 */
  path: string;
  /** 远端文件大小 */
  size: number;
}

/** 编辑器工作区就绪事件载荷 */
export interface EditorReadyPayload {
  /** 创建窗口时生成的请求标识 */
  requestId: string;
}

/** 打开文档请求载荷 */
export interface EditorOpenRequestPayload {
  /** 请求标识 */
  requestId: string;
  /** 待打开文档 */
  document: TextEditorWindowOptions;
}

/** 关闭会话文档请求载荷 */
export interface EditorCloseSessionRequestPayload {
  /** 请求标识 */
  requestId: string;
  /** 会话标识 */
  sessionId: string;
}

/** 准备关闭会话文档请求载荷 */
export interface EditorPrepareCloseRequestPayload {
  /** 请求标识 */
  requestId: string;
  /** 待锁定并查询的会话标识 */
  sessionIds: string[];
}

/** 释放会话文档关闭准备载荷 */
export interface EditorReleaseClosePreparationPayload {
  /** 准备请求标识 */
  requestId: string;
}

/** 编辑器请求完成事件载荷 */
export interface EditorRequestCompletedPayload {
  /** 请求标识 */
  requestId: string;
}

/** 会话文档关闭准备响应载荷 */
export interface EditorClosePreparedPayload extends EditorRequestCompletedPayload {
  /** 目标会话中尚未保存的文档数量 */
  dirtyCount: number;
}

/** 文档保存事件载荷 */
export interface EditorSavedPayload {
  /** 会话标识 */
  sessionId: string;
  /** 远端文件路径 */
  path: string;
}

/** 单例编辑器工作区窗口标签 */
export const EDITOR_WINDOW_LABEL = "editor-workspace";
/** 编辑器工作区就绪事件 */
export const EDITOR_READY_EVENT = "editor://ready";
/** 打开文档请求事件 */
export const EDITOR_OPEN_EVENT = "editor://open";
/** 文档已加入工作区事件 */
export const EDITOR_OPENED_EVENT = "editor://opened";
/** 关闭指定会话文档请求事件 */
export const EDITOR_CLOSE_SESSION_EVENT = "editor://close-session";
/** 编辑器已接收会话文档关闭请求事件 */
export const EDITOR_SESSION_CLOSE_READY_EVENT = "editor://session-close-ready";
/** 提交指定会话文档关闭事件 */
export const EDITOR_COMMIT_CLOSE_SESSION_EVENT = "editor://commit-close-session";
/** 取消指定会话文档关闭请求事件 */
export const EDITOR_CANCEL_CLOSE_SESSION_EVENT = "editor://cancel-close-session";
/** 指定会话文档已关闭事件 */
export const EDITOR_SESSION_CLOSED_EVENT = "editor://session-closed";
/** 准备关闭指定会话文档事件 */
export const EDITOR_PREPARE_CLOSE_EVENT = "editor://prepare-close";
/** 指定会话文档关闭准备完成事件 */
export const EDITOR_CLOSE_PREPARED_EVENT = "editor://close-prepared";
/** 释放指定会话文档关闭准备事件 */
export const EDITOR_RELEASE_CLOSE_PREPARATION_EVENT = "editor://release-close-preparation";
/** 文档保存完成事件 */
export const EDITOR_SAVED_EVENT = "editor://saved";
