import { ipc } from "../index";
import {
  type CameraCaptureResult,
  type CameraInfo,
  type TetherCameraSetting,
  type TetherCaptureResult,
  type TetherSessionDto,
  type TetherSettingKind,
  type TetherStartResult,
} from "./types";
import { INVOKE_UNAVAILABLE_PATTERN } from "./errors";

/** 脏数据容错：后端载荷 → CameraInfo 归一（形状异常返回 null，调用方按未探测处理） */
function normalizeCameraInfo(value: unknown): CameraInfo | null {
  if (value === null || typeof value !== "object") return null;
  const r = value as Record<string, unknown>;
  if (typeof r.pnpId !== "string" || r.pnpId === "") return null;
  const caps =
    r.capabilities !== null && typeof r.capabilities === "object"
      ? (r.capabilities as Record<string, unknown>)
      : {};
  const cap = (v: unknown): boolean => v === true;
  return {
    pnpId: r.pnpId,
    name: typeof r.name === "string" && r.name !== "" ? r.name : r.pnpId,
    capabilities: {
      fileTransfer: cap(caps.fileTransfer),
      standardCapture: cap(caps.standardCapture),
      vendorCaptureNikon: cap(caps.vendorCaptureNikon),
      objectAddedEvents: cap(caps.objectAddedEvents),
      liveView: cap(caps.liveView),
    },
  };
}

/** 已连接相机清单（tethering_camera_list；失败/非数组/条目异常回退 []） */
export async function tetheringCameraList(): Promise<CameraInfo[]> {
  try {
    const list = await ipc<unknown>("tethering_camera_list");
    if (!Array.isArray(list)) return [];
    return list.map(normalizeCameraInfo).filter((c): c is CameraInfo => c !== null);
  } catch {
    return [];
  }
}

/** 探测单台相机联拍能力（camera_probe）；失败/形状异常返回 null（UI 显示「未探测」，
 *  不显示拍摄入口——后端在途时自然降级） */
export async function cameraProbe(pnpId: string): Promise<CameraInfo | null> {
  try {
    return normalizeCameraInfo(await ipc<unknown>("camera_probe", { pnpId }));
  } catch {
    return null;
  }
}

/** 脏数据容错：未知形状 → null（调用方按会话已结束处理）。 */
function normalizeTetherSession(value: unknown): TetherSessionDto | null {
  if (value === null || typeof value !== "object") return null;
  const r = value as Record<string, unknown>;
  if (typeof r.id !== "string" || typeof r.albumId !== "number") return null;
  const camera = normalizeCameraInfo(r.camera);
  if (camera === null) return null;
  return {
    id: r.id,
    libraryId: typeof r.libraryId === "string" ? r.libraryId : "",
    albumId: r.albumId,
    albumName: typeof r.albumName === "string" ? r.albumName : "",
    camera,
    settings: normalizeTetherSettings(r.settings),
    photos: Array.isArray(r.photos)
      ? r.photos
          .filter((p): p is Record<string, unknown> => p !== null && typeof p === "object")
          .filter((p) => typeof p.id === "number" && typeof p.name === "string")
          .map((p) => ({ id: p.id as number, name: p.name as string, kind: typeof p.kind === "string" ? p.kind as string : "photo" }))
      : [],
    connected: r.connected === true,
    receiving: r.receiving === true,
    error: typeof r.error === "string" && r.error !== "" ? r.error : null,
  };
}

const TETHER_SETTING_KINDS = ["choice", "toggle", "range", "action", "text"] as const;

function normalizeTetherSettings(value: unknown): TetherCameraSetting[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter((s): s is Record<string, unknown> => s !== null && typeof s === "object")
    .filter((s) => typeof s.id === "string" && typeof s.current === "string")
    .map((s) => {
      const kindRaw = typeof s.kind === "string" ? s.kind : "";
      const kind: TetherSettingKind = (TETHER_SETTING_KINDS as readonly string[]).includes(kindRaw)
        ? (kindRaw as TetherSettingKind)
        : "choice";
      const num = (v: unknown): number | undefined =>
        typeof v === "number" && Number.isFinite(v) ? v : undefined;
      return {
        id: s.id as string,
        label: typeof s.label === "string" ? s.label : (s.id as string),
        kind,
        current: s.current as string,
        writable: s.writable === true,
        options: Array.isArray(s.options)
          ? s.options
              .filter((o): o is Record<string, unknown> => o !== null && typeof o === "object")
              .filter((o) => typeof o.value === "string")
              .map((o) => ({
                value: o.value as string,
                label: typeof o.label === "string" ? o.label : (o.value as string),
              }))
          : [],
        min: num(s.min),
        max: num(s.max),
        step: num(s.step),
      };
    });
}

/** 开启联拍会话并打开独立拍摄窗口（tethering_start）：后端创建 `tethering` 窗口
 *  （窗口关闭即自动结束会话）。业务错误（相机被占/已有会话/相册不存在）透传。 */
