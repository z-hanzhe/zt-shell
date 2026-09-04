import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";

/** 默认界面缩放比例，1 表示仅采用操作系统 DPI 缩放 */
export const DEFAULT_UI_SCALE = 1;

/** 设置页可选的界面缩放档位 */
export const UI_SCALE_OPTIONS = [
  { value: 0.8, label: "80%" },
  { value: 0.9, label: "90%" },
  { value: 1, label: "100%" },
  { value: 1.1, label: "110%" },
  { value: 1.25, label: "125%" },
  { value: 1.5, label: "150%" },
  { value: 1.75, label: "175%" },
  { value: 2, label: "200%" },
] as const;

/** 界面缩放变更事件载荷 */
export interface UiScaleChangedPayload {
  /** 相对于系统 DPI 缩放的附加比例 */
  scale: number;
}

/** 主窗口向独立窗口同步界面缩放的事件 */
export const UI_SCALE_CHANGED_EVENT = "settings://ui-scale-changed";

const MIN_UI_SCALE = UI_SCALE_OPTIONS[0].value;
const MAX_UI_SCALE = UI_SCALE_OPTIONS[UI_SCALE_OPTIONS.length - 1].value;
let currentUiScale = DEFAULT_UI_SCALE;
let uiScaleApplyQueue: Promise<void> = Promise.resolve();

/** 将外部输入规范为应用支持的界面缩放范围 */
export function normalizeUiScale(value: unknown): number {
  if (
    typeof value !== "number" &&
    (typeof value !== "string" || value.trim() === "")
  ) {
    return DEFAULT_UI_SCALE;
  }
  const parsed = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(parsed)) return DEFAULT_UI_SCALE;
  const clamped = Math.min(Math.max(parsed, MIN_UI_SCALE), MAX_UI_SCALE);
  return Math.round(clamped * 100) / 100;
}

/** 获取当前进程内最近一次应用的界面缩放比例 */
export function getCurrentUiScale(): number {
  return currentUiScale;
}

/**
 * 缩放当前 WebView；浏览器预览时使用 CSS zoom 模拟相同效果。
 * @returns 实际应用的规范化比例
 */
export function applyUiScale(value: unknown): Promise<number> {
  const scale = normalizeUiScale(value);
  currentUiScale = scale;

  const task = uiScaleApplyQueue.then(async () => {
    if (isTauri()) {
      document.documentElement.style.removeProperty("zoom");
      await getCurrentWebview().setZoom(scale);
    } else if (scale === DEFAULT_UI_SCALE) {
      document.documentElement.style.removeProperty("zoom");
    } else {
      document.documentElement.style.setProperty("zoom", String(scale));
    }
    return scale;
  });
  uiScaleApplyQueue = task.then(
    () => undefined,
    () => undefined
  );
  return task;
}
