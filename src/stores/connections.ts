/**
 * 连接配置 store：管理已保存的连接与分组文件夹，并通过 tauri-plugin-store 持久化到本地
 */

import { defineStore } from "pinia";
import { ref } from "vue";
import { load, type Store } from "@tauri-apps/plugin-store";
import type { ConnectionConfig, ConnectionFolder } from "../types";
import { genId } from "../utils";

/** 持久化文件名与键名 */
const STORE_FILE = "connections.json";
const STORE_KEY = "connections";
const FOLDER_KEY = "folders";

export const useConnectionsStore = defineStore("connections", () => {
  /** 已保存的连接列表 */
  const connections = ref<ConnectionConfig[]>([]);
  /** 已保存的分组文件夹列表 */
  const folders = ref<ConnectionFolder[]>([]);

  let store: Store | null = null;

  /** 从本地加载连接与文件夹列表 */
  async function init() {
    store = await load(STORE_FILE, { defaults: {}, autoSave: true });
    const savedConns = await store.get<ConnectionConfig[]>(STORE_KEY);
    const savedFolders = await store.get<ConnectionFolder[]>(FOLDER_KEY);
    connections.value = savedConns ?? [];
    folders.value = savedFolders ?? [];
  }

  /** 将当前连接与文件夹列表写回本地 */
  async function persist() {
    if (store) {
      await store.set(STORE_KEY, connections.value);
      await store.set(FOLDER_KEY, folders.value);
      await store.save();
    }
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

  /** 新增或更新连接，返回其 id */
  async function upsert(config: ConnectionConfig): Promise<string> {
    if (!config.id) config.id = genId();
    const idx = connections.value.findIndex((c) => c.id === config.id);
    const parentId = normalizeParentId(config.parentId);
    config.parentId = parentId;
    if (idx >= 0) {
      const current = connections.value[idx];
      const parentChanged = normalizeParentId(current.parentId) !== parentId;
      if (parentChanged) config.order = nextOrder(parentId);
      else config.order = config.order ?? current.order;
      connections.value[idx] = config;
    } else {
      config.order = config.order ?? nextOrder(parentId);
      connections.value.push(config);
    }
    await persist();
    return config.id;
  }

  /** 删除连接 */
  async function remove(id: string) {
    connections.value = connections.value.filter((c) => c.id !== id);
    await persist();
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
    folders.value = folders.value.filter((f) => !ids.has(f.id));
    connections.value = connections.value.filter(
      (c) => !(c.parentId && ids.has(c.parentId))
    );
    await persist();
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
    connections.value.push(copy);
    await persist();
    return copy.id;
  }

  return {
    connections,
    folders,
    init,
    upsert,
    remove,
    upsertFolder,
    reorderItems,
    removeFolderRecursive,
    countFolderContents,
    moveItems,
    duplicateConnection,
  };
});
