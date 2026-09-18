/** 展示层格式化助手（字节/速度/耗时/时间戳），任务中心与向导共用 */

const UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

/** 1024 进制字节数格式化："812 KB"、"1.5 GB"；非法输入返回 "—" */
export function formatBytes(bytes: number | null | undefined): string {
  if (typeof bytes !== "number" || !Number.isFinite(bytes) || bytes < 0) return "—";
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const text = unit === 0 ? String(Math.round(value)) : value.toFixed(value >= 100 ? 0 : 1);
  return `${text} ${UNITS[unit]}`;
}

/** 速度格式化（字节/秒 → MB/s 为主）："42.5 MB/s" */
export function formatSpeed(bytesPerSec: number | null | undefined): string {
  if (typeof bytesPerSec !== "number" || !Number.isFinite(bytesPerSec) || bytesPerSec < 0) {
    return "—";
  }
  if (bytesPerSec < 1024 * 1024) return `${formatBytes(bytesPerSec)}/s`;
  return `${(bytesPerSec / 1024 / 1024).toFixed(1)} MB/s`;
}

/** 毫秒耗时："860 ms"、"3.2 s"、"12分34秒"、"1时05分" */
export function formatDuration(ms: number | null | undefined): string {
  if (typeof ms !== "number" || !Number.isFinite(ms) || ms < 0) return "—";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const totalSeconds = Math.floor(ms / 1000);
  if (totalSeconds < 60) return `${(ms / 1000).toFixed(1)} s`;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  if (minutes < 60) return `${minutes}分${String(seconds).padStart(2, "0")}秒`;
  const hours = Math.floor(minutes / 60);
  return `${hours}时${String(minutes % 60).padStart(2, "0")}分`;
}

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

/** epoch 毫秒 → 本地 "YYYY-MM-DD HH:mm:ss"；非法输入返回 "—" */
export function formatDateTime(ts: number | null | undefined): string {
  if (typeof ts !== "number" || !Number.isFinite(ts) || ts <= 0) return "—";
  const d = new Date(ts);
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  );
}
