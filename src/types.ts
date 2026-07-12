/**
 * 与后端 Rust 数据结构对应的前端类型定义
 */

/** 认证方式 */
export type AuthType = "password" | "privateKey";

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
