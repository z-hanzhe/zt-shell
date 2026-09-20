/**
 * 应用设置 store：终端、监控与下载相关偏好，持久化到本地
 */

import { defineStore } from "pinia";
import { ref } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import { downloadDir } from "@tauri-apps/api/path";
import { load, type Store } from "@tauri-apps/plugin-store";
import { DEFAULT_UI_SCALE, normalizeUiScale } from "../uiScale";
import {
  DEFAULT_EDITOR_FONT_SIZE,
  normalizeEditorFontSize,
} from "../editorProtocol";

/** 应用设置项 */
export interface AppSettings {
  /** 相对于系统 DPI 缩放的界面比例 */
  uiScale: number;
  /** 终端字号 */
  fontSize: number;
  /** 文本编辑器基础字号 */
  editorFontSize: number;
  /** 终端字体 */
  fontFamily: string;
  /** 光标闪烁 */
  cursorBlink: boolean;
  /** 本地下载目录 */
  downloadPath: string;
  /** 监控采集间隔（秒） */
  monitorInterval: number;
}

/** 生成默认设置 */
function defaults(downloadPath = ""): AppSettings {
  return {
    uiScale: DEFAULT_UI_SCALE,
    fontSize: 14,
    editorFontSize: DEFAULT_EDITOR_FONT_SIZE,
    fontFamily: '"Consolas", "Cascadia Mono", "Courier New", monospace',
    cursorBlink: true,
    downloadPath,
    monitorInterval: 3,
  };
}

/** 解析当前系统的标准下载目录，浏览器预览或系统不支持时返回空 */
async function resolveDefaultDownloadPath(): Promise<string> {
  if (!isTauri()) return "";
  try {
    return await downloadDir();
  } catch (error) {
    console.warn("读取系统下载目录失败", error);
    return "";
  }
}

/** 解析包含当前系统下载目录的完整默认设置 */
export async function createDefaultAppSettings(): Promise<AppSettings> {
  return defaults(await resolveDefaultDownloadPath());
}

const STORE_FILE = "settings.json";
const STORE_KEY = "settings";

export const useSettingsStore = defineStore("settings", () => {
  /** 当前设置 */
  const settings = ref<AppSettings>(defaults());

  let store: Store | null = null;

  /** 加载设置 */
  async function init() {
    const defaultSettings = await createDefaultAppSettings();
    settings.value = defaultSettings;
    store = await load(STORE_FILE, { defaults: {}, autoSave: true });
    const saved = await store.get<Partial<AppSettings>>(STORE_KEY);
    if (saved) {
      const merged = { ...defaultSettings, ...saved };
      settings.value = {
        ...merged,
        uiScale: normalizeUiScale(merged.uiScale),
        editorFontSize: normalizeEditorFontSize(merged.editorFontSize),
        downloadPath:
          typeof merged.downloadPath === "string" && merged.downloadPath.trim()
            ? merged.downloadPath.trim()
            : defaultSettings.downloadPath,
      };
    }
  }

  /** 更新并持久化设置 */
  async function update(next: AppSettings) {
    const normalized = {
      ...next,
      uiScale: normalizeUiScale(next.uiScale),
      editorFontSize: normalizeEditorFontSize(next.editorFontSize),
      downloadPath: next.downloadPath.trim(),
    };
    if (store) {
      await store.set(STORE_KEY, normalized);
      await store.save();
    }
    settings.value = normalized;
  }

  return { settings, init, update };
});
