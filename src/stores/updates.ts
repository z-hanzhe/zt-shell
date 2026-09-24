/** 更新状态独立于设置页生命周期，跳过版本与网络偏好持久化保存。 */
import { computed, ref } from "vue";
import { defineStore } from "pinia";
import { isTauri } from "@tauri-apps/api/core";
import { load, type Store } from "@tauri-apps/plugin-store";
import {
  credentialsDeleteMany,
  credentialsSetMany,
  updaterCheck,
  updaterDownload,
  updaterEnvironment,
  updaterInstall,
} from "../api";
import type { SecretChange, UpdateInfo, UpdatePreferences, UpdateProgress } from "../types";

/** 可选更新源与外部页面，与原生更新源枚举保持一致。 */
export const UPDATE_SOURCES: ReadonlyArray<{
  id: UpdatePreferences["source"];
  name: string;
  repositoryUrl?: string;
  releaseUrl?: string;
}> = [
  {
    id: "github",
    name: "GitHub",
    repositoryUrl: "https://github.com/z-hanzhe/zt-shell",
    releaseUrl: "https://github.com/z-hanzhe/zt-shell/releases",
  },
];

/** 校验并规范化代理，禁止在偏好中保存认证信息。 */
export function normalizeUpdatePreferences(value: UpdatePreferences): UpdatePreferences {
  if (value.source !== "github") throw new Error("暂不支持此更新源");
  const proxyUrl = value.proxyUrl.trim();
  if (proxyUrl || value.useProxy) {
    let url: URL;
    try {
      url = new URL(proxyUrl);
    } catch {
      throw new Error("请填写完整的代理地址，例如 http://127.0.0.1:7897");
    }
    if (!["http:", "https:"].includes(url.protocol) || !url.hostname || url.username || url.password || url.pathname !== "/" || url.search || url.hash) {
      throw new Error("请填写不含账号、密码或路径的 HTTP / HTTPS 代理地址");
    }
  }
  const proxyUsername = value.proxyUsername?.trim() ?? "";
  if (proxyUsername.includes(":")) throw new Error("代理用户名不能包含冒号");
  return {
    autoCheck: value.autoCheck !== false,
    source: value.source,
    useProxy: value.useProxy,
    proxyUrl,
    proxyUsername,
    proxyCredentialId: value.proxyCredentialId ?? null,
  };
}

