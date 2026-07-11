/**
 * 应用设置 store：终端与监控相关的偏好，持久化到本地
 */

import { defineStore } from "pinia";
import { ref } from "vue";
import { load, type Store } from "@tauri-apps/plugin-store";

/** 应用设置项 */
export interface AppSettings {
  /** 终端字号 */
  fontSize: number;
  /** 终端字体 */
  fontFamily: string;
  /** 光标闪烁 */
  cursorBlink: boolean;
  /** 监控采集间隔（秒） */
  monitorInterval: number;
}

/** 默认设置 */
function defaults(): AppSettings {
  return {
    fontSize: 14,
    fontFamily: '"Consolas", "Cascadia Mono", "Courier New", monospace',
    cursorBlink: true,
    monitorInterval: 3,
  };
}

const STORE_FILE = "settings.json";
const STORE_KEY = "settings";

export const useSettingsStore = defineStore("settings", () => {
  /** 当前设置 */
  const settings = ref<AppSettings>(defaults());

  let store: Store | null = null;

  /** 加载设置 */
  async function init() {
    store = await load(STORE_FILE, { defaults: {}, autoSave: true });
    const saved = await store.get<AppSettings>(STORE_KEY);
    if (saved) settings.value = { ...defaults(), ...saved };
  }

  /** 更新并持久化设置 */
  async function update(next: AppSettings) {
    settings.value = next;
    if (store) {
      await store.set(STORE_KEY, next);
      await store.save();
    }
  }

  return { settings, init, update };
});
