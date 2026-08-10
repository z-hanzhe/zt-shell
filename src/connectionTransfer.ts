/**
 * 连接导入导出的纯数据算法：负责外部契约、范围筛选、严格校验与合并计划生成。
 */

import type {
  AuthType,
  ConnectionConfig,
  ConnectionFolder,
  ProxyConfig,
  ProxyType,
  TunnelConfig,
  TunnelType,
} from "./types";

/** 连接导出文件的固定格式标识 */
export const CONNECTION_EXPORT_FORMAT = "ztshell-connections" as const;
/** 当前连接导出文件版本 */
export const CONNECTION_EXPORT_VERSION = 1 as const;

/** 导入导出算法使用的本地数据快照 */
export interface ConnectionTransferSnapshot {
  connections: readonly ConnectionConfig[];
  folders: readonly ConnectionFolder[];
  proxies: readonly ProxyConfig[];
}

/** 连接导出范围 */
export type ConnectionExportScope =
  | { kind: "all" }
  | { kind: "connection"; id: string }
  | { kind: "folder"; id: string };

/** 导出时可注入的确定性依赖 */
export interface ConnectionExportOptions {
  now?: () => Date;
}

/** V1 文件中的文件夹记录，ref 仅在当前文件内建立层级引用 */
export interface ConnectionExportFolderV1 {
  ref: string;
  name: string;
  parentRef: string | null;
  order: number;
}

/** V1 文件中的隧道记录，不保留内部 id */
export interface ConnectionExportTunnelV1 {
  name: string;
  tunnelType: TunnelType;
  enabled: boolean;
  localOnly: boolean;
  listenPort: number;
  targetHost: string | null;
  targetPort: number | null;
}

/** V1 文件中的连接记录，不保留内部 id */
export interface ConnectionExportConnectionV1 {
  ref: string;
  name: string;
  host: string;
  port: number;
  username: string;
  authType: AuthType;
  password: string | null;
  privateKeyPath: string | null;
  passphrase: string | null;
  proxyRef: string | null;
  remark: string | null;
  tunnels: ConnectionExportTunnelV1[];
  parentRef: string | null;
  order: number;
}

/** V1 文件中的代理记录，ref 仅供连接记录引用 */
export interface ConnectionExportProxyV1 {
  ref: string;
  name: string;
  proxyType: ProxyType;
  host: string;
  port: number;
  username: string | null;
  password: string | null;
}

/** ZTShell 连接导出文件 V1 契约 */
export interface ConnectionExportFileV1 {
  format: typeof CONNECTION_EXPORT_FORMAT;
  version: typeof CONNECTION_EXPORT_VERSION;
  exportedAt: string;
  folders: ConnectionExportFolderV1[];
  connections: ConnectionExportConnectionV1[];
  proxies: ConnectionExportProxyV1[];
}

/** 导入规划使用的确定性依赖 */
export interface ConnectionImportOptions {
  idFactory: () => string;
  /** Rust 凭据比对后确认可复用的代理映射，键为文件代理 ref，值为本地代理 id */
  matchedProxyIds?: ReadonlyMap<string, string>;
}

/** 导入规划统计 */
export interface ConnectionImportSummary {
  connectionCount: number;
  folderCount: number;
  reusedFolderCount: number;
  addedProxyCount: number;
  reusedProxyCount: number;
  renamedConnectionCount: number;
  privateKeyConnectionCount: number;
}

/**
 * 导入合并计划。三个数组只包含待新增项；rootOrderIds 是新增后根层应使用的完整顺序。
 */
export interface ConnectionImportPlan {
  connections: ConnectionConfig[];
  folders: ConnectionFolder[];
  proxies: ProxyConfig[];
  rootOrderIds: string[];
  summary: ConnectionImportSummary;
}

/** 可参与代理配置比较的最小字段集合 */
export interface ComparableProxyConfig {
  proxyType: ProxyType;
  host: string;
  port: number;
  username?: string | null;
  password?: string | null;
  hasPassword?: boolean;
}

type UnknownRecord = Record<string, unknown>;

type TreeNode = {
  kind: "folder" | "connection";
  key: string;
  name: string;
  parentKey: string | null;
  order: number | undefined;
};

type OrderedTree = {
  traversal: TreeNode[];
  orderByNode: Map<string, number>;
};

type SnapshotIndexes = {
  connectionById: Map<string, ConnectionConfig>;
  folderById: Map<string, ConnectionFolder>;
  proxyById: Map<string, ProxyConfig>;
};

const ROOT_PARENT = null;
const MAX_ID_GENERATION_ATTEMPTS = 1000;
const AUTH_TYPES: readonly AuthType[] = ["password", "privateKey"];
const PROXY_TYPES: readonly ProxyType[] = ["socks4", "socks4a", "socks5", "http"];
const TUNNEL_TYPES: readonly TunnelType[] = ["local", "remote", "dynamic", "dynamicHttp"];

const FILE_KEYS = new Set(["format", "version", "exportedAt", "folders", "connections", "proxies"]);
const FOLDER_KEYS = new Set(["ref", "name", "parentRef", "order"]);
const CONNECTION_KEYS = new Set([
  "ref",
  "name",
  "host",
  "port",
  "username",
  "authType",
  "password",
  "privateKeyPath",
  "passphrase",
  "proxyRef",
  "remark",
  "tunnels",
  "parentRef",
  "order",
]);
const TUNNEL_KEYS = new Set([
  "name",
  "tunnelType",
  "enabled",
  "localOnly",
  "listenPort",
  "targetHost",
  "targetPort",
]);
const PROXY_KEYS = new Set([
  "ref",
  "name",
  "proxyType",
  "host",
  "port",
  "username",
  "password",
]);

