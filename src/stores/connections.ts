/**
 * 连接配置 store：管理已保存的连接与分组文件夹，并通过 tauri-plugin-store 持久化到本地
 */

import { defineStore } from "pinia";
import { ref } from "vue";
import { load, type Store } from "@tauri-apps/plugin-store";
import {
  credentialsCheckMany,
  credentialsCopyMany,
  credentialsDeleteMany,
  credentialsSetMany,
} from "../api";
import type {
  ConnectionConfig,
  ConnectionFolder,
  ConnectionSecretChanges,
  CredentialKey,
  CredentialWrite,
  SecretChange,
} from "../types";
import { genId } from "../utils";

/** 持久化文件名与键名 */
const STORE_FILE = "connections.json";
const STORE_KEY = "connections";
const FOLDER_KEY = "folders";
const EXPANDED_FOLDERS_KEY = "expandedFolderIds";

export const useConnectionsStore = defineStore("connections", () => {
  /** 已保存的连接列表 */
  const connections = ref<ConnectionConfig[]>([]);
  /** 已保存的分组文件夹列表 */
  const folders = ref<ConnectionFolder[]>([]);
  /** 连接管理器中已展开的文件夹标识 */
  const expandedFolderIds = ref<string[]>([]);

  let store: Store | null = null;
  /** 串行持久化队列，单次失败会被队列吸收但仍返回给原调用方 */
  let persistQueue: Promise<void> = Promise.resolve();

  /** 从本地加载连接与文件夹列表 */
  async function init() {
    store = await load(STORE_FILE, { defaults: {}, autoSave: false });
    const savedConns = await store.get<ConnectionConfig[]>(STORE_KEY);
    const savedFolders = await store.get<ConnectionFolder[]>(FOLDER_KEY);
    connections.value = savedConns ?? [];
    folders.value = savedFolders ?? [];
    expandedFolderIds.value = (await store.get<string[]>(EXPANDED_FOLDERS_KEY)) ?? [];
    await migrateLegacyCredentials();
    await refreshCredentialAvailability();
    const folderIds = new Set(folders.value.map((folder) => folder.id));
    const validExpandedIds = expandedFolderIds.value.filter((id) => folderIds.has(id));
    if (validExpandedIds.length !== expandedFolderIds.value.length) {
      expandedFolderIds.value = validExpandedIds;
      await persist();
    }
  }

  /** 将当前连接与文件夹列表写回本地 */
  async function persist() {
    const currentStore = store;
    if (!currentStore) return;
    const connectionSnapshot = connections.value.map((connection) => ({
      ...stripConnectionSecrets(connection),
      tunnels: connection.tunnels?.map((tunnel) => ({ ...tunnel })),
    }));
    const folderSnapshot = folders.value.map((folder) => ({ ...folder }));
    const expandedFolderSnapshot = [...expandedFolderIds.value];
    const operation = persistQueue.then(async () => {
      await currentStore.set(STORE_KEY, connectionSnapshot);
      await currentStore.set(FOLDER_KEY, folderSnapshot);
      await currentStore.set(EXPANDED_FOLDERS_KEY, expandedFolderSnapshot);
      await currentStore.save();
    });
    persistQueue = operation.catch(() => undefined);
    await operation;
  }

  /** 从连接配置中移除仅允许短暂存在于内存中的凭据明文 */
  function stripConnectionSecrets(config: ConnectionConfig): ConnectionConfig {
    const sanitized = { ...config };
    delete sanitized.password;
    delete sanitized.passphrase;
    return sanitized;
  }

  /** 将未知异常转换为可展示的错误文本 */
  function errorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  /** 尽力删除批量导入期间可能写入的系统凭据，返回清理失败信息 */
  async function cleanupImportedCredentials(keys: CredentialKey[]): Promise<string | null> {
    if (keys.length === 0) return null;
    try {
      await credentialsDeleteMany(keys);
      return null;
    } catch (error) {
      return `清理本批系统凭据失败：${errorMessage(error)}`;
    }
  }

  /** 校验批量导入只能追加新项目，且全部父文件夹引用有效、无循环 */
  function validateImportItems(
    importedConnections: ConnectionConfig[],
    importedFolders: ConnectionFolder[]
  ): void {
    const existingItemIds = new Set([
      ...connections.value.map((connection) => connection.id),
      ...folders.value.map((folder) => folder.id),
    ]);
    const importedItemIds = new Set<string>();
    for (const folder of importedFolders) {
      if (!folder.id.trim()) throw new Error("导入文件夹标识不能为空");
      if (existingItemIds.has(folder.id) || importedItemIds.has(folder.id)) {
        throw new Error(`导入文件夹标识 [ ${folder.id} ] 已存在`);
      }
      importedItemIds.add(folder.id);
    }
    for (const connection of importedConnections) {
      if (!connection.id.trim()) throw new Error("导入连接标识不能为空");
      if (existingItemIds.has(connection.id) || importedItemIds.has(connection.id)) {
        throw new Error(`导入连接标识 [ ${connection.id} ] 已存在`);
      }
      importedItemIds.add(connection.id);
    }

    const availableFolderIds = new Set([
      ...folders.value.map((folder) => folder.id),
      ...importedFolders.map((folder) => folder.id),
    ]);
    const importedParentById = new Map(
      importedFolders.map((folder) => [folder.id, normalizeParentId(folder.parentId)])
    );
    for (const folder of importedFolders) {
      const parentId = normalizeParentId(folder.parentId);
      if (parentId !== null && !availableFolderIds.has(parentId)) {
        throw new Error(`导入文件夹 [ ${folder.name} ] 引用了不存在的父文件夹`);
      }
      const visited = new Set<string>([folder.id]);
      let currentParentId = parentId;
      while (currentParentId !== null && importedParentById.has(currentParentId)) {
        if (visited.has(currentParentId)) throw new Error("导入文件夹层级存在循环引用");
        visited.add(currentParentId);
        currentParentId = importedParentById.get(currentParentId) ?? null;
      }
    }
    for (const connection of importedConnections) {
      const parentId = normalizeParentId(connection.parentId);
      if (parentId !== null && !availableFolderIds.has(parentId)) {
        throw new Error(`导入连接 [ ${connection.name} ] 引用了不存在的父文件夹`);
      }
    }
  }

  /** 校验完整根层顺序并生成从零开始的致密排序映射 */
  function buildDenseRootOrder(
    nextConnections: ConnectionConfig[],
    nextFolders: ConnectionFolder[],
    rootOrderIds: string[]
  ): Map<string, number> {
    const rootIds = new Set<string>();
    for (const folder of nextFolders) {
      if (normalizeParentId(folder.parentId) === null) rootIds.add(folder.id);
    }
    for (const connection of nextConnections) {
      if (normalizeParentId(connection.parentId) === null) rootIds.add(connection.id);
    }
    const seen = new Set<string>();
    for (const id of rootOrderIds) {
      if (!rootIds.has(id)) throw new Error(`根层排序包含不存在或非根层的项目 [ ${id} ]`);
      if (seen.has(id)) throw new Error(`根层排序包含重复项目 [ ${id} ]`);
      seen.add(id);
    }
    if (seen.size !== rootIds.size) throw new Error("根层排序未覆盖全部连接和文件夹");
    return new Map(rootOrderIds.map((id, index) => [id, index]));
  }

  /** 将编辑器的凭据修改转换为系统凭据库写入或删除操作 */
  function collectSecretChange(
    id: string,
    kind: CredentialKey["kind"],
    change: SecretChange,
    writes: CredentialWrite[],
    deletes: CredentialKey[]
  ): boolean | undefined {
    if (change.mode === "keep") return undefined;
    if (change.mode === "set" && change.value) {
      writes.push({ kind, id, value: change.value });
      return true;
    }
    deletes.push({ kind, id });
    return false;
  }

  /** 将旧版 JSON 中的明文凭据迁移到系统凭据库，成功后再清除明文 */
  async function migrateLegacyCredentials() {
    const writes: CredentialWrite[] = [];
    const deletes: CredentialKey[] = [];
    const legacyPasswordIds = new Set<string>();
    const legacyPassphraseIds = new Set<string>();
    for (const connection of connections.value) {
      if (typeof connection.password === "string") {
        legacyPasswordIds.add(connection.id);
        if (connection.password) {
          writes.push({ kind: "connectionPassword", id: connection.id, value: connection.password });
        } else deletes.push({ kind: "connectionPassword", id: connection.id });
      }
      if (typeof connection.passphrase === "string") {
        legacyPassphraseIds.add(connection.id);
        if (connection.passphrase) {
          writes.push({ kind: "connectionPassphrase", id: connection.id, value: connection.passphrase });
        } else deletes.push({ kind: "connectionPassphrase", id: connection.id });
      }
    }
    if (writes.length > 0) await credentialsSetMany(writes);
    if (deletes.length > 0) await credentialsDeleteMany(deletes);
    if (legacyPasswordIds.size === 0 && legacyPassphraseIds.size === 0) return;
    for (const connection of connections.value) {
      if (legacyPasswordIds.has(connection.id)) {
        connection.hasPassword = Boolean(connection.password);
        delete connection.password;
      }
      if (legacyPassphraseIds.has(connection.id)) {
        connection.hasPassphrase = Boolean(connection.passphrase);
        delete connection.passphrase;
      }
    }
    await persist();
  }

  /** 检查系统凭据库中的凭据是否仍可用，并清理复制数据后失效的标记 */
  async function refreshCredentialAvailability() {
    const keys: CredentialKey[] = [];
    for (const connection of connections.value) {
      if (connection.hasPassword) keys.push({ kind: "connectionPassword", id: connection.id });
      if (connection.hasPassphrase) keys.push({ kind: "connectionPassphrase", id: connection.id });
    }
    if (keys.length === 0) return;
    const available = await credentialsCheckMany(keys);
    let changed = false;
    keys.forEach((key, index) => {
      if (available[index]) return;
      const connection = connections.value.find((item) => item.id === key.id);
      if (!connection) return;
      if (key.kind === "connectionPassword") connection.hasPassword = false;
      else connection.hasPassphrase = false;
      changed = true;
    });
    if (changed) await persist();
  }

  /** 统一父级 id 表达 */
  function normalizeParentId(parentId: string | null | undefined): string | null {
    return parentId ?? null;
  }

  /** 判断项目是否属于指定父级 */
  function belongsToParent(item: { parentId?: string | null }, parentId: string | null): boolean {
    return normalizeParentId(item.parentId) === parentId;
  }

  /** 计算同级末尾排序值 */
  function nextOrder(parentId: string | null): number {
    let maxOrder = -1;
    for (const folder of folders.value) {
      if (belongsToParent(folder, parentId) && typeof folder.order === "number") {
        maxOrder = Math.max(maxOrder, folder.order);
      }
    }
    for (const conn of connections.value) {
      if (belongsToParent(conn, parentId) && typeof conn.order === "number") {
        maxOrder = Math.max(maxOrder, conn.order);
      }
    }
    return maxOrder + 1;
  }

  /** 设置连接或文件夹的排序值 */
  function setItemOrder(id: string, order: number): boolean {
    const folder = folders.value.find((f) => f.id === id);
    if (folder) {
      if (folder.order === order) return false;
      folder.order = order;
      return true;
    }
    const conn = connections.value.find((c) => c.id === id);
    if (!conn || conn.order === order) return false;
    conn.order = order;
    return true;
  }

  /** 新增或更新连接及其系统凭据，返回连接标识 */
  async function upsert(
    config: ConnectionConfig,
    secretChanges?: ConnectionSecretChanges
  ): Promise<string> {
    if (!config.id) config.id = genId();
    const idx = connections.value.findIndex((c) => c.id === config.id);
    const current = idx >= 0 ? connections.value[idx] : undefined;
    const inferredChanges: ConnectionSecretChanges = secretChanges ?? {
      password:
        typeof config.password === "string"
          ? config.password
            ? { mode: "set", value: config.password }
            : { mode: "clear" }
          : { mode: "keep" },
      passphrase:
        typeof config.passphrase === "string"
          ? config.passphrase
            ? { mode: "set", value: config.passphrase }
            : { mode: "clear" }
          : { mode: "keep" },
    };
    const writes: CredentialWrite[] = [];
    const deletes: CredentialKey[] = [];
    const passwordAvailable = collectSecretChange(
      config.id,
      "connectionPassword",
      inferredChanges.password,
      writes,
      deletes
    );
    const passphraseAvailable = collectSecretChange(
      config.id,
      "connectionPassphrase",
      inferredChanges.passphrase,
      writes,
      deletes
    );
    if (writes.length > 0) await credentialsSetMany(writes);
    if (deletes.length > 0) await credentialsDeleteMany(deletes);

    const sanitized = stripConnectionSecrets({
      ...config,
      hasPassword: passwordAvailable ?? current?.hasPassword ?? config.hasPassword ?? false,
      hasPassphrase: passphraseAvailable ?? current?.hasPassphrase ?? config.hasPassphrase ?? false,
    });
    const parentId = normalizeParentId(config.parentId);
    sanitized.parentId = parentId;
    if (idx >= 0) {
      const parentChanged = normalizeParentId(current?.parentId) !== parentId;
      if (parentChanged) sanitized.order = nextOrder(parentId);
      else sanitized.order = sanitized.order ?? current?.order;
      connections.value[idx] = sanitized;
    } else {
      sanitized.order = sanitized.order ?? nextOrder(parentId);
      connections.value.push(sanitized);
    }
    await persist();
    return sanitized.id;
  }

  /**
   * 原子追加一批导入连接和文件夹，并按完整根层标识顺序致密化排序。
   * 系统凭据先于连接记录写入；任一步失败都会恢复原数组并尽力清理本批凭据。
   */
  async function importBatch(
    importedConnections: ConnectionConfig[],
    importedFolders: ConnectionFolder[],
    rootOrderIds: string[]
  ): Promise<void> {
    if (!store) throw new Error("连接存储尚未初始化，无法导入连接");
    validateImportItems(importedConnections, importedFolders);

    const credentialWrites: CredentialWrite[] = [];
    const credentialKeys: CredentialKey[] = [];
    const sanitizedConnections = importedConnections.map((connection) => {
      const hasPassword = typeof connection.password === "string" && connection.password.length > 0;
      const hasPassphrase = typeof connection.passphrase === "string" && connection.passphrase.length > 0;
      if (hasPassword) {
        credentialWrites.push({
          kind: "connectionPassword",
          id: connection.id,
          value: connection.password as string,
        });
        credentialKeys.push({ kind: "connectionPassword", id: connection.id });
      }
      if (hasPassphrase) {
        credentialWrites.push({
          kind: "connectionPassphrase",
          id: connection.id,
          value: connection.passphrase as string,
        });
        credentialKeys.push({ kind: "connectionPassphrase", id: connection.id });
      }
      return stripConnectionSecrets({
        ...connection,
        tunnels: connection.tunnels?.map((tunnel) => ({ ...tunnel })),
        parentId: normalizeParentId(connection.parentId),
        hasPassword,
        hasPassphrase,
      });
    });
    const sanitizedFolders = importedFolders.map((folder) => ({
      ...folder,
      parentId: normalizeParentId(folder.parentId),
    }));
    const appendedConnections = [...connections.value, ...sanitizedConnections];
    const appendedFolders = [...folders.value, ...sanitizedFolders];
    const rootOrder = buildDenseRootOrder(appendedConnections, appendedFolders, rootOrderIds);
    const nextConnections = appendedConnections.map((connection) => {
      const order = rootOrder.get(connection.id);
      return order === undefined ? connection : { ...connection, order };
    });
    const nextFolders = appendedFolders.map((folder) => {
      const order = rootOrder.get(folder.id);
      return order === undefined ? folder : { ...folder, order };
    });

    try {
      if (credentialWrites.length > 0) await credentialsSetMany(credentialWrites);
    } catch (error) {
      const cleanupError = await cleanupImportedCredentials(credentialKeys);
      const suffix = cleanupError ? `；${cleanupError}` : "";
      throw new Error(`写入导入连接凭据失败：${errorMessage(error)}${suffix}`);
    }

    const previousConnections = connections.value;
    const previousFolders = folders.value;
    connections.value = nextConnections;
    folders.value = nextFolders;
    try {
      await persist();
    } catch (error) {
      connections.value = previousConnections;
      folders.value = previousFolders;
      let restoreError: string | null = null;
      try {
        await persist();
      } catch (restoreFailure) {
        restoreError = `恢复原连接数据失败：${errorMessage(restoreFailure)}`;
      }
      const cleanupError = await cleanupImportedCredentials(credentialKeys);
      const extraErrors = [restoreError, cleanupError].filter((message): message is string => Boolean(message));
      const suffix = extraErrors.length > 0 ? `；${extraErrors.join("；")}` : "";
      throw new Error(`持久化导入连接失败：${errorMessage(error)}${suffix}`);
    }
  }

  /** 删除连接及其系统凭据 */
  async function remove(id: string) {
    connections.value = connections.value.filter((c) => c.id !== id);
    await persist();
    await credentialsDeleteMany([
      { kind: "connectionPassword", id },
      { kind: "connectionPassphrase", id },
    ]);
  }

  /** 统计引用指定代理的连接数量 */
  function countProxyReferences(proxyId: string): number {
    return connections.value.filter((connection) => connection.proxyId === proxyId).length;
  }

  /** 清除全部连接对指定代理的引用，返回受影响连接数量 */
  async function clearProxyReferences(proxyId: string): Promise<number> {
    let changed = 0;
    for (const connection of connections.value) {
      if (connection.proxyId !== proxyId) continue;
      connection.proxyId = null;
      changed += 1;
    }
    if (changed > 0) await persist();
    return changed;
  }

  /** 新增或更新文件夹，返回其 id */
  async function upsertFolder(folder: ConnectionFolder): Promise<string> {
    if (!folder.id) folder.id = genId();
    const idx = folders.value.findIndex((f) => f.id === folder.id);
    const parentId = normalizeParentId(folder.parentId);
    folder.parentId = parentId;
    if (idx >= 0) {
      const current = folders.value[idx];
      const parentChanged = normalizeParentId(current.parentId) !== parentId;
      if (parentChanged) folder.order = nextOrder(parentId);
      else folder.order = folder.order ?? current.order;
      folders.value[idx] = folder;
    } else {
      folder.order = folder.order ?? nextOrder(parentId);
      folders.value.push(folder);
    }
    await persist();
    return folder.id;
  }

  /** 按给定 id 顺序重排同一父级下的连接和文件夹 */
  async function reorderItems(parentId: string | null, orderedIds: string[]) {
    const normalizedParentId = normalizeParentId(parentId);
    const siblingIds = [
      ...folders.value.filter((folder) => belongsToParent(folder, normalizedParentId)).map((folder) => folder.id),
      ...connections.value.filter((conn) => belongsToParent(conn, normalizedParentId)).map((conn) => conn.id),
    ];
    const siblingSet = new Set(siblingIds);
    const seen = new Set<string>();
    const normalizedIds = orderedIds.filter((id) => {
      if (!siblingSet.has(id) || seen.has(id)) return false;
      seen.add(id);
      return true;
    });
    for (const id of siblingIds) {
      if (!seen.has(id)) normalizedIds.push(id);
    }
    let changed = false;
    normalizedIds.forEach((id, index) => {
      if (setItemOrder(id, index)) changed = true;
    });
    if (changed) await persist();
  }

  /** 收集指定文件夹的全部子孙文件夹 id（含自身） */
  function collectFolderIds(id: string): Set<string> {
    const ids = new Set<string>([id]);
    let changed = true;
    // 反复扫描直至没有新的子文件夹被纳入
    while (changed) {
      changed = false;
      for (const folder of folders.value) {
        if (folder.parentId && ids.has(folder.parentId) && !ids.has(folder.id)) {
          ids.add(folder.id);
          changed = true;
        }
      }
    }
    return ids;
  }

  /** 递归删除文件夹，连同其下全部子文件夹与连接一并移除 */
  async function removeFolderRecursive(id: string) {
    const ids = collectFolderIds(id);
    const removedConnectionIds = connections.value
      .filter((connection) => connection.parentId && ids.has(connection.parentId))
      .map((connection) => connection.id);
    folders.value = folders.value.filter((f) => !ids.has(f.id));
    connections.value = connections.value.filter(
      (c) => !(c.parentId && ids.has(c.parentId))
    );
    expandedFolderIds.value = expandedFolderIds.value.filter((folderId) => !ids.has(folderId));
    await persist();
    const credentialKeys = removedConnectionIds.flatMap<CredentialKey>((connectionId) => [
      { kind: "connectionPassword", id: connectionId },
      { kind: "connectionPassphrase", id: connectionId },
    ]);
    if (credentialKeys.length > 0) await credentialsDeleteMany(credentialKeys);
  }

  /** 统计文件夹内含的连接数与子文件夹数（递归） */
  function countFolderContents(id: string): { connCount: number; folderCount: number } {
    const ids = collectFolderIds(id);
    // 自身不计入子文件夹数
    const folderCount = ids.size - 1;
    const connCount = connections.value.filter(
      (c) => c.parentId && ids.has(c.parentId)
    ).length;
    return { connCount, folderCount };
  }

  /**
   * 将若干连接/文件夹移动到目标文件夹（targetParentId 为 null 表示根目录）。
   * 会拦截将文件夹移入其自身或子孙的非法操作。
   */
  async function moveItems(ids: string[], targetParentId: string | null) {
    // 目标所在的祖先链，用于防止把文件夹拖进自己的子孙形成环
    const forbidden = new Set<string>();
    for (const id of ids) {
      if (folders.value.some((f) => f.id === id)) {
        for (const sub of collectFolderIds(id)) forbidden.add(sub);
      }
    }
    if (targetParentId && forbidden.has(targetParentId)) return;

    const idSet = new Set(ids);
    let targetOrder = nextOrder(targetParentId);
    let changed = false;
    for (const folder of folders.value) {
      if (idSet.has(folder.id) && folder.parentId !== targetParentId) {
        folder.parentId = targetParentId;
        folder.order = targetOrder++;
        changed = true;
      }
    }
    for (const conn of connections.value) {
      const current = conn.parentId ?? null;
      if (idSet.has(conn.id) && current !== targetParentId) {
        conn.parentId = targetParentId;
        conn.order = targetOrder++;
        changed = true;
      }
    }
    if (changed) await persist();
  }

  /** 复制连接：在同级目录下生成一份 [ 原名 - 复制 ] ，返回新连接 id */
  async function duplicateConnection(id: string): Promise<string | null> {
    const source = connections.value.find((c) => c.id === id);
    if (!source) return null;
    const parentId = source.parentId ?? null;
    // 同级现有名称集合，用于避免复制名重复
    const siblingNames = new Set(
      connections.value
        .filter((c) => (c.parentId ?? null) === parentId)
        .map((c) => c.name)
    );
    let name = `${source.name} - 复制`;
    let seq = 2;
    while (siblingNames.has(name)) {
      name = `${source.name} - 复制 ${seq++}`;
    }
    const copy: ConnectionConfig = { ...source, id: genId(), name, parentId, order: nextOrder(parentId) };
    const credentialCopies = [];
    if (source.hasPassword) {
      credentialCopies.push({ kind: "connectionPassword" as const, sourceId: source.id, targetId: copy.id });
    }
    if (source.hasPassphrase) {
      credentialCopies.push({ kind: "connectionPassphrase" as const, sourceId: source.id, targetId: copy.id });
    }
    if (credentialCopies.length > 0) await credentialsCopyMany(credentialCopies);
    connections.value.push(copy);
    await persist();
    return copy.id;
  }

  /** 持久化连接管理器的文件夹展开状态 */
  async function setExpandedFolderIds(ids: string[]) {
    const existingIds = new Set(folders.value.map((folder) => folder.id));
    expandedFolderIds.value = Array.from(new Set(ids.filter((id) => existingIds.has(id))));
    await persist();
  }

  return {
    connections,
    folders,
    expandedFolderIds,
    init,
    upsert,
    importBatch,
    remove,
    countProxyReferences,
    clearProxyReferences,
    upsertFolder,
    reorderItems,
    removeFolderRecursive,
    countFolderContents,
    moveItems,
    duplicateConnection,
    setExpandedFolderIds,
  };
});
