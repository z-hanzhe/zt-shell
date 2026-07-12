/**
 * 监控 store：按会话维度持续采集并缓存监控数据
 *
 * 监控随会话建立启动、随会话关闭停止，与当前激活的选项卡无关。
 * 这样来回切换选项卡时始终能看到该会话最近一次的数据，不会清空重采导致刷新。
 */

import { defineStore } from "pinia";
import { reactive } from "vue";
import { monitorCollect } from "../api";
import type { MonitorData, NetInterface } from "../types";
import { useSettingsStore } from "./settings";

/** 网卡历史采样点（收发速率，字节/秒） */
export interface NetSample {
  rx: number;
  tx: number;
}

/** 网卡历史保留的采样点上限 */
const NET_HISTORY_MAX = 40;
/** 自动选网卡的观察样本数：达到后若有流量则锁定，避免早期抖动 */
const AUTO_LOCK_AFTER = 3;

/** 单个会话的监控运行时状态 */
export interface MonitorState {
  /** 最近一次采集到的数据，未采集到为 null */
  data: MonitorData | null;
  /** 采集错误信息 */
  error: string;
  /** 所有网卡的收发速率历史（按网卡名存），切换网卡无需从头积攒 */
  netHistories: Record<string, NetSample[]>;
  /** 当前展示的网卡名 */
  netName: string;
  /** 用户是否手动选过网卡（选过则不再自动切换） */
  netPinned: boolean;
  /** 自动选择是否已锁定（锁定后不再自动跳动） */
  netAutoLocked: boolean;
  /** 已采集样本数 */
  sampleCount: number;
  /** 采集定时器句柄 */
  timer: number | null;
}

export const useMonitorStore = defineStore("monitor", () => {
  /** 会话标识 → 监控状态 */
  const states = reactive<Record<string, MonitorState>>({});

  /** 确保会话状态存在 */
  function ensure(id: string): MonitorState {
    if (!states[id]) {
      states[id] = {
        data: null,
        error: "",
        netHistories: {},
        netName: "",
        netPinned: false,
        netAutoLocked: false,
        sampleCount: 0,
        timer: null,
      };
    }
    return states[id];
  }

  /**
   * 自动挑选默认网卡：
   * 1. 只在物理网卡中选（无物理网卡才退回全部网卡）
   * 2. 取历史累计流量最高者（等观察若干样本后再据此决定）
   * 3. 都无流量时取第一个
   * 返回 { name, lock }，lock 为 true 表示可锁定不再跳动
   */
  function pickDefaultNet(
    nics: NetInterface[],
    histories: Record<string, NetSample[]>,
    sampleCount: number
  ): { name: string; lock: boolean } {
    if (!nics.length) return { name: "", lock: false };
    const phys = nics.filter((n) => n.isPhysical);
    const pool = phys.length ? phys : nics;
    const score = (n: NetInterface) =>
      (histories[n.name] ?? []).reduce((a, s) => a + s.rx + s.tx, 0);
    // 按累计流量降序（稳定排序，都为 0 时保持原顺序即取第一个）
    const ranked = [...pool].sort((a, b) => score(b) - score(a));
    const best = ranked[0];
    // 观察够样本且已有流量才锁定
    const lock = score(best) > 0 && sampleCount >= AUTO_LOCK_AFTER;
    return { name: best.name, lock };
  }

  /** 采集一次并写入状态 */
  async function poll(id: string) {
    const s = states[id];
    if (!s) return;
    try {
      const d = await monitorCollect(id);
      s.data = d;
      s.error = "";
      s.sampleCount++;

      // 记录所有网卡历史（切换网卡时直接有数据，无需重新积攒）
      for (const n of d.netInterfaces) {
        const h = s.netHistories[n.name] ?? (s.netHistories[n.name] = []);
        h.push({ rx: n.rxRate, tx: n.txRate });
        if (h.length > NET_HISTORY_MAX) h.shift();
      }

      // 当前网卡消失则重新进入自动选择
      const exists = d.netInterfaces.some((n) => n.name === s.netName);
      if (!exists) {
        s.netPinned = false;
        s.netAutoLocked = false;
      }
      // 未手动固定且未锁定时，持续自动挑选直至锁定
      if (!s.netPinned && !s.netAutoLocked) {
        const picked = pickDefaultNet(d.netInterfaces, s.netHistories, s.sampleCount);
        s.netName = picked.name;
        if (picked.lock) s.netAutoLocked = true;
      }
    } catch (e) {
      s.error = String(e);
    }
  }

  /** 会话建立后启动持续采集（幂等，重复调用不会重复起定时器） */
  function start(id: string) {
    const s = ensure(id);
    if (s.timer !== null) return;
    const interval = useSettingsStore().settings.monitorInterval * 1000;
    poll(id);
    s.timer = window.setInterval(() => poll(id), interval);
  }

  /** 会话关闭时停止采集并清除状态 */
  function stop(id: string) {
    const s = states[id];
    if (s?.timer != null) window.clearInterval(s.timer);
    delete states[id];
  }

  /** 读取指定会话的监控状态，不存在返回 null */
  function state(id: string): MonitorState | null {
    return states[id] ?? null;
  }

  /** 手动切换网卡：固定选择（历史已在采集所有网卡，无需清空） */
  function setNetName(id: string, name: string) {
    const s = states[id];
    if (s) {
      s.netName = name;
      s.netPinned = true;
      s.netAutoLocked = true;
    }
  }

  return { states, start, stop, state, setNetName };
});