/** 创建带统一前缀的连接数据错误 */
function transferError(message: string): Error {
  return new Error(`连接数据无效：${message}`);
}

/** 将未知值校验为普通对象 */
function readRecord(value: unknown, path: string): UnknownRecord {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw transferError(`[ ${path} ] 必须是对象`);
  }
  return value as UnknownRecord;
}

/** 拒绝契约未声明的对象字段 */
function assertAllowedKeys(record: UnknownRecord, allowed: ReadonlySet<string>, path: string): void {
  for (const key of Object.keys(record)) {
    if (!allowed.has(key)) throw transferError(`[ ${path}.${key} ] 是不支持的字段`);
  }
}

/** 读取字符串字段 */
function readString(record: UnknownRecord, key: string, path: string): string {
  const value = record[key];
  if (typeof value !== "string") throw transferError(`[ ${path}.${key} ] 必须是字符串`);
  return value;
}

/** 读取非空字符串字段 */
function readNonBlankString(record: UnknownRecord, key: string, path: string): string {
  const value = readString(record, key, path);
  if (!value.trim()) throw transferError(`[ ${path}.${key} ] 不能为空`);
  return value;
}

/** 读取可空字符串字段 */
function readNullableString(record: UnknownRecord, key: string, path: string): string | null {
  const value = record[key];
  if (value === null) return null;
  if (typeof value !== "string") throw transferError(`[ ${path}.${key} ] 必须是字符串或 null`);
  return value;
}

/** 读取可空且非空的引用字段 */
function readNullableReference(record: UnknownRecord, key: string, path: string): string | null {
  const value = readNullableString(record, key, path);
  if (value !== null && !value.trim()) throw transferError(`[ ${path}.${key} ] 不能为空字符串`);
  return value;
}

/** 读取布尔字段 */
function readBoolean(record: UnknownRecord, key: string, path: string): boolean {
  const value = record[key];
  if (typeof value !== "boolean") throw transferError(`[ ${path}.${key} ] 必须是布尔值`);
  return value;
}

/** 读取有效端口字段 */
function readPort(record: UnknownRecord, key: string, path: string): number {
  const value = record[key];
  if (typeof value !== "number" || !Number.isInteger(value) || value < 1 || value > 65535) {
    throw transferError(`[ ${path}.${key} ] 必须是 1 到 65535 之间的整数`);
  }
  return value;
}

/** 读取可空端口字段 */
function readNullablePort(record: UnknownRecord, key: string, path: string): number | null {
  if (record[key] === null) return null;
  return readPort(record, key, path);
}

/** 读取非负整数排序字段 */
function readOrder(record: UnknownRecord, key: string, path: string): number {
  const value = record[key];
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0) {
    throw transferError(`[ ${path}.${key} ] 必须是非负整数`);
  }
  return value;
}

/** 读取数组字段 */
function readArray(record: UnknownRecord, key: string, path: string): unknown[] {
  const value = record[key];
  if (!Array.isArray(value)) throw transferError(`[ ${path}.${key} ] 必须是数组`);
  return value;
}

/** 读取限定字符串枚举字段 */
function readEnum<T extends string>(
  record: UnknownRecord,
  key: string,
  path: string,
  values: readonly T[]
): T {
  const value = record[key];
  if (typeof value !== "string" || !values.includes(value as T)) {
    throw transferError(`[ ${path}.${key} ] 不是支持的取值`);
  }
  return value as T;
}

/** 从未知值显式提取一个 V1 文件夹 */
function readExportFolder(value: unknown, index: number): ConnectionExportFolderV1 {
  const path = `folders[${index}]`;
  const record = readRecord(value, path);
  assertAllowedKeys(record, FOLDER_KEYS, path);
  return {
    ref: readNonBlankString(record, "ref", path),
    name: readNonBlankString(record, "name", path),
    parentRef: readNullableReference(record, "parentRef", path),
    order: readOrder(record, "order", path),
  };
}

/** 从未知值显式提取一个 V1 隧道 */
function readExportTunnel(value: unknown, connectionIndex: number, tunnelIndex: number): ConnectionExportTunnelV1 {
  const path = `connections[${connectionIndex}].tunnels[${tunnelIndex}]`;
  const record = readRecord(value, path);
  assertAllowedKeys(record, TUNNEL_KEYS, path);
  const tunnelType = readEnum(record, "tunnelType", path, TUNNEL_TYPES);
  const targetHost = readNullableString(record, "targetHost", path);
  const targetPort = readNullablePort(record, "targetPort", path);
  if ((tunnelType === "local" || tunnelType === "remote") && !targetHost?.trim()) {
    throw transferError(`[ ${path}.targetHost ] 不能为空`);
  }
  if ((tunnelType === "local" || tunnelType === "remote") && targetPort === null) {
    throw transferError(`[ ${path}.targetPort ] 不能为空`);
  }
  return {
    name: readNonBlankString(record, "name", path),
    tunnelType,
    enabled: readBoolean(record, "enabled", path),
    localOnly: readBoolean(record, "localOnly", path),
    listenPort: readPort(record, "listenPort", path),
    targetHost,
    targetPort,
  };
}

