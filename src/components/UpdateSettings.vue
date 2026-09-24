<script setup lang="ts">
/** 更新页面：版本状态、更新说明和网络偏好。 */
import { computed, ref } from "vue";
import Icon from "./Icon.vue";
import ReleaseNotes from "./ReleaseNotes.vue";
import UpdateProxyDialog from "./UpdateProxyDialog.vue";
import { openExternalUrl } from "../api";
import type { UpdatePreferences } from "../types";
import { UPDATE_SOURCES, useUpdatesStore } from "../stores/updates";

const emit = defineEmits<{ (event: "install-update"): void }>();
const updates = useUpdatesStore();
const proxyDialogOpen = ref(false);
const networkError = ref("");
const openingSourcePage = ref(false);
const sourcePageError = ref("");
const activeSource = computed(() => UPDATE_SOURCES.find((source) => source.id === updates.preferences.source));
const networkLocked = computed(() => updates.busy || updates.phase === "ready");
const percentage = computed(() => updates.progress.total ? Math.min(100, Math.floor(updates.progress.downloaded / updates.progress.total * 100)) : null);
const status = computed(() => {
  if (updates.phase === "checking") return "正在检查更新";
  if (updates.phase === "downloading") return "正在下载更新";
  if (updates.phase === "installing" || updates.preparingInstall) return "准备安装更新";
  if (updates.phase === "ready") return "更新已就绪";
  if (updates.latest) return updates.skipped ? "已跳过本次更新" : "发现新版本";
  if (updates.error) return "暂时无法检查更新";
  if (updates.lastChecked) return "已是最新版本";
  return "保持 ZTShell 为最新版本";
});
/** 更新源选定后直接保存，不触发检查或修改代理配置。 */
async function saveSource(event: Event): Promise<void> {
  const select = event.target as HTMLSelectElement;
  const source = select.value as UpdatePreferences["source"];
  if (networkLocked.value || source === updates.preferences.source) return;
  networkError.value = "";
  try {
    await updates.savePreferences({ ...updates.preferences, source });
  } catch (reason) {
    networkError.value = String(reason);
  } finally {
    select.value = updates.preferences.source;
  }
}

/** 保存启动检查开关，失败时恢复为已保存的状态。 */
async function saveAutoCheck(event: Event): Promise<void> {
  const input = event.target as HTMLInputElement;
  if (updates.busy) {
    input.checked = updates.preferences.autoCheck;
    return;
  }
  networkError.value = "";
  try {
    await updates.savePreferences({ ...updates.preferences, autoCheck: input.checked });
  } catch (reason) {
    networkError.value = `保存自动检查设置失败：${String(reason)}`;
  } finally {
    input.checked = updates.preferences.autoCheck;
  }
}

/** 使用系统默认浏览器打开当前更新源的外部页面。 */
async function openSourcePage(url: string | undefined, label: string): Promise<void> {
  if (!url || openingSourcePage.value) return;
  openingSourcePage.value = true;
  sourcePageError.value = "";
  try {
    await openExternalUrl(url);
  } catch (reason) {
    sourcePageError.value = `打开${label}失败：${String(reason)}`;
  } finally {
    openingSourcePage.value = false;
  }
}

