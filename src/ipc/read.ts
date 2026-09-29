import { ipc } from "./index";

/** 列表读取共用数组校验和空态回退；错误留痕及可用性仍由 ipc 维护。 */
export async function ipcList<T>(command: string, payload?: unknown): Promise<T[]> {
  try {
    const list = await ipc<T[] | null>(command, payload);
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}
