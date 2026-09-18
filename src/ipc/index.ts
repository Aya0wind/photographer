import { invoke } from "@tauri-apps/api/core";
import type { InvokeArgs } from "@tauri-apps/api/core";

/**
 * 统一的 Tauri IPC 调用入口。
 * 所有 Rust 命令调用都应经由本函数，便于后续统一追加日志 / 错误规整。
 */
export async function ipc<T>(cmd: string, payload?: unknown): Promise<T> {
  return invoke<T>(cmd, payload as InvokeArgs);
}
