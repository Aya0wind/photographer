import { ipc } from "../index";
import { INVOKE_UNAVAILABLE_PATTERN } from "./errors";
import type {
  DatabaseCreateResult,
  DatabaseEntry,
  DatabaseList,
  DatabaseRemoveResult,
} from "./types";

// --- 数据库注册表（2026-10-09 多数据库修正；后端 ipc/databases.rs 一一对应） ----------
// database_list / create / switch / remove：变更统一走 databasesChanged 事件
// （前端全量刷新库内数据——换库语义）。

/** 空快照（命令失败/后端不可用的兜底形态；门禁按「无数据库」处理） */
const EMPTY_LIST: DatabaseList = { databases: [], activeId: null };

/** 注册表快照（database_list）；失败/形状异常回退空快照——UI 自然降级 */
export async function databaseList(): Promise<DatabaseList> {
  try {
    const list = await ipc<DatabaseList | null>("database_list");
    if (list === null || typeof list !== "object" || !Array.isArray(list.databases)) {
      return EMPTY_LIST;
    }
    return list;
  } catch {
    return EMPTY_LIST;
  }
}

/**
 * 新建数据库（database_create）：名称必填 + 位置可选（缺省 = 后端约定路径
 * `<应用配置目录>/databases/<id>`；首个库自动激活）。业务错误（相对路径/
 * 与已有数据库或照片库根重叠等）透传原始 Err 文案；invoke 不可用 error=null。
 */
export async function databaseCreate(
  name: string,
  dbDir?: string,
): Promise<DatabaseCreateResult> {
  try {
    const database = await ipc<DatabaseEntry>("database_create", {
      name,
      dbDir: dbDir?.trim() ? dbDir.trim() : null,
    });
    if (database === null || typeof database !== "object" || typeof database.id !== "string") {
      return { ok: false, error: null };
    }
    return { ok: true, database };
  } catch (err) {
    const message = err instanceof Error ? err.message : typeof err === "string" ? err : "";
    if (!message || INVOKE_UNAVAILABLE_PATTERN.test(message)) {
      return { ok: false, error: null };
    }
    return { ok: false, error: message };
  }
}

/**
 * 切换激活数据库（database_switch）：一切库内操作（照片库/画廊/相册）随之
 * 换库。不 catch：失败文案（数据库不存在/不可用/导入进行中）透传给调用方。
 * 成功后端发 databasesChanged，前端全量刷新。
 */
export async function databaseSwitch(id: string): Promise<void> {
  await ipc<void>("database_switch", { id });
}

/**
 * 移除数据库（database_remove）：deleteData=false 仅摘登记（数据目录留在
 * 磁盘）；true 连数据目录删（后端有照片库根重叠安全闸，**绝不删照片库
 * 文件夹**）。不 catch：失败文案透传给调用方提示。
 */
export async function databaseRemove(
  id: string,
  deleteData: boolean,
): Promise<DatabaseRemoveResult> {
  return ipc<DatabaseRemoveResult>("database_remove", { id, deleteData });
}