/** 从未知值显式提取一个 V1 连接 */
function readExportConnection(value: unknown, index: number): ConnectionExportConnectionV1 {
  const path = `connections[${index}]`;
  const record = readRecord(value, path);
  assertAllowedKeys(record, CONNECTION_KEYS, path);
  return {
    ref: readNonBlankString(record, "ref", path),
    name: readNonBlankString(record, "name", path),
    host: readNonBlankString(record, "host", path),
    port: readPort(record, "port", path),
    username: readString(record, "username", path),
    authType: readEnum(record, "authType", path, AUTH_TYPES),
    password: readNullableString(record, "password", path),
    privateKeyPath: readNullableString(record, "privateKeyPath", path),
    passphrase: readNullableString(record, "passphrase", path),
    proxyRef: readNullableReference(record, "proxyRef", path),
    remark: readNullableString(record, "remark", path),
    tunnels: readArray(record, "tunnels", path).map((tunnel, tunnelIndex) =>
      readExportTunnel(tunnel, index, tunnelIndex)
    ),
    parentRef: readNullableReference(record, "parentRef", path),
    order: readOrder(record, "order", path),
  };
}

/** 从未知值显式提取一个 V1 代理 */
function readExportProxy(value: unknown, index: number): ConnectionExportProxyV1 {
  const path = `proxies[${index}]`;
  const record = readRecord(value, path);
  assertAllowedKeys(record, PROXY_KEYS, path);
  const proxy: ConnectionExportProxyV1 = {
    ref: readNonBlankString(record, "ref", path),
    name: readNonBlankString(record, "name", path),
    proxyType: readEnum(record, "proxyType", path, PROXY_TYPES),
    host: readNonBlankString(record, "host", path),
    port: readPort(record, "port", path),
    username: readNullableString(record, "username", path),
    password: readNullableString(record, "password", path),
  };
  if ((proxy.proxyType === "socks4" || proxy.proxyType === "socks4a") && proxy.password !== null) {
    throw transferError(`[ ${path}.password ] 不能用于 SOCKS4 或 SOCKS4A 代理`);
  }
  if (proxy.password && !proxy.username?.trim()) {
    throw transferError(`[ ${path}.username ] 在代理包含密码时不能为空`);
  }
  return proxy;
}

/** 校验父引用存在且文件夹关系无环 */
function assertValidParentGraph(parentByRef: ReadonlyMap<string, string | null>, label: string): void {
  for (const [ref, parentRef] of parentByRef) {
    if (parentRef !== null && !parentByRef.has(parentRef)) {
      throw transferError(`${label} [ ${ref} ] 引用了不存在的父文件夹 [ ${parentRef} ]`);
    }
  }

  const states = new Map<string, "visiting" | "done">();
  for (const startRef of parentByRef.keys()) {
    if (states.get(startRef) === "done") continue;
    const chain: string[] = [];
    let currentRef: string | null = startRef;
    while (currentRef !== null && states.get(currentRef) !== "done") {
      if (states.get(currentRef) === "visiting") {
        throw transferError(`${label}层级存在循环引用`);
      }
      states.set(currentRef, "visiting");
      chain.push(currentRef);
      currentRef = parentByRef.get(currentRef) ?? null;
    }
    for (const ref of chain) states.set(ref, "done");
  }
}

/** 校验同一父级的混合排序值不存在重复 */
function assertUniqueSiblingOrders(file: ConnectionExportFileV1): void {
  const ordersByParent = new Map<string | null, Set<number>>();
  /** 为父级预留一个混合排序值 */
  const reserveOrder = (parentRef: string | null, order: number): void => {
    const orders = ordersByParent.get(parentRef) ?? new Set<number>();
    if (orders.has(order)) throw transferError("同一层级的连接和文件夹不能使用相同排序值");
    orders.add(order);
    ordersByParent.set(parentRef, orders);
  };
  for (const folder of file.folders) reserveOrder(folder.parentRef, folder.order);
  for (const connection of file.connections) reserveOrder(connection.parentRef, connection.order);
}

/** 校验文件内引用关系和代理使用约束 */
function assertValidExportRelations(file: ConnectionExportFileV1): void {
  const folderParents = new Map<string, string | null>();
  for (const folder of file.folders) {
    if (folderParents.has(folder.ref)) throw transferError(`文件夹引用 [ ${folder.ref} ] 重复`);
    folderParents.set(folder.ref, folder.parentRef);
  }
  assertValidParentGraph(folderParents, "文件夹");

  const proxyByRef = new Map<string, ConnectionExportProxyV1>();
  for (const proxy of file.proxies) {
    if (proxyByRef.has(proxy.ref)) throw transferError(`代理引用 [ ${proxy.ref} ] 重复`);
    proxyByRef.set(proxy.ref, proxy);
  }

  const usedProxyRefs = new Set<string>();
  const connectionRefs = new Set<string>();
  for (const connection of file.connections) {
    if (connectionRefs.has(connection.ref)) throw transferError(`连接引用 [ ${connection.ref} ] 重复`);
    connectionRefs.add(connection.ref);
    if (connection.parentRef !== null && !folderParents.has(connection.parentRef)) {
      throw transferError(`连接 [ ${connection.name} ] 引用了不存在的文件夹 [ ${connection.parentRef} ]`);
    }
    if (connection.proxyRef !== null) {
      if (!proxyByRef.has(connection.proxyRef)) {
        throw transferError(`连接 [ ${connection.name} ] 引用了不存在的代理 [ ${connection.proxyRef} ]`);
      }
      usedProxyRefs.add(connection.proxyRef);
    }
  }
  for (const proxy of file.proxies) {
    if (!usedProxyRefs.has(proxy.ref)) throw transferError(`代理 [ ${proxy.name} ] 未被任何连接使用`);
  }
  assertUniqueSiblingOrders(file);
}

/**
 * 严格校验未知值并返回只含 V1 白名单字段的新对象。
 */