export async function tetheringStart(albumId: number, cameraId: string): Promise<TetherStartResult> {
  try {
    const raw = await ipc<unknown>("tethering_start", { albumId, cameraId });
    const session = normalizeTetherSession(raw);
    if (session === null) return { ok: false, error: null };
    return { ok: true, session };
  } catch (err) {
    const message = err instanceof Error ? err.message : typeof err === "string" ? err : "";
    if (!message || INVOKE_UNAVAILABLE_PATTERN.test(message)) return { ok: false, error: null };
    return { ok: false, error: message };
  }
}

/** 拉取会话快照（tethering_session）；会话已结束/形状异常返回 null。 */
export async function tetheringSession(sessionId: string): Promise<TetherSessionDto | null> {
  try {
    return normalizeTetherSession(await ipc<unknown>("tethering_session", { sessionId }));
  } catch {
    return null;
  }
}

/** 刷新拍摄参数（tethering_settings）；失败返回 null（UI 保持旧值）。 */
export async function tetheringSettings(sessionId: string): Promise<TetherCameraSetting[] | null> {
  try {
    const raw = await ipc<unknown>("tethering_settings", { sessionId });
    return normalizeTetherSettings(raw);
  } catch {
    return null;
  }
}

/** 设置拍摄参数（tethering_setting_set）并返回刷新后的全量参数。
 *  不 catch：业务错误（参数无效/相机拒绝）文案透传给调用方。 */
export async function tetheringSettingSet(
  sessionId: string,
  id: string,
  value: string,
): Promise<TetherCameraSetting[]> {
  return normalizeTetherSettings(
    await ipc<unknown>("tethering_setting_set", { sessionId, id, value }),
  );
}

/** 点击取景画面对焦（tethering_focus_at）：坐标为归一化 live view 坐标
 *  （0..1，原点左上）。业务错误透传文案。 */
export async function tetheringFocusAt(sessionId: string, x: number, y: number): Promise<void> {
  await ipc<unknown>("tethering_focus_at", { sessionId, x, y });
}

/** 按快门（tethering_capture）：触发拍摄 + 同步等待收片入册。
 *  业务错误透传；invoke 不可用 error=null（通用文案）。 */
export async function tetheringCapture(sessionId: string): Promise<TetherCaptureResult> {
  try {
    await ipc<unknown>("tethering_capture", { sessionId });
    return { ok: true };
  } catch (err) {
    const message = err instanceof Error ? err.message : typeof err === "string" ? err : "";
    if (!message || INVOKE_UNAVAILABLE_PATTERN.test(message)) return { ok: false, error: null };
    return { ok: false, error: message };
  }
}

/** 取一帧实时取景（tethering_frame）：JPEG data URL；相机忙（拍摄/收片中）或
 *  不支持 live view 时返回 null（UI 保持上一帧不闪烁）。 */
export async function tetheringFrame(sessionId: string): Promise<string | null> {
  try {
    const value = await ipc<unknown>("tethering_frame", { sessionId });
    return typeof value === "string" && value.startsWith("data:image/") ? value : null;
  } catch {
    return null;
  }
}

/** 会话内照片预览（tethering_photo_preview；size>512 取 2048 档）：缩略图
 *  data URL；尚未生成/失败返回 null。 */
export async function tetheringPhotoPreview(
  sessionId: string,
  assetId: number,
  size: number,
): Promise<string | null> {
  try {
    const value = await ipc<unknown>("tethering_photo_preview", { sessionId, assetId, size });
    return typeof value === "string" && value.startsWith("data:image/") ? value : null;
  } catch {
    return null;
  }
}

/** 结束会话（tethering_stop）：幂等；失败静默（窗口关闭路径后端兜底）。 */
export async function tetheringStop(sessionId: string): Promise<void> {
  try {
    await ipc<unknown>("tethering_stop", { sessionId });
  } catch {
    // 会话已结束/后端在途：无操作
  }
}

/** 触发拍摄（camera_capture；timeoutMs 可选，缺省用后端默认）。
 *  业务错误（如设备被占用）透传 Err 文案；invoke 不可用时 error=null（通用文案）。 */
export async function cameraCapture(pnpId: string, timeoutMs?: number): Promise<CameraCaptureResult> {
  const payload: Record<string, unknown> = { pnpId };
  if (timeoutMs !== undefined) payload.timeoutMs = timeoutMs;
  try {
    const raw = await ipc<unknown>("camera_capture", payload);
    if (raw === null || typeof raw !== "object") {
      return { objectName: null, objectSize: null, error: null };
    }
    const r = raw as Record<string, unknown>;
    return {
      objectName: typeof r.objectName === "string" && r.objectName !== "" ? r.objectName : null,
      objectSize:
        typeof r.objectSize === "number" && Number.isFinite(r.objectSize) ? r.objectSize : null,
      error: typeof r.error === "string" && r.error !== "" ? r.error : null,
    };
  } catch (err) {
    const message = err instanceof Error ? err.message : typeof err === "string" ? err : "";
    if (!message || INVOKE_UNAVAILABLE_PATTERN.test(message)) {
      return { objectName: null, objectSize: null, error: null };
    }
    return { objectName: null, objectSize: null, error: message };
  }
}
