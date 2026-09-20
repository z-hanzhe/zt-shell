<script setup lang="ts">
/** 分组设置表单：保存反馈留在页面中，离开时恢复已保存的缩放。 */
import { computed, onBeforeUnmount, reactive, ref, watch } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import Icon from "./Icon.vue";
import AppDialog from "./AppDialog.vue";
import { pathIsDir } from "../api";
import { createDefaultAppSettings, useSettingsStore, type AppSettings } from "../stores/settings";
import { UI_SCALE_OPTIONS } from "../uiScale";
import { MAX_EDITOR_FONT_SIZE, MIN_EDITOR_FONT_SIZE } from "../editorProtocol";

const props = defineProps<{
  active: boolean;
  saveSettings: (settings: AppSettings) => Promise<void>;
}>();
const emit = defineEmits<{ (event: "preview-ui-scale", scale: number): void }>();
const store = useSettingsStore();
const form = reactive<AppSettings>({ ...store.settings });
const submitting = ref(false);
const resetting = ref(false);
const resetConfirmOpen = ref(false);
const message = ref("");
const failed = ref(false);
const dirty = computed(() => JSON.stringify(form) !== JSON.stringify(store.settings));

watch(() => store.settings, (value) => Object.assign(form, value));
watch(() => [props.active, form.uiScale] as const, ([active, scale]) => {
  emit("preview-ui-scale", active ? scale : store.settings.uiScale);
});
onBeforeUnmount(() => emit("preview-ui-scale", store.settings.uiScale));

/** 放弃尚未保存的修改并恢复缩放。 */
function cancel(): void {
  Object.assign(form, store.settings);
  message.value = "";
}

/** 将默认值写入表单，等待用户手动保存。 */
async function confirmReset(): Promise<void> {
  resetConfirmOpen.value = false;
  resetting.value = true;
  try {
    Object.assign(form, await createDefaultAppSettings());
    message.value = "默认设置已填入，点击保存后生效";
    failed.value = false;
  } catch (error) {
    message.value = `读取默认设置失败：${String(error)}`;
    failed.value = true;
  } finally {
    resetting.value = false;
  }
}

/** 打开系统目录选择框并回填下载路径。 */
async function selectDownloadPath(): Promise<void> {
  try {
    const picked = await openDialog({ directory: true, title: "选择下载路径", defaultPath: form.downloadPath.trim() || undefined });
    if (typeof picked === "string") form.downloadPath = picked;
  } catch (error) {
    message.value = `选择下载路径失败：${String(error)}`;
    failed.value = true;
  }
}

/** 校验设置并等待持久化完成后显示保存结果。 */
async function submit(): Promise<void> {
  if (submitting.value || resetting.value) return;
  submitting.value = true;
  message.value = "";
  try {
    const downloadPath = form.downloadPath.trim();
    if (!downloadPath) throw new Error("请填写下载路径");
    if (isTauri() && !(await pathIsDir(downloadPath))) throw new Error("下载路径不存在或不是文件夹，请重新选择");
    if (!form.fontFamily.trim()) throw new Error("请填写字体");
    for (const [value, min, max, label] of [
      [form.fontSize, 8, 32, "终端字号"],
      [form.editorFontSize, MIN_EDITOR_FONT_SIZE, MAX_EDITOR_FONT_SIZE, "编辑器字号"],
      [form.monitorInterval, 1, 30, "监控间隔"],
    ] as const) {
      if (!Number.isInteger(value) || value < min || value > max) throw new Error(`${label}须为 ${min} 至 ${max} 的整数`);
    }
    await props.saveSettings({ ...form, downloadPath });
    message.value = "设置已保存";
    failed.value = false;
  } catch (error) {
    message.value = String(error);
    failed.value = true;
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <form class="settings-page" @submit.prevent="submit">
    <header class="settings-page-header">
      <div><h1>设置</h1><p>让每一次连接，都更合你的习惯。</p></div>
      <span v-if="dirty" class="settings-badge">未保存</span>
    </header>

    <section class="settings-card">
      <h2><Icon name="panelLeft" :size="17" />界面与编辑</h2>
      <label class="settings-row">
        <span class="settings-label">界面缩放<small>即时预览，保存后应用到所有窗口</small></span>
        <select v-model.number="form.uiScale" class="input settings-control">
          <option v-for="option in UI_SCALE_OPTIONS" :key="option.value" :value="option.value">{{ option.label }}</option>
        </select>
      </label>
      <label class="settings-row">
        <span class="settings-label">编辑器字号<small>远端文本编辑器的默认文字大小</small></span>
        <div class="settings-number"><input v-model.number="form.editorFontSize" class="input" type="number" :min="MIN_EDITOR_FONT_SIZE" :max="MAX_EDITOR_FONT_SIZE" required /><span>px</span></div>
      </label>
    </section>

    <section class="settings-card">
      <h2><Icon name="terminal" :size="17" />终端</h2>
      <label class="settings-row">
        <span class="settings-label">终端字号<small>调整终端输出和输入文字的大小</small></span>
        <div class="settings-number"><input v-model.number="form.fontSize" class="input" type="number" min="8" max="32" required /><span>px</span></div>
      </label>
      <label class="settings-row settings-row-wide">
        <span class="settings-label">字体<small>可填写多个字体名称，按顺序使用可用字体</small></span>
        <input v-model="form.fontFamily" class="input settings-control" required />
      </label>
      <label class="settings-row">
        <span class="settings-label">光标闪烁<small>让输入位置更容易辨认</small></span>
        <span class="settings-toggle"><input v-model="form.cursorBlink" type="checkbox" role="switch" /><span>{{ form.cursorBlink ? "启用" : "关闭" }}</span></span>
      </label>
    </section>

    <section class="settings-card">
      <h2><Icon name="folder" :size="17" />下载与监控</h2>
      <label class="settings-row settings-row-wide">
        <span class="settings-label">下载路径<small>远端文件下载到本机时的默认目录</small></span>
        <span class="settings-path">
          <input v-model="form.downloadPath" class="input" placeholder="请选择本地下载目录" required />
          <button class="btn btn-icon" type="button" title="选择下载路径" aria-label="选择下载路径" @click="selectDownloadPath"><Icon name="folder" :size="15" /></button>
        </span>
      </label>
      <label class="settings-row">
        <span class="settings-label">监控间隔(秒)<small>CPU、内存和网络数据的刷新频率</small></span>
        <div class="settings-number"><input v-model.number="form.monitorInterval" class="input" type="number" min="1" max="30" required /><span>秒</span></div>
      </label>
    </section>

    <footer class="settings-savebar">
      <button class="btn" type="button" :disabled="submitting || resetting" @click="resetConfirmOpen = true">重置</button>
      <span class="settings-feedback" :class="{ error: failed }" role="status">{{ message }}</span>
      <button class="btn" type="button" :disabled="!dirty || submitting || resetting" @click="cancel">取消</button>
      <button class="btn btn-primary" type="submit" :disabled="!dirty || submitting || resetting">{{ submitting ? "保存中" : "保存" }}</button>
    </footer>
  </form>
  <AppDialog :open="resetConfirmOpen" type="confirm" title="恢复默认设置" message="确定要恢复为默认设置吗？确认后仍需点击保存才会生效。" confirm-text="恢复默认" @confirm="confirmReset" @cancel="resetConfirmOpen = false" />
</template>