export function validateConnectionExport(value: unknown): ConnectionExportFileV1 {
  const record = readRecord(value, "root");
  assertAllowedKeys(record, FILE_KEYS, "root");
  if (record.format !== CONNECTION_EXPORT_FORMAT) throw transferError("文件格式标识不正确");
  if (record.version !== CONNECTION_EXPORT_VERSION) throw transferError("文件版本不受支持");
  const exportedAt = readString(record, "exportedAt", "root");
  if (Number.isNaN(Date.parse(exportedAt))) throw transferError("导出时间格式不正确");

  const file: ConnectionExportFileV1 = {
    format: CONNECTION_EXPORT_FORMAT,
    version: CONNECTION_EXPORT_VERSION,
    exportedAt,
    folders: readArray(record, "folders", "root").map(readExportFolder),
    connections: readArray(record, "connections", "root").map(readExportConnection),
    proxies: readArray(record, "proxies", "root").map(readExportProxy),
  };
  assertValidExportRelations(file);
  return file;
}

/** 解析 JSON 文本并执行完整契约校验 */
export function parseConnectionExport(json: string): ConnectionExportFileV1 {
  let value: unknown;
  try {
    value = JSON.parse(json) as unknown;
  } catch {
    throw transferError("导入文件不是有效的 JSON");
  }
  return validateConnectionExport(value);
}

/** 将连接导出对象序列化为 UTF-8 友好的格式化 JSON 文本 */
export function serializeConnectionExport(value: unknown): string {
  return `${JSON.stringify(validateConnectionExport(value), null, 2)}\n`;
}

/** 统一连接或文件夹的父级表达 */
function normalizeParentId(parentId: string | null | undefined): string | null {
  return parentId ?? null;
}

/** 校验本地排序值是否可用于连接管理器比较 */
function assertValidLocalOrder(order: number | undefined, label: string): void {
  if (order !== undefined && (!Number.isInteger(order) || order < 0)) {
    throw transferError(`${label}的排序值必须是非负整数`);
  }
}

/** 为本地快照建立索引并校验 id 与文件夹引用 */
function indexSnapshot(snapshot: ConnectionTransferSnapshot): SnapshotIndexes {
  const connectionById = new Map<string, ConnectionConfig>();
  const folderById = new Map<string, ConnectionFolder>();
  const proxyById = new Map<string, ProxyConfig>();

  for (const folder of snapshot.folders) {
    if (!folder.id?.trim()) throw transferError("本地文件夹 id 不能为空");
    if (folderById.has(folder.id)) throw transferError(`本地文件夹 id [ ${folder.id} ] 重复`);
    assertValidLocalOrder(folder.order, `本地文件夹 [ ${folder.name} ]`);
    folderById.set(folder.id, folder);
  }
  for (const connection of snapshot.connections) {
    if (!connection.id?.trim()) throw transferError("本地连接 id 不能为空");
    if (connectionById.has(connection.id) || folderById.has(connection.id)) {
      throw transferError(`本地连接或文件夹 id [ ${connection.id} ] 重复`);
    }
    assertValidLocalOrder(connection.order, `本地连接 [ ${connection.name} ]`);
    connectionById.set(connection.id, connection);
  }
  for (const proxy of snapshot.proxies) {
    if (!proxy.id?.trim()) throw transferError("本地代理 id 不能为空");
    if (proxyById.has(proxy.id)) throw transferError(`本地代理 id [ ${proxy.id} ] 重复`);
    proxyById.set(proxy.id, proxy);
  }

  const folderParents = new Map<string, string | null>();
  for (const folder of snapshot.folders) folderParents.set(folder.id, normalizeParentId(folder.parentId));
  assertValidParentGraph(folderParents, "本地文件夹");
  for (const connection of snapshot.connections) {
    const parentId = normalizeParentId(connection.parentId);
    if (parentId !== null && !folderById.has(parentId)) {
      throw transferError(`本地连接 [ ${connection.name} ] 引用了不存在的文件夹 [ ${parentId} ]`);
    }
  }
  return { connectionById, folderById, proxyById };
}

/** 生成区分节点类型的内部排序键 */
function treeNodeKey(node: Pick<TreeNode, "kind" | "key">): string {
  return `${node.kind}:${node.key}`;
}

/** 按连接管理器当前规则比较同层节点 */
function compareTreeNodes(a: TreeNode, b: TreeNode): number {
  if (a.order !== undefined || b.order !== undefined) {
    const orderDiff = (a.order ?? Number.MAX_SAFE_INTEGER) - (b.order ?? Number.MAX_SAFE_INTEGER);
    if (orderDiff !== 0) return orderDiff;
  }
  if (a.kind !== b.kind) return a.kind === "folder" ? -1 : 1;
  return a.name.localeCompare(b.name, "zh-Hans-CN", { numeric: true, sensitivity: "base" });
}

/** 对扁平节点按树形显示顺序遍历，并为每个同层节点生成连续排序值 */
function orderTreeNodes(nodes: readonly TreeNode[]): OrderedTree {
  const childrenByParent = new Map<string | null, TreeNode[]>();
  for (const node of nodes) {
    const siblings = childrenByParent.get(node.parentKey) ?? [];
    siblings.push(node);
    childrenByParent.set(node.parentKey, siblings);
  }

  const orderByNode = new Map<string, number>();
  for (const siblings of childrenByParent.values()) {
    siblings.sort(compareTreeNodes);
    siblings.forEach((node, index) => orderByNode.set(treeNodeKey(node), index));
  }

  const traversal: TreeNode[] = [];
  const stack = [...(childrenByParent.get(ROOT_PARENT) ?? [])].reverse();
  while (stack.length > 0) {
    const node = stack.pop();
    if (!node) break;
    traversal.push(node);
    if (node.kind === "folder") {
      const children = childrenByParent.get(node.key) ?? [];
      for (let index = children.length - 1; index >= 0; index -= 1) stack.push(children[index]);
    }
  }
  if (traversal.length !== nodes.length) throw transferError("树形数据包含无法到达根层的项目");
  return { traversal, orderByNode };
}

