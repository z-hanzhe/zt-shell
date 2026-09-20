<script setup lang="ts">
/** 单例设置工作区，导航和内容区域共享工具页生命周期。 */
import { ref, watch } from "vue";
import Icon from "./Icon.vue";
import GeneralSettings from "./GeneralSettings.vue";
import UpdateSettings from "./UpdateSettings.vue";
import { useUpdatesStore } from "../stores/updates";
import type { AppSettings } from "../stores/settings";

defineProps<{ active: boolean; saveSettings: (settings: AppSettings) => Promise<void> }>();
const emit = defineEmits<{
  (event: "preview-ui-scale", scale: number): void;
  (event: "install-update"): void;
}>();
const section = ref<"general" | "updates">("general");
const updates = useUpdatesStore();
const contentRef = ref<HTMLElement | null>(null);

// 菜单切换回到内容顶部，避免沿用上一个页面的滚动位置。
watch(section, () => contentRef.value?.scrollTo({ top: 0 }));
</script>

<template>
  <div class="settings-workspace">
    <nav class="settings-nav" aria-label="设置菜单">
      <div class="settings-nav-title">偏好与更新</div>
      <button :class="{ selected: section === 'general' }" :aria-current="section === 'general' ? 'page' : undefined" @click="section = 'general'"><Icon name="settings" :size="16" />设置</button>
      <button :class="{ selected: section === 'updates' }" :aria-current="section === 'updates' ? 'page' : undefined" @click="section = 'updates'"><Icon name="refresh" :size="16" />检查更新<span v-if="updates.hasUpdate" class="update-dot" role="img" aria-label="有新版本"></span></button>
      <div class="settings-nav-footer">ZTShell<span v-if="updates.currentVersion">v{{ updates.currentVersion }}</span></div>
    </nav>
    <main ref="contentRef" class="settings-content">
      <div v-show="section === 'general'">
        <GeneralSettings :active="active && section === 'general'" :save-settings="saveSettings" @preview-ui-scale="emit('preview-ui-scale', $event)" />
      </div>
      <UpdateSettings v-show="section === 'updates'" @install-update="emit('install-update')" />
    </main>
  </div>
</template>

<style src="../styles/settings.css"></style>
