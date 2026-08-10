/**
 * 与后端 Rust 数据结构对应的前端类型定义
 */

/** 认证方式 */
export type AuthType = "password" | "privateKey";

/** 代理协议 */
export type ProxyType = "socks4" | "socks4a" | "socks5" | "http";

/** 隧道类型 */
export type TunnelType = "local" | "remote" | "dynamic" | "dynamicHttp";

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
  /** 系统凭据库中是否已有代理密码 */
  hasPassword?: boolean;
}

/** SSH 隧道配置 */
export interface TunnelConfig {
  /** 隧道唯一标识 */
  id: string;
  /** 隧道显示名称 */
  name: string;
  /** 隧道类型 */
  tunnelType: TunnelType;
  /** 是否启用，未勾选的隧道不随会话启动 */
  enabled: boolean;
  /** 是否仅接受监听端本机连接 */
  localOnly: boolean;
  /** 监听端口：本地/动态为本机端口，远程为服务器端口 */
  listenPort: number;
  /** 目标主机：本地为服务器侧目标，远程为客户端侧目标 */
  targetHost?: string;
  /** 目标端口 */
  targetPort?: number;
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
  /** 系统凭据库中是否已有登录密码 */
  hasPassword?: boolean;
  privateKeyPath?: string;
  passphrase?: string;
  /** 系统凭据库中是否已有私钥口令 */
  hasPassphrase?: boolean;
  /** 使用的共享代理 id，空或 null 表示直连 */
  proxyId?: string | null;
  /** 用户备注 */
  remark?: string;
  /** 当前连接的隧道列表，运行时每个会话独立启动 */
  tunnels?: TunnelConfig[];
  /** 所属文件夹 id，空或 null 表示位于根目录 */
  parentId?: string | null;
  /** 同级显示顺序，由连接管理器维护 */
  order?: number;
}

/** 会话连接信息条目类别 */
export type ExtensionKind = "proxy" | "tunnel";

/** 会话连接信息条目：本次连接使用的代理与隧道及其成功失败状态 */
export interface ExtensionEntry {
  /** 条目类别 */
  kind: ExtensionKind;
  /** 条目显示名称 */
  name: string;
  /** 类型描述，如 SOCKS5、本地拨出 */
  category: string;
  /** 使用明细，如 本机 8080 → 服务器侧 db:3306 */
  detail: string;
  /** 是否正常启用 */
  ok: boolean;
  /** 失败原因，ok 为 true 时为空 */
  error: string;
}

/** SSH 建连结果 */
export interface ConnectResult {
  /** 会话标识 */
  sessionId: string;
  /** 本次连接的代理与隧道条目，存在 ok 为 false 的条目表示部分功能未启用 */
  extensions: ExtensionEntry[];
}

/** 系统凭据类别 */
export type CredentialKind =
  | "connectionPassword"
  | "connectionPassphrase"
  | "proxyPassword";

/** 系统凭据定位键 */
export interface CredentialKey {
  /** 凭据类别 */
  kind: CredentialKind;
  /** 连接或代理的稳定标识 */
  id: string;
}

/** 写入系统凭据库的凭据 */
export interface CredentialWrite extends CredentialKey {
  /** 凭据明文，仅在写入调用期间存在 */
  value: string;
}

/** 与系统凭据库中已存代理密码进行比较的参数 */
export interface CredentialMatch extends CredentialKey {
  /** 候选明文，仅在比较调用期间存在 */
  value: string;
}

/** 复制系统凭据库中的凭据 */
export interface CredentialCopy {
  /** 凭据类别 */
  kind: CredentialKind;
  /** 来源连接或代理标识 */
  sourceId: string;
  /** 目标连接或代理标识 */
  targetId: string;
}

/** 编辑器对一项凭据的修改意图 */
export type SecretChange =
  | { mode: "keep" }
  | { mode: "set"; value: string }
  | { mode: "clear" };

/** 连接编辑器提交的凭据修改 */
export interface ConnectionSecretChanges {
  /** 登录密码修改 */
  password: SecretChange;
  /** 私钥口令修改 */
  passphrase: SecretChange;
}

/** 导出记录与本地稳定标识的凭据来源映射 */
export interface ConnectionExportCredentialSource {
  /** 导出 JSON 中连接或代理的文件内引用 */
  exportRef: string;
  /** 系统凭据库使用的本地连接或代理稳定标识 */
  sourceId: string;
}

/** 导出内容使用的连接与代理凭据来源 */
export interface ConnectionExportCredentialSources {
  /** 导出连接的凭据来源 */
  connections: ConnectionExportCredentialSource[];
  /** 导出代理的凭据来源 */
  proxies: ConnectionExportCredentialSource[];
}

/** 主机密钥确认场景 */
export type HostKeyConfirmationKind = "unknown" | "changed";

/** 等待用户确认的服务端主机密钥 */
export interface HostKeyChallenge {
  /** 首次连接或已有密钥发生变化 */
  kind: HostKeyConfirmationKind;
  /** 建连使用的目标主机 */
  host: string;
  /** 建连使用的目标端口 */
  port: number;
  /** 服务端本次提供的密钥算法 */
  algorithm: string;
  /** 服务端本次提供的 SHA-256 指纹 */
  fingerprint: string;
  /** 已保存密钥的 SHA-256 指纹，首次连接时为空 */
  knownFingerprint: string | null;
  /** 完整服务端公钥，仅用于确认后的二次握手精确匹配 */
  publicKey: string;
}

/** 用户对一次主机密钥确认的授权 */
export interface HostKeyApproval {
  /** 用户确认时看到的完整服务端公钥 */
  publicKey: string;
  /** 是否允许替换当前主机与端口的已有可信密钥 */
  replaceExisting: boolean;
}

/** SSH 建连命令结果 */
export type ConnectOutcome =
  | { status: "connected"; result: ConnectResult }
  | { status: "hostKeyConfirmationRequired"; challenge: HostKeyChallenge };

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