/** 收集指定文件夹及其全部后代 */
function collectFolderSubtree(rootId: string, folders: readonly ConnectionFolder[]): Set<string> {
  const ids = new Set<string>([rootId]);
  let changed = true;
  while (changed) {
    changed = false;
    for (const folder of folders) {
      if (folder.parentId && ids.has(folder.parentId) && !ids.has(folder.id)) {
        ids.add(folder.id);
        changed = true;
      }
    }
  }
  return ids;
}

/** 将内部隧道显式映射为 V1 白名单字段 */
function exportTunnel(tunnel: TunnelConfig): ConnectionExportTunnelV1 {
  return {
    name: tunnel.name,
    tunnelType: tunnel.tunnelType,
    enabled: tunnel.enabled,
    localOnly: tunnel.localOnly,
    listenPort: tunnel.listenPort,
    targetHost: tunnel.targetHost ?? null,
    targetPort: tunnel.targetPort ?? null,
  };
}

/** 将内部连接显式映射为 V1 白名单字段 */
function exportConnection(
  connection: ConnectionConfig,
  parentRef: string | null,
  order: number
): ConnectionExportConnectionV1 {
  return {
    ref: connection.id,
    name: connection.name,
    host: connection.host,
    port: connection.port,
    username: connection.username,
    authType: connection.authType,
    password: null,
    privateKeyPath: connection.privateKeyPath ?? null,
    passphrase: null,
    proxyRef: connection.proxyId ?? null,
    remark: connection.remark ?? null,
    tunnels: (connection.tunnels ?? []).map(exportTunnel),
    parentRef,
    order,
  };
}

/** 将内部代理显式映射为 V1 白名单字段 */
function exportProxy(proxy: ProxyConfig): ConnectionExportProxyV1 {
  return {
    ref: proxy.id,
    name: proxy.name,
    proxyType: proxy.proxyType,
    host: proxy.host,
    port: proxy.port,
    username: proxy.username ?? null,
    password: null,
  };
}

/**
 * 根据指定范围构建 V1 导出对象。私钥路径仅作为字符串保留，不读取私钥文件内容。
 */
export function buildConnectionExport(
  scope: ConnectionExportScope,
  snapshot: ConnectionTransferSnapshot,
  options: ConnectionExportOptions = {}
): ConnectionExportFileV1 {
  const indexes = indexSnapshot(snapshot);
  let includedFolderIds: Set<string>;
  let includedConnections: ConnectionConfig[];
  let exportedFolderRootId: string | null = null;

  switch (scope.kind) {
    case "all":
      includedFolderIds = new Set(snapshot.folders.map((folder) => folder.id));
      includedConnections = [...snapshot.connections];
      break;
    case "folder": {
      if (!indexes.folderById.has(scope.id)) throw transferError(`找不到待导出的文件夹 [ ${scope.id} ]`);
      includedFolderIds = collectFolderSubtree(scope.id, snapshot.folders);
      includedConnections = snapshot.connections.filter((connection) => {
        const parentId = normalizeParentId(connection.parentId);
        return parentId !== null && includedFolderIds.has(parentId);
      });
      exportedFolderRootId = scope.id;
      break;
    }
    case "connection": {
      const connection = indexes.connectionById.get(scope.id);
      if (!connection) throw transferError(`找不到待导出的连接 [ ${scope.id} ]`);
      includedFolderIds = new Set();
      includedConnections = [connection];
      break;
    }
  }

  const nodes: TreeNode[] = [];
  for (const folder of snapshot.folders) {
    if (!includedFolderIds.has(folder.id)) continue;
    nodes.push({
      kind: "folder",
      key: folder.id,
      name: folder.name,
      parentKey: folder.id === exportedFolderRootId ? null : normalizeParentId(folder.parentId),
      order: folder.id === exportedFolderRootId ? undefined : folder.order,
    });
  }
  for (const connection of includedConnections) {
    nodes.push({
      kind: "connection",
      key: connection.id,
      name: connection.name,
      parentKey: scope.kind === "connection" ? null : normalizeParentId(connection.parentId),
      order: scope.kind === "connection" ? undefined : connection.order,
    });
  }

  const ordered = orderTreeNodes(nodes);
  const folders: ConnectionExportFolderV1[] = [];
  const connections: ConnectionExportConnectionV1[] = [];
  for (const node of ordered.traversal) {
    const order = ordered.orderByNode.get(treeNodeKey(node));
    if (order === undefined) throw transferError("无法确定导出项目的排序值");
    if (node.kind === "folder") {
      const folder = indexes.folderById.get(node.key);
      if (!folder) throw transferError(`找不到待导出的文件夹 [ ${node.key} ]`);
      folders.push({ ref: folder.id, name: folder.name, parentRef: node.parentKey, order });
    } else {
      const connection = indexes.connectionById.get(node.key);
      if (!connection) throw transferError(`找不到待导出的连接 [ ${node.key} ]`);
      connections.push(exportConnection(connection, node.parentKey, order));
    }
  }

  const proxies: ConnectionExportProxyV1[] = [];
  const usedProxyIds = new Set<string>();
  for (const connection of connections) {
    if (connection.proxyRef === null || usedProxyIds.has(connection.proxyRef)) continue;
    const proxy = indexes.proxyById.get(connection.proxyRef);
    if (!proxy) {
      throw transferError(`连接 [ ${connection.name} ] 引用了不存在的代理 [ ${connection.proxyRef} ]`);
    }
    usedProxyIds.add(connection.proxyRef);
    proxies.push(exportProxy(proxy));
  }

  const exportedDate = options.now?.() ?? new Date();
  if (Number.isNaN(exportedDate.getTime())) throw transferError("导出时间无效");
  const exportedAt = exportedDate.toISOString();
  return validateConnectionExport({
    format: CONNECTION_EXPORT_FORMAT,
    version: CONNECTION_EXPORT_VERSION,
    exportedAt,
    folders,
    connections,
    proxies,
  });
}

