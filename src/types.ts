/**
 * 与后端 Rust 数据结构对应的前端类型定义
 */

/** 认证方式 */
export type AuthType = "password" | "privateKey";

/** 代理协议 */
export type ProxyType = "socks4" | "socks4a" | "socks5" | "http";

/** 可复用代理配置 */
export interface ProxyConfig {
  /** 代理唯一标识 */
  id: string;
  /** 代理显示名称 */
  name: string;
  /** 代理协议 */
  proxyType: ProxyType;
  /** 代理服务器地址 */
  host: string;
  /** 代理服务器端口 */
  port: number;
  /** SOCKS4 用户标识或 SOCKS5/HTTP 用户名 */
  username?: string;
  /** SOCKS5 密码或 HTTP Basic 密码 */
  password?: string;
}

/** 连接配置 */
export interface ConnectionConfig {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  authType: AuthType;
  password?: string;
  privateKeyPath?: string;
  passphrase?: string;
  /** 使用的共享代理 id，空或 null 表示直连 */
  proxyId?: string | null;
  /** 所属文件夹 id，空或 null 表示位于根目录 */
  parentId?: string | null;
  /** 同级显示顺序，由连接管理器维护 */
  order?: number;
}

/** 连接分组文件夹（支持多级嵌套） */
export interface ConnectionFolder {
  /** 文件夹唯一标识 */
  id: string;
  /** 文件夹显示名称 */
  name: string;
  /** 父文件夹 id，null 表示位于根目录 */
  parentId: string | null;
  /** 同级显示顺序，由连接管理器维护 */
  order?: number;
}

/** SFTP 文件条目 */
export interface FileEntry {
  name: string;
  isDir: boolean;
  isSymlink: boolean;
  size: number;
  permissions: number;
  permissionsStr: string;
  modified: number;
  owner: string;
  group: string;
}

/** 网卡监控 */
export interface NetInterface {
  name: string;
  rxRate: number;
  txRate: number;
  rxTotal: number;
  txTotal: number;
  /** 是否为物理网卡 */
  isPhysical: boolean;
}

/** 磁盘使用 */
export interface DiskUsage {
  filesystem: string;
  mount: string;
  total: number;
  used: number;
  available: number;
  usePercent: number;
}

/** 进程信息 */
export interface ProcessInfo {
  pid: number;
  name: string;
  cpu: number;
  mem: number;
  /** 实际内存占用（字节） */
  memBytes: number;
}

/** 完整监控数据 */
export interface MonitorData {
  hostname: string;
  os: string;
  kernel: string;
  uptime: number;
  cpuCount: number;
  cpuUsage: number;
  loadAvg: [number, number, number];
  memTotal: number;
  memUsed: number;
  memAvailable: number;
  swapTotal: number;
  swapUsed: number;
  netInterfaces: NetInterface[];
  disks: DiskUsage[];
  processes: ProcessInfo[];
}

/** 传输任务状态 */
export type TransferStatus =
  | "pending"
  | "running"
  | "packing"
  | "paused"
  | "failed"
  | "completed"
  | "cancelled";

/** 传输方向 */
export type TransferKind = "upload" | "download";

/** 传输任务（transfer://changed 全量与 transfer_list 返回） */
export interface TransferTask {
  id: string;
  /** 父任务标识，顶层任务为 null */
  parentId: string | null;
  sessionId: string;
  kind: TransferKind;
  isDir: boolean;
  name: string;
  localPath: string;
  remotePath: string;
  status: TransferStatus;
  /** 已传输字节数 */
  transferred: number;
  /** 总字节数 */
  total: number;
  /** 当前速度（字节/秒） */
  speed: number;
  /** 预计剩余秒数，-1 表示未知 */
  etaSecs: number;
  /** 累计传输耗时（毫秒） */
  elapsedMs: number;
  /** 失败原因 */
  error: string;
}

/** 传输任务动态字段（transfer://progress 增量） */
export interface TransferProgress {
  id: string;
  status: TransferStatus;
  transferred: number;
  total: number;
  speed: number;
  etaSecs: number;
  elapsedMs: number;
  error: string;
}

/** 创建传输任务的返回：needConfirm 或 existNames 非空时未建任务，需确认后重调 */
export interface TransferCreateResult {
  needConfirm: boolean;
  /** 本次待传文件数 */
  fileCount: number;
  /** 会话内已存在的未完成任务数 */
  activeCount: number;
  /** 目标位置已存在的同名条目，非空时需确认覆盖 */
  existNames: string[];
}
