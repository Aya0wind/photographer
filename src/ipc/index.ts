import { invoke } from "@tauri-apps/api/core";
import type { InvokeArgs } from "@tauri-apps/api/core";

// --- IPC 可用性标志（自愈式） ---------------------------------------------------
// 桥接故障置 false；成功和后端返回的业务错误均说明后端可达：一次瞬时失败（如 dev 重启竞态）
// 不应把整个会话永久钉死在"预览模式"（真实案例：app 自动重启窗口期一次失败
// 后所有功能正常但横幅不消失）。标志由本入口统一维护，命令封装不再各自置位。

let ipcAvailable = true;

/** 当前 IPC 是否可用；UI 用于降级提示（如"后端未连接"） */
export function isIpcAvailable(): boolean {
  return ipcAvailable;
}

/** 仅测试用：恢复初始"可用"状态 */
export function resetIpcAvailable(): void {
  ipcAvailable = true;
}

/**
 * 统一的 Tauri IPC 调用入口。
 * 所有 Rust 命令调用都应经由本函数：统一可用性跟踪与错误留痕，
 * 失败时向上抛出（调用方自行决定回退值）。
 */
export async function ipc<T>(cmd: string, payload?: unknown): Promise<T> {
  try {
    const result = await invoke<T>(cmd, payload as InvokeArgs);
    ipcAvailable = true;
    return result;
  } catch (err) {
    // Rust command 返回 String 错误本身就证明后端已响应；不能把设备/数据库
    // 业务错误误报为整个后端离线。桥接缺失/通道中断才标记不可用。
    ipcAvailable = typeof err === "string" && !/__TAURI_INTERNALS__|invoke is not available|command .*not found|channel.*closed|failed to fetch|ipc.*disconnect/i.test(err);
    console.error(`[ipc] ${cmd} failed:`, err);
    throw err;
  }
}