/** 创建只比较有效代理配置且忽略 id 与名称的稳定指纹 */
export function createProxyConfigFingerprint(proxy: ComparableProxyConfig): string {
  const fields: Array<string | number> = [
    proxy.proxyType,
    proxy.host.trim().toLowerCase(),
    proxy.port,
    proxy.username?.trim() ?? "",
  ];
  if (proxy.proxyType === "socks5" || proxy.proxyType === "http") fields.push(proxy.password ?? "");
  return JSON.stringify(fields);
}

/** 查找同层可用连接名称，冲突时依次追加 (2)、(3) */
export function findAvailableConnectionName(baseName: string, usedNames: ReadonlySet<string>): string {
  if (!usedNames.has(baseName)) return baseName;
  for (let sequence = 2; sequence < Number.MAX_SAFE_INTEGER; sequence += 1) {
    const candidate = `${baseName} (${sequence})`;
    if (!usedNames.has(candidate)) return candidate;
  }
  throw transferError(`无法为连接 [ ${baseName} ] 生成可用名称`);
}

/** 从 id 工厂获取一个不与现有数据冲突的新 id */
function nextUniqueId(idFactory: () => string, reservedIds: Set<string>): string {
  for (let attempt = 0; attempt < MAX_ID_GENERATION_ATTEMPTS; attempt += 1) {
    const id = idFactory();
    if (typeof id !== "string" || !id.trim()) throw transferError("id 生成器返回了空值");
    if (reservedIds.has(id)) continue;
    reservedIds.add(id);
    return id;
  }
  throw transferError("多次生成 id 均发生冲突");
}

/** 收集快照内全部已占用 id */
function collectReservedIds(snapshot: ConnectionTransferSnapshot): Set<string> {
  const ids = new Set<string>();
  for (const folder of snapshot.folders) ids.add(folder.id);
  for (const connection of snapshot.connections) {
    ids.add(connection.id);
    for (const tunnel of connection.tunnels ?? []) ids.add(tunnel.id);
  }
  for (const proxy of snapshot.proxies) ids.add(proxy.id);
  return ids;
}

/** 将 V1 文件转换为通用树节点 */
function exportFileTreeNodes(file: ConnectionExportFileV1): TreeNode[] {
  const nodes: TreeNode[] = file.folders.map((folder) => ({
    kind: "folder",
    key: folder.ref,
    name: folder.name,
    parentKey: folder.parentRef,
    order: folder.order,
  }));
  for (const connection of file.connections) {
    nodes.push({
      kind: "connection",
      key: connection.ref,
      name: connection.name,
      parentKey: connection.parentRef,
      order: connection.order,
    });
  }
  return nodes;
}

/** 判断本地代理是否具备可安全比较的完整配置 */
function canFingerprintLocalProxy(proxy: ProxyConfig): boolean {
  return !(
    (proxy.proxyType === "socks5" || proxy.proxyType === "http") &&
    proxy.hasPassword === true &&
    proxy.password === undefined
  );
}

/** 将 V1 代理转换为内部新增代理 */
function importProxy(proxy: ConnectionExportProxyV1, id: string): ProxyConfig {
  return {
    id,
    name: proxy.name,
    proxyType: proxy.proxyType,
    host: proxy.host,
    port: proxy.port,
    username: proxy.username ?? undefined,
    password:
      proxy.proxyType === "socks5" || proxy.proxyType === "http"
        ? proxy.password ?? undefined
        : undefined,
  };
}

/** 将 V1 隧道转换为内部新增隧道 */
function importTunnel(
  tunnel: ConnectionExportTunnelV1,
  idFactory: () => string,
  reservedIds: Set<string>
): TunnelConfig {
  return {
    id: nextUniqueId(idFactory, reservedIds),
    name: tunnel.name,
    tunnelType: tunnel.tunnelType,
    enabled: tunnel.enabled,
    localOnly: tunnel.localOnly,
    listenPort: tunnel.listenPort,
    targetHost: tunnel.targetHost ?? undefined,
    targetPort: tunnel.targetPort ?? undefined,
  };
}

/** 获取当前根层按连接管理器规则显示的节点 */
function currentRootNodes(snapshot: ConnectionTransferSnapshot): TreeNode[] {
  const nodes: TreeNode[] = [];
  for (const folder of snapshot.folders) {
    if (normalizeParentId(folder.parentId) === null) {
      nodes.push({ kind: "folder", key: folder.id, name: folder.name, parentKey: null, order: folder.order });
    }
  }
  for (const connection of snapshot.connections) {
    if (normalizeParentId(connection.parentId) === null) {
      nodes.push({
        kind: "connection",
        key: connection.id,
        name: connection.name,
        parentKey: null,
        order: connection.order,
      });
    }
  }
  return nodes.sort(compareTreeNodes);
}

