/**
 * 连接配置 store：管理已保存的连接，并通过 tauri-plugin-store 持久化到本地
 */

import { defineStore } from "pinia";
import { ref } from "vue";
import { load, type Store } from "@tauri-apps/plugin-store";
import type { ConnectionConfig } from "../types";
import { genId } from "../utils";

/** 持久化文件名与键名 */
const STORE_FILE = "connections.json";
const STORE_KEY = "connections";

export const useConnectionsStore = defineStore("connections", () => {
  /** 已保存的连接列表 */
  const connections = ref<ConnectionConfig[]>([]);

  let store: Store | null = null;

  /** 从本地加载连接列表 */
  async function init() {
    store = await load(STORE_FILE, { defaults: {}, autoSave: true });
    const saved = await store.get<ConnectionConfig[]>(STORE_KEY);
    connections.value = saved ?? [];
  }

  /** 将当前列表写回本地 */
  async function persist() {
    if (store) {
      await store.set(STORE_KEY, connections.value);
      await store.save();
    }
  }

  /** 新增或更新连接，返回其 id */
  async function upsert(config: ConnectionConfig): Promise<string> {
    if (!config.id) config.id = genId();
    const idx = connections.value.findIndex((c) => c.id === config.id);
    if (idx >= 0) {
      connections.value[idx] = config;
    } else {
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

  return { connections, init, upsert, remove };
});
