/**
 * 代理配置 store：管理可复用代理列表并通过 tauri-plugin-store 持久化到本地
 */

import { defineStore } from "pinia";
import { ref } from "vue";
import { load, type Store } from "@tauri-apps/plugin-store";
import {
  credentialsCheckMany,
  credentialsDeleteMany,
  credentialsSetMany,
} from "../api";
import type { CredentialKey, CredentialWrite, ProxyConfig, SecretChange } from "../types";
import { genId } from "../utils";

const STORE_FILE = "proxies.json";
const STORE_KEY = "proxies";

export const useProxiesStore = defineStore("proxies", () => {
  /** 已保存的代理列表 */
  const proxies = ref<ProxyConfig[]>([]);

  let store: Store | null = null;
  /** 串行持久化队列，单次失败会被队列吸收但仍返回给原调用方 */
  let persistQueue: Promise<void> = Promise.resolve();

  /** 从本地加载代理列表 */
  async function init() {
    store = await load(STORE_FILE, { defaults: {}, autoSave: false });
    proxies.value = (await store.get<ProxyConfig[]>(STORE_KEY)) ?? [];
    await migrateLegacyCredentials();
    await refreshCredentialAvailability();
  }

  /** 将当前代理列表写回本地 */
  async function persist() {
    const currentStore = store;
    if (!currentStore) return;
    const proxySnapshot = proxies.value.map((proxy) => stripProxySecret(proxy));
    const operation = persistQueue.then(async () => {
      await currentStore.set(STORE_KEY, proxySnapshot);
      await currentStore.save();
    });
    persistQueue = operation.catch(() => undefined);
    await operation;
  }

  /** 从代理配置中移除仅允许短暂存在于内存中的密码明文 */
  function stripProxySecret(config: ProxyConfig): ProxyConfig {
    const sanitized = { ...config };
    delete sanitized.password;
    return sanitized;
  }

  /** 将未知异常转换为可展示的错误文本 */
  function errorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  /** 尽力删除批量导入期间可能写入的代理凭据，返回清理失败信息 */
  async function cleanupImportedCredentials(keys: CredentialKey[]): Promise<string | null> {
    if (keys.length === 0) return null;
    try {
      await credentialsDeleteMany(keys);
      return null;
    } catch (error) {
      return `清理本批代理凭据失败：${errorMessage(error)}`;
    }
  }

  /** 校验批量导入只能追加标识非空且互不冲突的新代理 */
  function validateImportItems(importedProxies: ProxyConfig[]): void {
    const ids = new Set(proxies.value.map((proxy) => proxy.id));
    for (const proxy of importedProxies) {
      if (!proxy.id.trim()) throw new Error("导入代理标识不能为空");
      if (ids.has(proxy.id)) throw new Error(`导入代理标识 [ ${proxy.id} ] 已存在`);
      ids.add(proxy.id);
    }
  }

  /** 将旧版 JSON 中的代理密码迁移到系统凭据库 */
  async function migrateLegacyCredentials() {
    const writes: CredentialWrite[] = [];
    const deletes: CredentialKey[] = [];
    const legacyIds = new Set<string>();
    for (const proxy of proxies.value) {
      if (typeof proxy.password !== "string") continue;
      legacyIds.add(proxy.id);
      if (proxy.password) writes.push({ kind: "proxyPassword", id: proxy.id, value: proxy.password });
      else deletes.push({ kind: "proxyPassword", id: proxy.id });
    }
    if (writes.length > 0) await credentialsSetMany(writes);
    if (deletes.length > 0) await credentialsDeleteMany(deletes);
    if (legacyIds.size === 0) return;
    for (const proxy of proxies.value) {
      if (!legacyIds.has(proxy.id)) continue;
      proxy.hasPassword = Boolean(proxy.password);
      delete proxy.password;
    }
    await persist();
  }

  /** 检查系统凭据库中的代理密码是否仍可用 */
  async function refreshCredentialAvailability() {
    const keys: CredentialKey[] = proxies.value
      .filter((proxy) => proxy.hasPassword)
      .map((proxy) => ({ kind: "proxyPassword", id: proxy.id }));
    if (keys.length === 0) return;
    const available = await credentialsCheckMany(keys);
    let changed = false;
    keys.forEach((key, index) => {
      if (available[index]) return;
      const proxy = proxies.value.find((item) => item.id === key.id);
      if (!proxy) return;
      proxy.hasPassword = false;
      changed = true;
    });
    if (changed) await persist();
  }

  /** 新增或更新代理及其系统凭据，返回代理标识 */
  async function upsert(config: ProxyConfig, passwordChange?: SecretChange): Promise<string> {
    const id = config.id || genId();
    const current = proxies.value.find((proxy) => proxy.id === id);
    const inferredChange: SecretChange = passwordChange ?? (
      typeof config.password === "string"
        ? config.password
          ? { mode: "set", value: config.password }
          : { mode: "clear" }
        : { mode: "keep" }
    );
    let hasPassword = current?.hasPassword ?? config.hasPassword ?? false;
    if (inferredChange.mode === "set" && inferredChange.value) {
      await credentialsSetMany([{ kind: "proxyPassword", id, value: inferredChange.value }]);
      hasPassword = true;
    } else if (inferredChange.mode === "clear") {
      await credentialsDeleteMany([{ kind: "proxyPassword", id }]);
      hasPassword = false;
    }
    const normalized = stripProxySecret({ ...config, id, hasPassword });
    const index = proxies.value.findIndex((proxy) => proxy.id === normalized.id);
    if (index >= 0) proxies.value[index] = normalized;
    else proxies.value.push(normalized);
    await persist();
    return normalized.id;
  }

  /**
   * 原子追加一批导入代理，返回本批新增标识供后续跨 store 失败时撤销。
   * 代理密码先写入系统凭据库，持久化失败时恢复原数组并尽力清理凭据。
   */
  async function importBatch(importedProxies: ProxyConfig[]): Promise<string[]> {
    if (!store) throw new Error("代理存储尚未初始化，无法导入代理");
    validateImportItems(importedProxies);
    if (importedProxies.length === 0) return [];

    const credentialWrites: CredentialWrite[] = [];
    const credentialKeys: CredentialKey[] = [];
    const sanitizedProxies = importedProxies.map((proxy) => {
      const hasPassword = typeof proxy.password === "string" && proxy.password.length > 0;
      if (hasPassword) {
        credentialWrites.push({
          kind: "proxyPassword",
          id: proxy.id,
          value: proxy.password as string,
        });
        credentialKeys.push({ kind: "proxyPassword", id: proxy.id });
      }
      return stripProxySecret({ ...proxy, hasPassword });
    });

    try {
      if (credentialWrites.length > 0) await credentialsSetMany(credentialWrites);
    } catch (error) {
      const cleanupError = await cleanupImportedCredentials(credentialKeys);
      const suffix = cleanupError ? `；${cleanupError}` : "";
      throw new Error(`写入导入代理凭据失败：${errorMessage(error)}${suffix}`);
    }

    const previousProxies = proxies.value;
    proxies.value = [...previousProxies, ...sanitizedProxies];
    try {
      await persist();
    } catch (error) {
      proxies.value = previousProxies;
      let restoreError: string | null = null;
      try {
        await persist();
      } catch (restoreFailure) {
        restoreError = `恢复原代理数据失败：${errorMessage(restoreFailure)}`;
      }
      const cleanupError = await cleanupImportedCredentials(credentialKeys);
      const extraErrors = [restoreError, cleanupError].filter((message): message is string => Boolean(message));
      const suffix = extraErrors.length > 0 ? `；${extraErrors.join("；")}` : "";
      throw new Error(`持久化导入代理失败：${errorMessage(error)}${suffix}`);
    }
    return sanitizedProxies.map((proxy) => proxy.id);
  }

  /**
   * 撤销一批由 `importBatch` 新增的代理；调用方只能传入该方法返回的标识。
   * 先移除持久化记录，再删除系统凭据，避免持久化失败时留下缺少密码的代理。
   */
  async function rollbackImported(ids: string[]): Promise<void> {
    if (!store) throw new Error("代理存储尚未初始化，无法撤销导入代理");
    const uniqueIds = Array.from(new Set(ids));
    if (uniqueIds.length === 0) return;
    const idSet = new Set(uniqueIds);
    const previousProxies = proxies.value;
    proxies.value = previousProxies.filter((proxy) => !idSet.has(proxy.id));
    try {
      await persist();
    } catch (error) {
      proxies.value = previousProxies;
      let restoreError: string | null = null;
      try {
        await persist();
      } catch (restoreFailure) {
        restoreError = `恢复导入代理失败：${errorMessage(restoreFailure)}`;
      }
      const suffix = restoreError ? `；${restoreError}` : "";
      throw new Error(`撤销导入代理失败：${errorMessage(error)}${suffix}`);
    }

    try {
      await credentialsDeleteMany(
        uniqueIds.map((id) => ({ kind: "proxyPassword" as const, id }))
      );
    } catch (error) {
      throw new Error(`代理记录已撤销，但清理导入代理凭据失败：${errorMessage(error)}`);
    }
  }

  /** 删除代理及其系统凭据 */
  async function remove(id: string) {
    proxies.value = proxies.value.filter((proxy) => proxy.id !== id);
    await persist();
    await credentialsDeleteMany([{ kind: "proxyPassword", id }]);
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

  return { proxies, init, upsert, importBatch, rollbackImported, remove, move };
});