/** 为每个现有父级建立连接名称占用集合 */
function existingNamesByParent(snapshot: ConnectionTransferSnapshot): Map<string | null, Set<string>> {
  const namesByParent = new Map<string | null, Set<string>>();
  for (const connection of snapshot.connections) {
    const parentId = normalizeParentId(connection.parentId);
    const names = namesByParent.get(parentId) ?? new Set<string>();
    names.add(connection.name);
    namesByParent.set(parentId, names);
  }
  return namesByParent;
}

/** 为每个现有父级建立精确名称到文件夹标识的稳定映射 */
function existingFolderIdsByParentAndName(
  snapshot: ConnectionTransferSnapshot
): Map<string | null, Map<string, string>> {
  const idsByParentAndName = new Map<string | null, Map<string, string>>();
  for (const folder of snapshot.folders) {
    const parentId = normalizeParentId(folder.parentId);
    const idsByName = idsByParentAndName.get(parentId) ?? new Map<string, string>();
    if (!idsByName.has(folder.name)) idsByName.set(folder.name, folder.id);
    idsByParentAndName.set(parentId, idsByName);
  }
  return idsByParentAndName;
}

/** 创建按父级向现有同层内容末尾分配排序值的函数 */
function createImportOrderAllocator(
  snapshot: ConnectionTransferSnapshot
): (parentId: string) => number {
  const nextOrderByParent = new Map<string, number>();
  /** 将现有同层内容纳入下一个排序值计算 */
  const includeItem = (item: ConnectionFolder | ConnectionConfig) => {
    const parentId = normalizeParentId(item.parentId);
    if (parentId === null || typeof item.order !== "number") return;
    nextOrderByParent.set(parentId, Math.max(nextOrderByParent.get(parentId) ?? 0, item.order + 1));
  };
  snapshot.folders.forEach(includeItem);
  snapshot.connections.forEach(includeItem);
  return (parentId: string) => {
    const order = nextOrderByParent.get(parentId) ?? 0;
    nextOrderByParent.set(parentId, order + 1);
    return order;
  };
}

/**
 * 完整校验导入对象并生成无副作用合并计划；同名文件夹复用，其他文件根项目追加到本地根层。
 */