/** 将下载字节数展示为便于阅读的大小。 */
function size(bytes: number): string {
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
</script>

<template>
  <div class="settings-page">
    <header class="settings-page-header"><div><h1>检查更新</h1><p>新功能与改进，在这里与你见面。</p></div><span class="settings-badge">稳定版</span></header>
    <section class="settings-card update-overview" aria-live="polite">
      <div class="update-overview-top">
        <div class="update-hero">
          <a
            v-if="activeSource?.repositoryUrl"
            class="update-app-icon update-app-link"
            :href="activeSource.repositoryUrl"
            :title="`打开 ${activeSource.name} 仓库首页`"
            :aria-label="`打开 ${activeSource.name} 仓库首页`"
            :aria-busy="openingSourcePage"
            @click.prevent="openSourcePage(activeSource.repositoryUrl, '仓库首页')"
          ><img src="/app-icon.png" alt="ZTShell" /></a>
          <div v-else class="update-app-icon"><img src="/app-icon.png" alt="ZTShell" /></div>
          <div class="update-hero-copy">
            <span class="settings-eyebrow">ZTShell</span>
            <h2>{{ status }}</h2>
            <p>
              当前版本 {{ updates.currentVersion ? `v${updates.currentVersion}` : "—" }}<template v-if="updates.latest"> <span class="update-arrow">→</span> <strong>v{{ updates.latest.version }}</strong></template><template v-if="activeSource?.releaseUrl">，<a
                class="update-release-link"
                :href="activeSource.releaseUrl"
                :title="`打开 ${activeSource.name} 发布页面`"
                :aria-busy="openingSourcePage"
                @click.prevent="openSourcePage(activeSource.releaseUrl, '发布页面')"
              >点击打开发布页</a></template>
            </p>
          </div>
        </div>
        <div class="update-check">
          <button v-if="updates.phase !== 'ready'" class="btn" :class="{ 'btn-primary': !updates.latest }" :disabled="updates.busy" @click="updates.check"><Icon name="refresh" :size="14" />{{ updates.phase === 'checking' ? '检查中…' : '检查更新' }}</button>
          <p v-if="updates.lastChecked" class="settings-caption">上次检查：{{ updates.lastChecked }}</p>
        </div>
      </div>
      <p v-if="sourcePageError" class="settings-error" role="alert">{{ sourcePageError }}</p>
      <p v-if="updates.skipped && updates.phase === 'idle'" class="settings-hint">本版本不再提醒，下个新版本发布后会再次提示。你仍可随时下载此版本。</p>
      <p v-else-if="updates.phase === 'ready'" class="settings-hint">下载完成并已通过签名验证。安装时会退出程序并重新启动。</p>
      <p v-else-if="updates.phase === 'downloading'" class="settings-hint">你可以切换到其他标签页继续工作，下载完成后再安装。</p>
      <div v-if="updates.phase === 'downloading'" class="update-progress">
        <progress :value="percentage ?? undefined" max="100" aria-label="更新下载进度"></progress>
        <span>{{ size(updates.progress.downloaded) }}<template v-if="updates.progress.total"> / {{ size(updates.progress.total) }} · {{ percentage }}%</template><template v-else> · 正在接收数据</template></span>
      </div>
      <div v-if="updates.error" class="settings-error" role="alert">{{ updates.error }}<p>请检查网络或代理配置，然后重试。</p></div>
      <div v-if="updates.latest" class="update-actions">
        <button v-if="updates.phase === 'ready'" class="btn btn-primary" :disabled="updates.busy" @click="emit('install-update')"><Icon name="refresh" :size="14" />安装并重启</button>
        <button v-else-if="updates.latest" class="btn btn-primary" :disabled="updates.busy || !updates.canInstall || !updates.downloadReady" @click="updates.download"><Icon name="download" :size="14" />{{ updates.phase === 'downloading' ? '下载中…' : '下载更新' }}</button>
        <button v-if="updates.latest && !updates.skipped && (updates.phase === 'idle' || updates.phase === 'ready')" class="btn" :disabled="updates.busy" @click="updates.skip">跳过本次更新</button>
      </div>
      <p v-if="!updates.canInstall" class="settings-hint">开发或浏览器预览环境不支持下载安装，请使用正式安装包。</p>
    </section>

    <section v-if="updates.latest" class="settings-card">
      <h2><Icon name="file" :size="17" />更新说明 <span class="settings-caption">v{{ updates.latest.version }}</span></h2>
      <ReleaseNotes class="update-notes" :content="updates.latest.notes || '此版本未提供更新说明。'" />
    </section>

    <section class="settings-card">
      <h2><Icon name="network" :size="17" />更新来源与网络</h2>
      <label class="settings-row update-auto-check-row">
        <span class="settings-label">启动时自动检查更新<small>每次打开软件时检查一次新版，关闭后需手动检查</small></span>
        <span class="settings-toggle">
          <input type="checkbox" role="switch" :checked="updates.preferences.autoCheck" :disabled="updates.busy" @change="saveAutoCheck" />
          <span>{{ updates.preferences.autoCheck ? "启用" : "关闭" }}</span>
        </span>
      </label>
      <div class="settings-row">
        <label for="update-source" class="settings-label">选择更新源<small>检查和下载安装包使用同一来源</small></label>
        <select id="update-source" :value="updates.preferences.source" class="input settings-control" :disabled="networkLocked" @change="saveSource">
          <option v-for="source in UPDATE_SOURCES" :key="source.id" :value="source.id">{{ source.name }}</option>
        </select>
      </div>
      <div class="settings-row update-proxy-row">
        <span class="settings-label">
          <span>网络代理设置（{{ updates.preferences.useProxy ? "已启用" : "未启用" }}）</span>
          <small>使用网络代理进行 ZTShell 检查下载更新操作</small>
        </span>
        <button
          class="btn btn-icon update-proxy-config"
          type="button"
          title="配置网络代理"
          aria-label="配置网络代理"
          aria-haspopup="dialog"
          :disabled="networkLocked"
          @click="proxyDialogOpen = true"
        >
          <Icon name="settings" :size="16" />
        </button>
      </div>
      <p v-if="networkError" class="settings-error" role="alert">{{ networkError }}</p>
    </section>
    <UpdateProxyDialog v-if="proxyDialogOpen" @close="proxyDialogOpen = false" />
  </div>
</template>
