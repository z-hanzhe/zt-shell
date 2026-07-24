/**
 * 代理配置 store：管理可复用代理列表并通过 tauri-plugin-store 持久化到本地
 */

import { defineStore } from "pinia";
import { ref } from "vue";
import { load, type Store } from "@tauri-apps/plugin-store";
import type { ProxyConfig } from "../types";
import { genId } from "../utils";

const STORE_FILE = "proxies.json";
const STORE_KEY = "proxies";

export const useProxiesStore = defineStore("proxies", () => {
  /** 已保存的代理列表 */
  const proxies = ref<ProxyConfig[]>([]);

  let store: Store | null = null;

  /** 从本地加载代理列表 */
  async function init() {
    store = await load(STORE_FILE, { defaults: {}, autoSave: true });
    proxies.value = (await store.get<ProxyConfig[]>(STORE_KEY)) ?? [];
  }

  /** 将当前代理列表写回本地 */
  async function persist() {
    if (!store) return;
    await store.set(STORE_KEY, proxies.value);
    await store.save();
  }

  /** 新增或更新代理，返回代理 id */
  async function upsert(config: ProxyConfig): Promise<string> {
    const normalized = { ...config, id: config.id || genId() };
    const index = proxies.value.findIndex((proxy) => proxy.id === normalized.id);
    if (index >= 0) proxies.value[index] = normalized;
    else proxies.value.push(normalized);
    await persist();
    return normalized.id;
  }

  /** 删除代理 */
  async function remove(id: string) {
    proxies.value = proxies.value.filter((proxy) => proxy.id !== id);
    await persist();
  }

  /** 移动代理排序位置 */
  async function move(id: string, direction: "up" | "down") {
    const index = proxies.value.findIndex((proxy) => proxy.id === id);
    const targetIndex = direction === "up" ? index - 1 : index + 1;
    if (index < 0 || targetIndex < 0 || targetIndex >= proxies.value.length) return;
    const next = [...proxies.value];
    const [item] = next.splice(index, 1);
    next.splice(targetIndex, 0, item);
    proxies.value = next;
    await persist();
  }

  return { proxies, init, upsert, remove, move };
});