export function planConnectionImport(
  value: unknown,
  snapshot: ConnectionTransferSnapshot,
  options: ConnectionImportOptions
): ConnectionImportPlan {
  const file = validateConnectionExport(value);
  const indexes = indexSnapshot(snapshot);
  const reservedIds = collectReservedIds(snapshot);
  for (const folder of file.folders) reservedIds.add(folder.ref);
  for (const connection of file.connections) reservedIds.add(connection.ref);
  for (const proxy of file.proxies) reservedIds.add(proxy.ref);
  const fileTree = orderTreeNodes(exportFileTreeNodes(file));

  const folderByRef = new Map(file.folders.map((folder) => [folder.ref, folder]));
  const folderIdByRef = new Map<string, string>();
  const newFolderRefs = new Set<string>();
  const folderIdsByParentAndName = existingFolderIdsByParentAndName(snapshot);
  let reusedFolderCount = 0;
  for (const node of fileTree.traversal) {
    if (node.kind !== "folder") continue;
    const sourceFolder = folderByRef.get(node.key);
    if (!sourceFolder) throw transferError(`找不到文件夹引用 [ ${node.key} ]`);
    const parentId = sourceFolder.parentRef === null ? null : folderIdByRef.get(sourceFolder.parentRef);
    if (parentId === undefined) throw transferError(`找不到父文件夹引用 [ ${sourceFolder.parentRef} ]`);

    // 同一应用导出的根文件夹可能原本位于更深层级，来源标识一致时优先回到原文件夹。
    const originalFolder = indexes.folderById.get(sourceFolder.ref);
    const originalFolderId =
      originalFolder &&
      originalFolder.name === sourceFolder.name &&
      (sourceFolder.parentRef === null || normalizeParentId(originalFolder.parentId) === parentId)
        ? originalFolder.id
        : undefined;
    const reusableId =
      originalFolderId ?? folderIdsByParentAndName.get(parentId)?.get(sourceFolder.name);
    if (reusableId) {
      folderIdByRef.set(sourceFolder.ref, reusableId);
      reusedFolderCount += 1;
      continue;
    }

    const id = nextUniqueId(options.idFactory, reservedIds);
    folderIdByRef.set(sourceFolder.ref, id);
    newFolderRefs.add(sourceFolder.ref);
    const idsByName = folderIdsByParentAndName.get(parentId) ?? new Map<string, string>();
    idsByName.set(sourceFolder.name, id);
    folderIdsByParentAndName.set(parentId, idsByName);
  }

  const connectionByRef = new Map(file.connections.map((connection) => [connection.ref, connection]));
  const proxyByRef = new Map(file.proxies.map((proxy) => [proxy.ref, proxy]));
  const localProxyIdByFingerprint = new Map<string, string>();
  for (const proxy of snapshot.proxies) {
    if (!canFingerprintLocalProxy(proxy)) continue;
    const fingerprint = createProxyConfigFingerprint(proxy);
    if (!localProxyIdByFingerprint.has(fingerprint)) localProxyIdByFingerprint.set(fingerprint, proxy.id);
  }

  const proxyIdByRef = new Map<string, string>();
  const addedProxies: ProxyConfig[] = [];
  let reusedProxyCount = 0;
  for (const sourceProxy of file.proxies) {
    const matchedProxyId = options.matchedProxyIds?.get(sourceProxy.ref);
    if (matchedProxyId === undefined) continue;
    if (!indexes.proxyById.has(matchedProxyId)) {
      throw transferError(
        `代理引用 [ ${sourceProxy.ref} ] 匹配了不存在的本地代理 [ ${matchedProxyId} ]`
      );
    }
    proxyIdByRef.set(sourceProxy.ref, matchedProxyId);
    localProxyIdByFingerprint.set(createProxyConfigFingerprint(sourceProxy), matchedProxyId);
    reusedProxyCount += 1;
  }
  for (const node of fileTree.traversal) {
    if (node.kind !== "connection") continue;
    const connection = connectionByRef.get(node.key);
    if (!connection) throw transferError(`找不到连接引用 [ ${node.key} ]`);
    const proxyRef = connection.proxyRef;
    if (proxyRef === null || proxyIdByRef.has(proxyRef)) continue;
    const sourceProxy = proxyByRef.get(proxyRef);
    if (!sourceProxy) throw transferError(`找不到代理引用 [ ${proxyRef} ]`);
    const fingerprint = createProxyConfigFingerprint(sourceProxy);
    const reusableId = localProxyIdByFingerprint.get(fingerprint);
    if (reusableId) {
      proxyIdByRef.set(proxyRef, reusableId);
      reusedProxyCount += 1;
      continue;
    }
    const id = nextUniqueId(options.idFactory, reservedIds);
    proxyIdByRef.set(proxyRef, id);
    localProxyIdByFingerprint.set(fingerprint, id);
    addedProxies.push(importProxy(sourceProxy, id));
  }

  const existingRoots = currentRootNodes(snapshot);
  const rootOrderIds = existingRoots.map((node) => node.key);
  const rootOrderBase = rootOrderIds.length;
  const importedFolders: ConnectionFolder[] = [];
  const importedConnections: ConnectionConfig[] = [];
  const namesByParent = existingNamesByParent(snapshot);
  const allocateImportOrder = createImportOrderAllocator(snapshot);
  let renamedConnectionCount = 0;

  for (const node of fileTree.traversal) {
    const sourceOrder = fileTree.orderByNode.get(treeNodeKey(node));
    if (sourceOrder === undefined) throw transferError("无法确定导入项目的排序值");
    if (node.kind === "folder") {
      const sourceFolder = folderByRef.get(node.key);
      const id = folderIdByRef.get(node.key);
      if (!sourceFolder || !id) throw transferError(`找不到文件夹引用 [ ${node.key} ]`);
      const parentId = sourceFolder.parentRef === null ? null : folderIdByRef.get(sourceFolder.parentRef);
      if (parentId === undefined) throw transferError(`找不到父文件夹引用 [ ${sourceFolder.parentRef} ]`);
      if (!newFolderRefs.has(node.key)) continue;
      const targetOrder = parentId === null ? rootOrderBase + sourceOrder : allocateImportOrder(parentId);
      importedFolders.push({ id, name: sourceFolder.name, parentId, order: targetOrder });
      if (sourceFolder.parentRef === null) rootOrderIds.push(id);
      continue;
    }

    const sourceConnection = connectionByRef.get(node.key);
    if (!sourceConnection) throw transferError(`找不到连接引用 [ ${node.key} ]`);
    const parentId = sourceConnection.parentRef === null ? null : folderIdByRef.get(sourceConnection.parentRef);
    if (parentId === undefined) throw transferError(`找不到父文件夹引用 [ ${sourceConnection.parentRef} ]`);
    const targetOrder = parentId === null ? rootOrderBase + sourceOrder : allocateImportOrder(parentId);
    const usedNames = namesByParent.get(parentId) ?? new Set<string>();
    const name = findAvailableConnectionName(sourceConnection.name, usedNames);
    usedNames.add(name);
    namesByParent.set(parentId, usedNames);
    if (name !== sourceConnection.name) renamedConnectionCount += 1;
    const id = nextUniqueId(options.idFactory, reservedIds);
    const proxyId =
      sourceConnection.proxyRef === null ? null : proxyIdByRef.get(sourceConnection.proxyRef);
    if (proxyId === undefined) {
      throw transferError(`找不到代理引用 [ ${sourceConnection.proxyRef} ] 的导入映射`);
    }
    importedConnections.push({
      id,
      name,
      host: sourceConnection.host,
      port: sourceConnection.port,
      username: sourceConnection.username,
      authType: sourceConnection.authType,
      password:
        sourceConnection.authType === "password"
          ? sourceConnection.password ?? undefined
          : undefined,
      privateKeyPath: sourceConnection.privateKeyPath ?? undefined,
      passphrase:
        sourceConnection.authType === "privateKey"
          ? sourceConnection.passphrase ?? undefined
          : undefined,
      proxyId,
      remark: sourceConnection.remark ?? undefined,
      tunnels: sourceConnection.tunnels.map((tunnel) => importTunnel(tunnel, options.idFactory, reservedIds)),
      parentId,
      order: targetOrder,
    });
    if (sourceConnection.parentRef === null) rootOrderIds.push(id);
  }

  return {
    connections: importedConnections,
    folders: importedFolders,
    proxies: addedProxies,
    rootOrderIds,
    summary: {
      connectionCount: importedConnections.length,
      folderCount: importedFolders.length,
      reusedFolderCount,
      addedProxyCount: addedProxies.length,
      reusedProxyCount,
      renamedConnectionCount,
      privateKeyConnectionCount: importedConnections.filter(
        (connection) => connection.authType === "privateKey"
      ).length,
    },
  };
}
