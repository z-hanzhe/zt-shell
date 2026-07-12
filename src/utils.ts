/**
 * 通用格式化与工具函数
 */

/** 将字节数格式化为易读字符串（如 1.5 GB） */
export function formatBytes(bytes: number, decimals = 1): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  const value = bytes / Math.pow(1024, i);
  return `${value.toFixed(i === 0 ? 0 : decimals)} ${units[i]}`;
}

/** 将字节速率格式化为易读字符串（如 1.2 MB/s） */
export function formatRate(bytesPerSec: number): string {
  return `${formatBytes(bytesPerSec)}/s`;
}

/** 紧凑字节格式（FinalShell 风格，如 1.2G、681M、329K），无空格单字母单位 */
export function formatShort(bytes: number): string {
  if (bytes <= 0) return "0";
  const units = ["B", "K", "M", "G", "T", "P"];
  // 下限 0 上限最后一档，避免小于 1 字节时索引为负取到 undefined
  const i = Math.min(Math.max(Math.floor(Math.log(bytes) / Math.log(1024)), 0), units.length - 1);
  const value = bytes / Math.pow(1024, i);
  // 大于等于 100 或整数不带小数，否则保留一位
  const str = value >= 100 || i === 0 ? String(Math.round(value)) : value.toFixed(1);
  return `${str}${units[i]}`;
}

/** 紧凑速率格式（如 329K，用于网络上下行） */
export function formatShortRate(bytesPerSec: number): string {
  return formatShort(bytesPerSec);
}

/** 将运行秒数格式化为天/时/分 */
export function formatUptime(seconds: number): string {
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const parts: string[] = [];
  if (d > 0) parts.push(`${d} 天`);
  if (h > 0) parts.push(`${h} 小时`);
  parts.push(`${m} 分钟`);
  return parts.join(" ");
}

/** 将 Unix 时间戳格式化为本地日期时间 */
export function formatTime(timestamp: number): string {
  if (!timestamp) return "-";
  const date = new Date(timestamp * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(
    date.getDate()
  )} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** 生成随机唯一标识 */
export function genId(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

/** 拼接远端路径（始终使用正斜杠） */
export function joinPath(base: string, name: string): string {
  if (base === "/") return `/${name}`;
  return `${base.replace(/\/$/, "")}/${name}`;
}

/** 获取路径的父目录 */
export function parentPath(path: string): string {
  if (path === "/" || !path.includes("/")) return "/";
  const trimmed = path.replace(/\/$/, "");
  const idx = trimmed.lastIndexOf("/");
  return idx <= 0 ? "/" : trimmed.slice(0, idx);
}
