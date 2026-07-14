/**
 * 传输 store：维护上传/下载任务列表并监听后端进度事件
 *
 * 后端通过两类事件推送：
 * - transfer://changed 任务结构变化（创建/删除），载荷为全量任务列表
 * - transfer://progress 动态字段增量（进度/速度/状态/耗时），按 id 合并
 */

import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { listen } from "@tauri-apps/api/event";
import type { TransferProgress, TransferTask } from "../types";
import { transferList } from "../api";

export const useTransfersStore = defineStore("transfers", () => {
  /** 全部任务（后端创建顺序，父任务先于子任务），按 sessionId 区分所属会话 */
  const tasks = ref<TransferTask[]>([]);
  /** 上传任务完成计数，文件管理器据此感知刷新时机 */
  const uploadDoneTick = ref(0);
  /** 最近完成上传任务所属的会话标识，供文件管理器过滤非当前会话的完成事件 */
  const uploadDoneSession = ref("");
  /** 是否已初始化监听 */
  let started = false;

  /** id 到任务的映射，用于增量合并 */
  const byId = computed(() => {
    const map = new Map<string, TransferTask>();
    for (const t of tasks.value) map.set(t.id, t);
    return map;
  });

  /** 初始化事件监听并拉取当前任务列表（仅 Tauri 环境下有效） */
  async function init() {
    if (started) return;
    started = true;
    await listen<TransferTask[]>("transfer://changed", (event) => {
      tasks.value = event.payload;
    });
    await listen<TransferProgress[]>("transfer://progress", (event) => {
      // 通过响应式数组元素的代理对象修改，确保视图刷新
      const map = byId.value;
      for (const update of event.payload) {
        const task = map.get(update.id);
        if (!task) continue;
        // 上传任务转为完成时递增信号，供文件管理器刷新目录
        if (task.kind === "upload" && task.status !== "completed" && update.status === "completed") {
          uploadDoneSession.value = task.sessionId;
          uploadDoneTick.value++;
        }
        task.status = update.status;
        task.transferred = update.transferred;
        task.total = update.total;
        task.speed = update.speed;
        task.etaSecs = update.etaSecs;
        task.elapsedMs = update.elapsedMs;
        task.error = update.error;
      }
    });
    tasks.value = await transferList();
  }

  return { tasks, byId, uploadDoneTick, uploadDoneSession, init };
});