export const useUpdatesStore = defineStore("updates", () => {
  const preferences = ref<UpdatePreferences>({
    autoCheck: true,
    source: "github",
    useProxy: false,
    proxyUrl: "http://127.0.0.1:7897",
    proxyUsername: "",
    proxyCredentialId: null,
  });
  const currentVersion = ref("");
  const canInstall = ref(false);
  const latest = ref<UpdateInfo | null>(null);
  const downloadReady = ref(false);
  const skippedVersion = ref("");
  const phase = ref<"idle" | "checking" | "downloading" | "ready" | "installing">("idle");
  const preparingInstall = ref(false);
  const saving = ref(false);
  const initializing = ref(false);
  const error = ref("");
  const lastChecked = ref("");
  const progress = ref<UpdateProgress>({ downloaded: 0, total: null });
  const hasUpdate = computed(() => !!latest.value && latest.value.version !== skippedVersion.value);
  const busy = computed(() => initializing.value || saving.value || preparingInstall.value || ["checking", "downloading", "installing"].includes(phase.value));
  const skipped = computed(() => !!latest.value && latest.value.version === skippedVersion.value);
  let store: Store | null = null;
  let initialization: Promise<void> | undefined;

  /** 启动时只初始化一次，按已保存偏好决定是否静默检查。 */
  function init(): Promise<void> {
    initialization ??= initialize();
    return initialization;
  }

  /** 加载网络偏好后检查，持久化不可用时保留错误供用户查看。 */
  async function initialize(): Promise<void> {
    if (!isTauri()) return;
    initializing.value = true;
    try {
      const environment = await updaterEnvironment();
      currentVersion.value = environment.version;
      canInstall.value = environment.canInstall;
      store = await load("updater.json", { defaults: {}, autoSave: false });
      const saved = await store.get<UpdatePreferences>("preferences");
      if (saved) preferences.value = normalizeUpdatePreferences(saved);
      skippedVersion.value = (await store.get<string>("skippedVersion")) ?? "";
    } catch (reason) {
      error.value = `加载更新设置失败：${String(reason)}`;
      return;
    } finally {
      initializing.value = false;
    }
    if (preferences.value.autoCheck) await check();
  }

  /** 手动或静默检查，失败时保留先前已发现的版本和提醒。 */
  async function check(): Promise<void> {
    if (busy.value || phase.value === "ready") return;
    if (!isTauri()) {
      error.value = "请在桌面应用中检查更新";
      return;
    }
    phase.value = "checking";
    error.value = "";
    try {
      latest.value = await updaterCheck(preferences.value);
      downloadReady.value = latest.value !== null;
      lastChecked.value = new Date().toLocaleString();
    } catch (reason) {
      error.value = String(reason);
    } finally {
      phase.value = "idle";
    }
  }

  /** 仅保存更新偏好与密码，后续检查须由用户手动发起。 */
  async function savePreferences(
    value: UpdatePreferences,
    passwordChange: SecretChange = { mode: "keep" }
  ): Promise<void> {
    if (busy.value) return;
    const next = normalizeUpdatePreferences(value);
    const previous = { ...preferences.value };
    // 密码引用由保存逻辑生成，不接受表单自带的凭据引用。
    next.proxyCredentialId = previous.proxyCredentialId;
    if (passwordChange.mode === "clear") next.proxyCredentialId = null;
    if (passwordChange.mode === "set") {
      if (!passwordChange.value) throw new Error("代理密码不能为空");
      next.proxyCredentialId = `updater-proxy:${crypto.randomUUID()}`;
    }
    if (next.proxyCredentialId && !next.proxyUsername) {
      throw new Error("请填写代理用户名，或清除已保存的代理密码");
    }
    const networkChanged = next.source !== previous.source ||
      next.useProxy !== previous.useProxy ||
      next.proxyUrl !== previous.proxyUrl ||
      next.proxyUsername !== previous.proxyUsername ||
      next.proxyCredentialId !== previous.proxyCredentialId;
    if (networkChanged && phase.value === "ready") return;
    const newCredentialId = passwordChange.mode === "set" ? next.proxyCredentialId : null;
    saving.value = true;
    try {
      if (isTauri() && !store) throw new Error("更新设置尚未加载，请重启应用后重试");
      if (passwordChange.mode === "set" && newCredentialId) {
        await credentialsSetMany([{ kind: "proxyPassword", id: newCredentialId, value: passwordChange.value }]);
      }
      if (store) {
        await store.set("preferences", next);
        await store.save();
      }
      preferences.value = next;
      // 自动检查开关只影响下次启动，不使已有检查结果或下载包失效。
      if (networkChanged) {
        downloadReady.value = false;
        lastChecked.value = "";
        error.value = "";
      }
    } catch (reason) {
      // 保存失败时恢复插件内存快照，避免之后保存跳过记录时带入半完成的设置。
      if (store) await store.set("preferences", previous).catch(() => undefined);
      if (newCredentialId) {
        await credentialsDeleteMany([{ kind: "proxyPassword", id: newCredentialId }]).catch(() => undefined);
      }
      throw reason;
    } finally {
      saving.value = false;
    }
    if (previous.proxyCredentialId && previous.proxyCredentialId !== next.proxyCredentialId) {
      // 新配置提交后再删除旧凭据，磁盘保存失败不会破坏当前可用的认证。
      await credentialsDeleteMany([{ kind: "proxyPassword", id: previous.proxyCredentialId }]).catch((reason) => {
        console.warn("清理旧更新代理凭据失败", reason);
      });
    }
  }

  /** 仅跳过当前版本；先落盘再撤去红点，失败时允许重试。 */
  async function skip(): Promise<void> {
    if (!latest.value || busy.value) return;
    const version = latest.value.version;
    saving.value = true;
    error.value = "";
    try {
      if (isTauri() && !store) throw new Error("更新设置尚未加载");
      if (store) {
        await store.set("skippedVersion", version);
        await store.save();
      }
      skippedVersion.value = version;
    } catch (reason) {
      error.value = `保存跳过记录失败：${String(reason)}`;
    } finally {
      saving.value = false;
    }
  }

  /** 下载完成后停留在待安装状态，不打断当前会话。 */
  async function download(): Promise<void> {
    if (!latest.value || !downloadReady.value || busy.value || phase.value === "ready" || !canInstall.value) return;
    phase.value = "downloading";
    error.value = "";
    progress.value = { downloaded: 0, total: null };
    try {
      await updaterDownload(latest.value.version, (value) => { progress.value = value; });
      phase.value = "ready";
    } catch (reason) {
      phase.value = "idle";
      error.value = String(reason);
    }
  }

  /** 由应用根组件在退出保护通过后调用，安装失败保留已下载内容。 */
  async function install(): Promise<void> {
    if (!latest.value || phase.value !== "ready") throw new Error("请先下载更新");
    phase.value = "installing";
    error.value = "";
    try {
      await updaterInstall(latest.value.version);
    } catch (reason) {
      phase.value = "ready";
      error.value = String(reason);
      throw reason;
    }
  }

  return { preferences, currentVersion, canInstall, latest, downloadReady, skippedVersion, phase, preparingInstall, error, lastChecked, progress, hasUpdate, busy, skipped, init, check, savePreferences, skip, download, install };
});
