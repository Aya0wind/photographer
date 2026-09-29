import { useCallback, useEffect, useRef, useState } from "react";
import { useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { motion } from "motion/react";

import { motionInitial, TRANS, useMotionOn } from "@/lib/motion";

import {
  subscribeAppEvents,
  tetheringCapture,
  tetheringFocusAt,
  tetheringFrame,
  tetheringPhotoPreview,
  tetheringSession,
  tetheringSettingSet,
  tetheringSettings,
  tetheringStop,
  type TetherCameraSetting,
  type TetherSessionDto,
} from "@/ipc/api";

/**
 * 联机拍摄独立窗口（/tethering?session=<id>；后端 tethering_start 创建本窗口，
 * 窗口销毁即自动结束会话）。LR 式布局：
 * - 顶部：自绘标题栏（decorations=false）——相册名/相机名/连接状态 + 最小化/关闭
 * - 中央：实时取景（tethering_frame 轮询，帧率 15/30/60 档可调；不支持时回落最近一张成片）
 * - 右栏：相机参数（shutter/aperture/iso 下拉；tethering_setting_set 即时生效）
 * - 底部：本次会话胶片条（tetheringPhotoAdded 事件驱动 + 预览）
 * 会话态以后端为唯一真值（独立 webview 的 store 是空会话，不读主窗口状态）。
 */

/** 取景帧率档位（轮询间隔 = 1000/fps；相机/USB 带宽不足时实际帧率低于档位）。 */
const FRAME_RATE_STEPS = [15, 30, 60] as const;
const FPS_STORAGE_KEY = "tethering.frameFps";
const FPS_DEFAULT: number = 30;

function framePollMs(fps: number): number {
  return Math.round(1000 / fps);
}

function loadStoredFps(): number {
  const raw = window.localStorage.getItem(FPS_STORAGE_KEY);
  const parsed = raw === null ? Number.NaN : Number(raw);
  return (FRAME_RATE_STEPS as readonly number[]).includes(parsed) ? parsed : FPS_DEFAULT;
}

/** 后端契约的参数 id → 本地化标签（tether.param.<id>）；未知 id 用后端 label。 */
function settingLabel(id: string, fallback: string, t: (k: string) => string): string {
  const key = `tether.param.${id}`;
  const translated = t(key);
  return translated === key ? fallback : translated;
}

/** 面板排序：常用拍摄参数在前，其余按标签字母序，动作按钮最后。 */
const SETTING_ORDER = [
  "shutterspeed", "f-number", "iso", "expprogram", "exposuremetermode", "exposurecompensation",
  "whitebalance", "colortemperature", "imagequality", "capturemode", "focusmode", "focusarea",
  "imagestabilization", "flashmode", "shuttertype", "silentmode", "dro", "aspectratio",
  "imagesize", "pcsaveimgsize", "liveviewsettingeffect", "manualfocus", "focusmagnifier",
] as const;

/** 悬浮工具栏参数（拍摄模式 + 曝光三要素 + 对焦 + 白平衡）：常驻取景画面
 *  底部，随手可调不用翻面板。choice 形态才进工具栏（range/action 走右栏）。 */
const QUICK_IDS = ["expprogram", "shutterspeed", "f-number", "iso", "whitebalance", "focusmode", "focusarea"] as const;

/** 工具栏常驻例外：只读也保留展示——
 *  - expprogram（拍摄模式）：唯一能切回 M/A/S 的入口，藏了就被锁死在档位里；
 *  - f-number（光圈）：镜头环控制的机身常见，只读也要能看到当前值。 */
const QUICK_ALWAYS_SHOW = new Set<string>(["expprogram", "f-number"]);

/** 机身模式切换（A/S 档 ↔ M 档）会改变参数可写性——settings 轮询间隔：
 *  只读的快门在 A 档隐藏、切回 M 档要能自动回来，靠这轮询刷新快照。 */
const SETTINGS_POLL_MS = 3000;

/** 工具栏拖动位置持久化键（相对取景容器左上的像素）。 */
const QUICKBAR_POS_KEY = "tethering.quickbar.pos";

function orderSettings(settings: TetherCameraSetting[]): TetherCameraSetting[] {
  const rank = (s: TetherCameraSetting): number => {
    if (s.kind === "action") return SETTING_ORDER.length + 1;
    const i = SETTING_ORDER.indexOf(s.id as (typeof SETTING_ORDER)[number]);
    return i === -1 ? SETTING_ORDER.length : i;
  };
  return [...settings].sort((a, b) => {
    const d = rank(a) - rank(b);
    return d !== 0 ? d : a.label.localeCompare(b.label);
  });
}

/** 工具栏参数：拍摄模式/曝光三要素/对焦/白平衡；只读隐藏——模式与光圈例外。 */
function quickSettings(settings: TetherCameraSetting[]): TetherCameraSetting[] {
  return QUICK_IDS.map((id) => settings.find((s) => s.id === id))
    .filter((s): s is TetherCameraSetting => {
      if (s === undefined || s.kind !== "choice") return false;
      return s.writable || QUICK_ALWAYS_SHOW.has(s.id);
    });
}

/** 右栏参数：工具栏之外的**可调**参数（只读一律不显示，光圈例外在工具栏）。 */
function panelSettings(settings: TetherCameraSetting[]): TetherCameraSetting[] {
  const quick = new Set<string>(QUICK_IDS);
  return orderSettings(settings).filter((s) => !quick.has(s.id) && s.writable);
}

export default function TetheringWindowPage() {
  const { t } = useTranslation();
  const [params] = useSearchParams();
  const sessionId = params.get("session") ?? "";

  const [session, setSession] = useState<TetherSessionDto | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [frame, setFrame] = useState<string | null>(null);
  const [fps, setFps] = useState<number>(loadStoredFps);
  const [capturing, setCapturing] = useState(false);
  const [captureError, setCaptureError] = useState<string | null>(null);
  const [settingErrors, setSettingErrors] = useState<Record<string, string>>({});
  const [previews, setPreviews] = useState<Record<number, string>>({});
  const photosRef = useRef<number[]>([]);
  /** Range 滑条本地草稿（id → 未提交值；松手才下发，避免拖动连发）。 */
  const [rangeDrafts, setRangeDrafts] = useState<Record<string, string>>({});
  /** 点击对焦标记（归一化坐标 + 2s 自动消失）。 */
  const [focusMark, setFocusMark] = useState<{ x: number; y: number } | null>(null);
  /** 工具栏参数弹层：当前展开的设置 id（单开；点外部收起）。 */
  const [quickOpen, setQuickOpen] = useState<string | null>(null);
  const quickBarRef = useRef<HTMLDivElement | null>(null);
  /** 工具栏拖动位置（null = 默认底部居中；持久化 localStorage）。 */
  const [barPos, setBarPos] = useState<{ x: number; y: number } | null>(() => {
    try {
      const raw = window.localStorage.getItem(QUICKBAR_POS_KEY);
      if (raw === null) return null;
      const p = JSON.parse(raw) as { x?: unknown; y?: unknown };
      return typeof p.x === "number" && typeof p.y === "number" ? { x: p.x, y: p.y } : null;
    } catch {
      return null;
    }
  });
  /** AF 触发反馈（600ms 绿闪）。 */
  const [afFlash, setAfFlash] = useState(false);
  const motionOn = useMotionOn();

  const liveViewSupported = session?.camera.capabilities.liveView === true;
  const connected = session?.connected === true;
  const receiving = session?.receiving === true;
  const photos = session?.photos ?? [];

  // 工具栏弹层点外部收起（SelectionBar 同款契约）
  useEffect(() => {
    if (quickOpen === null) return;
    const onDown = (e: MouseEvent) => {
      if (quickBarRef.current !== null && e.target instanceof Node && !quickBarRef.current.contains(e.target)) {
        setQuickOpen(null);
      }
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [quickOpen]);

  // 参数快照轮询：机身档位切换（A/S/M…）改变参数可写性，快门/光圈等
  // 的显示隐藏依赖最新 writable；断连不轮询。
  useEffect(() => {
    if (sessionId === "" || !connected) return;
    const timer = window.setInterval(() => {
      void tetheringSettings(sessionId)
        .then((next) => {
          if (next !== null) setSession((prev) => (prev === null ? prev : { ...prev, settings: next }));
        })
        .catch(() => {
          /* 单次轮询失败静默：下轮再试 */
        });
    }, SETTINGS_POLL_MS);
    return () => window.clearInterval(timer);
  }, [sessionId, connected]);

  // 会话快照：挂载拉一次；photoAdded/status 事件增量合并（轻路径，不整页重拉）
  const mergeSession = useCallback((next: TetherSessionDto) => {
    setSession(next);
    const ids = next.photos.map((p) => p.id);
    photosRef.current = ids;
    setPreviews((prev) => {
      const keep: Record<number, string> = {};
      for (const id of ids) if (prev[id] !== undefined) keep[id] = prev[id];
      return keep;
    });
  }, []);

  useEffect(() => {
    if (sessionId === "") return;
    let cancelled = false;
    void tetheringSession(sessionId).then((dto) => {
      if (cancelled) return;
      setLoaded(true);
      if (dto !== null) mergeSession(dto);
    });
    return () => {
      cancelled = true;
    };
  }, [sessionId, mergeSession]);

  // 事件：新片入册（胶片条 + 预览拉取）/ 连接状态
  useEffect(() => {
    if (sessionId === "") return;
    let off: (() => void) | null = null;
    let cancelled = false;
    void subscribeAppEvents((event) => {
      if (event.type === "tetheringPhotoAdded" && event.sessionId === sessionId) {
        setSession((prev) =>
          prev === null || prev.photos.some((photo) => photo.id === event.assetId)
            ? prev
            : {
                ...prev,
                photos: [...prev.photos, { id: event.assetId, name: event.name, kind: "photo" }].slice(-64),
              },
        );
      } else if (event.type === "tetheringStatus" && event.sessionId === sessionId) {
        setSession((prev) =>
          prev === null ? prev : { ...prev, connected: event.connected, error: event.error },
        );
      }
    })
      .then((fn) => {
        if (cancelled) fn();
        else off = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      off?.();
    };
  }, [sessionId]);

  // 实时取景轮询：拿不到帧保持上一帧（不闪烁）；拍摄/收片期间后端返回 null
  useEffect(() => {
    if (sessionId === "" || !liveViewSupported || !connected) return;
    let stopped = false;
    let timer: number | null = null;
    const tick = () => {
      void tetheringFrame(sessionId).then((url) => {
        if (stopped) return;
        if (url !== null) setFrame(url);
        timer = window.setTimeout(tick, framePollMs(fps));
      });
    };
    tick();
    return () => {
      stopped = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [sessionId, liveViewSupported, connected, fps]);

  // 胶片条预览：为还没有预览的照片拉缩略图（新片到达后缩略图生成有延迟，重试 3 次）
  useEffect(() => {
    if (sessionId === "") return;
    let cancelled = false;
    const missing = photos.map((p) => p.id).filter((id) => previews[id] === undefined);
    if (missing.length === 0) return;
    const attempt = (id: number, round: number) => {
      if (cancelled) return;
      void tetheringPhotoPreview(sessionId, id, 256).then((url) => {
        if (cancelled || url === null) {
          if (!cancelled && round < 3) window.setTimeout(() => attempt(id, round + 1), 1500);
          return;
        }
        setPreviews((prev) => (prev[id] === undefined ? { ...prev, [id]: url } : prev));
      });
    };
    for (const id of missing.slice(-8)) attempt(id, 0);
    return () => {
      cancelled = true;
    };
  }, [sessionId, photos, previews]);

  async function closeWindow(): Promise<void> {
    if (sessionId !== "") await tetheringStop(sessionId);
    void getCurrentWindow().close();
  }

  async function applySetting(setting: TetherCameraSetting, value: string): Promise<void> {
    // 拍摄模式是档位逃生舱：相机上报只读也照常尝试（readonly 标志随机身
    // 状态翻转——休眠/档位切换后常翻只读；真拒绝会走错误提示，不锁死入口）
    if (sessionId === "" || (!setting.writable && setting.id !== "expprogram") || value === setting.current) {
      return;
    }
    setSettingErrors((prev) => {
      const next = { ...prev };
      delete next[setting.id];
      return next;
    });
    try {
      const next = await tetheringSettingSet(sessionId, setting.id, value);
      setSession((prev) => (prev === null ? prev : { ...prev, settings: next }));
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setSettingErrors((prev) => ({ ...prev, [setting.id]: message }));
    }
  }

  /** 点击取景画面对焦：坐标归一化下发 + 十字标记（2s 消失）。 */
  async function focusAtView(e: React.MouseEvent<HTMLDivElement>): Promise<void> {
    if (sessionId === "" || !liveViewSupported || !connected) return;
    const img = e.currentTarget.querySelector("img");
    if (img === null) return;
    const rect = img.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return;
    const x = (e.clientX - rect.left) / rect.width;
    const y = (e.clientY - rect.top) / rect.height;
    if (x < 0 || x > 1 || y < 0 || y > 1) return;
    setFocusMark({ x, y });
    window.setTimeout(() => setFocusMark(null), 2000);
    try {
      await tetheringFocusAt(sessionId, x, y);
    } catch {
      /* 相机不支持/模式不符：标记自然消失即可，不打断取景 */
    }
  }

  function selectFps(next: number): void {
    setFps(next);
    try {
      window.localStorage.setItem(FPS_STORAGE_KEY, String(next));
    } catch {
      /* localStorage 不可用（隐私模式等）：仅本次会话生效 */
    }
  }

  /** Range 滑条松手/移焦提交：草稿 ≠ 当前值才下发。 */
  function commitRange(setting: TetherCameraSetting): void {
    const draft = rangeDrafts[setting.id];
    if (draft === undefined || draft === setting.current) return;
    setRangeDrafts((prev) => {
      const next = { ...prev };
      delete next[setting.id];
      return next;
    });
    void applySetting(setting, draft);
  }

  async function shoot(): Promise<void> {
    if (sessionId === "" || capturing) return;
    setCapturing(true);
    setCaptureError(null);
    const result = await tetheringCapture(sessionId);
    if (!result.ok) setCaptureError(result.error ?? t("tether.captureFailed"));
    // 收片完成后用后端快照校准：事件延迟或错过订阅时也能立即显示新片。
    const next = await tetheringSession(sessionId);
    if (next !== null) mergeSession(next);
    setCapturing(false);
  }

  /** 工具栏 AF 触发：画面中心对焦（不支持/模式不符静默）+ 绿闪反馈。 */
  async function triggerAf(): Promise<void> {
    if (sessionId === "" || !connected) return;
    setAfFlash(true);
    window.setTimeout(() => setAfFlash(false), 600);
    try {
      await tetheringFocusAt(sessionId, 0.5, 0.5);
    } catch {
      /* 相机不支持：绿闪照常消失 */
    }
  }

  /** 工具栏拖动：grip 按下 → pointer 跟随（限取景容器内）→ 抬起持久化。 */
  function beginBarDrag(e: React.PointerEvent<HTMLButtonElement>): void {
    if (e.button !== 0) return;
    const bar = quickBarRef.current;
    if (bar === null || bar.parentElement === null) return;
    setQuickOpen(null);
    const offX = e.clientX - bar.getBoundingClientRect().left;
    const offY = e.clientY - bar.getBoundingClientRect().top;
    let last: { x: number; y: number } | null = null;
    const onMove = (ev: PointerEvent) => {
      const view = quickBarRef.current?.parentElement;
      const barEl = quickBarRef.current;
      if (view === null || view === undefined || barEl === null) return;
      const vr = view.getBoundingClientRect();
      const x = Math.min(Math.max(0, ev.clientX - vr.left - offX), Math.max(0, vr.width - barEl.offsetWidth));
      const y = Math.min(Math.max(0, ev.clientY - vr.top - offY), Math.max(0, vr.height - barEl.offsetHeight));
      last = { x, y };
      setBarPos(last);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      if (last !== null) {
        try {
          window.localStorage.setItem(QUICKBAR_POS_KEY, JSON.stringify(last));
        } catch {
          /* 持久化失败：本次会话内仍有效 */
        }
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }

  // --- 渲染 ----------------------------------------------------------------------------
  if (sessionId === "") {
    return (
      <div className="flex h-screen items-center justify-center bg-bg text-sm text-text-muted">
        {t("tether.sessionEnded")}
      </div>
    );
  }
  if (!loaded) {
    return <div className="h-screen bg-bg" data-testid="tether-loading" />;
  }
  if (session === null) {
    return (
      <div className="flex h-screen flex-col items-center justify-center gap-3 bg-bg" data-testid="tether-ended">
        <p className="text-sm text-text-secondary">{t("tether.sessionEnded")}</p>
        <button
          type="button"
          onClick={() => void getCurrentWindow().close()}
          className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110"
          data-testid="tether-ended-close"
        >
          {t("tether.close")}
        </button>
      </div>
    );
  }

  const lastPhoto = photos.length > 0 ? photos[photos.length - 1] : null;
  const mainImage = liveViewSupported ? frame : lastPhoto !== null ? (previews[lastPhoto.id] ?? null) : null;
  const quick = quickSettings(session.settings);
  const panel = panelSettings(session.settings);
  const quickError = quick.map((s) => settingErrors[s.id]).find(Boolean);

  return (
    <div className="flex h-screen flex-col bg-bg text-text-primary" data-testid="tether-window">
      {/* 自绘标题栏（decorations=false） */}
      <div className="relative flex h-10 shrink-0 select-none items-center gap-3 border-b border-edge bg-surface pl-3" data-testid="tether-titlebar">
        <div className="absolute inset-0" data-tauri-drag-region data-testid="tether-window-drag-region" />
        <span className="pointer-events-none relative text-xs font-semibold" data-testid="tether-title">{t("tether.windowTitle")}</span>
        <span className="pointer-events-none relative text-[11px] text-text-muted">
          {t("tether.albumTarget")}：{session.albumName} · {session.camera.name}
        </span>
        <span
          className={`pointer-events-none relative ml-2 rounded-full px-2 py-0.5 text-[10px] ${connected ? "bg-emerald-400/15 text-emerald-400" : "bg-red-400/15 text-red-400"}`}
          data-testid="tether-connection"
        >
          {connected ? (receiving ? t("tether.receiving") : t("tether.liveView")) : t("tether.disconnected")}
        </span>
        {liveViewSupported && (
          <div
            className="relative flex items-center overflow-hidden rounded-full border border-edge text-[10px] leading-none"
            role="group"
            aria-label={t("tether.fps")}
            data-testid="tether-fps"
          >
            {FRAME_RATE_STEPS.map((step) => (
              <button
                key={step}
                type="button"
                onClick={() => selectFps(step)}
                aria-pressed={fps === step}
                data-testid={`tether-fps-${step}`}
                className={`h-5 w-8 transition-colors ${
                  fps === step ? "bg-accent font-semibold text-black" : "text-text-muted hover:bg-panel"
                }`}
              >
                {step}
              </button>
            ))}
          </div>
        )}
        <div className="relative ml-auto flex items-center">
          <button
            type="button"
            onClick={() => void getCurrentWindow().minimize()}
            className="flex h-10 w-11 items-center justify-center text-text-muted transition-colors hover:bg-panel hover:text-text-primary"
            aria-label="minimize"
          >
            —
          </button>
          <button
            type="button"
            onClick={() => void closeWindow()}
            className="flex h-10 w-11 items-center justify-center text-text-muted transition-colors hover:bg-red-500 hover:text-white"
            aria-label="close"
            data-testid="tether-close"
          >
            ✕
          </button>
        </div>
      </div>

      {session.error !== null && (
        <div className="flex items-center justify-between gap-3 border-b border-red-400/30 bg-red-400/10 px-3 py-1.5 text-[11px] text-red-400" data-testid="tether-error-banner">
          <span className="truncate">{session.error}</span>
        </div>
      )}
      {!connected && (
        <div className="border-b border-amber-400/30 bg-amber-400/10 px-3 py-1.5 text-[11px] text-amber-300" data-testid="tether-disconnect-banner">
          {t("tether.disconnected")}
        </div>
      )}

      <div className="flex min-h-0 flex-1">
        {/* 中央：取景画面（点击画面对焦） */}
        <div
          className="relative flex min-w-0 flex-1 items-center justify-center bg-black"
          data-testid="tether-view"
          onClick={(e) => void focusAtView(e)}
        >
          {mainImage !== null ? (
            <img src={mainImage} alt="live view" className="max-h-full max-w-full object-contain" data-testid="tether-view-img" />
          ) : (
            <div className="flex flex-col items-center gap-2 text-xs text-white/40">
              <span className={liveViewSupported ? "animate-pulse" : undefined}>{liveViewSupported ? t("tether.liveViewWaiting") : t("tether.liveViewUnsupported")}</span>
              {!liveViewSupported && lastPhoto === null && <span className="text-[11px]">{t("tether.noPhotos")}</span>}
            </div>
          )}
          {focusMark !== null && (
            <motion.div
              className="pointer-events-none absolute h-6 w-6 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-accent/90 shadow-[0_0_0_2px_rgba(0,0,0,0.35)]"
              style={{ left: `${focusMark.x * 100}%`, top: `${focusMark.y * 100}%` }}
              initial={motionInitial(motionOn, { scale: 0.4, opacity: 0 })}
              animate={{ scale: 1, opacity: 1 }}
              transition={{ duration: 0.18, ease: "easeOut" }}
              data-testid="tether-focus-marker"
            >
              <span className="absolute left-1/2 top-1/2 h-3 w-0.5 -translate-x-1/2 -translate-y-1/2 bg-accent/90" />
              <span className="absolute left-1/2 top-1/2 h-0.5 w-3 -translate-x-1/2 -translate-y-1/2 bg-accent/90" />
            </motion.div>
          )}
          {capturing && (
            <div className="absolute inset-0 flex items-center justify-center bg-black/40" data-testid="tether-capturing">
              <span className="animate-pulse rounded-full bg-black/70 px-4 py-2 text-xs text-white">{t("tether.capturing")}</span>
            </div>
          )}
          {captureError !== null && (
            <motion.div
              className="absolute bottom-16 left-1/2 -translate-x-1/2 rounded-md bg-red-500/90 px-3 py-1.5 text-[11px] text-white"
              initial={motionInitial(motionOn, { y: 12, opacity: 0 })}
              animate={{ y: 0, opacity: 1 }}
              transition={TRANS.slide}
              data-testid="tether-capture-error"
            >
              {captureError}
            </motion.div>
          )}

          {/* 悬浮工具栏（SelectionBar 同款浮条视觉）：曝光三要素 / 对焦 / 白平衡
              常驻画面底部，随手可调不必翻右栏；AF 触发 + 快门随时可拍；
              grip 拖动可挪位（位置持久化）。
              不可调参数不进工具栏——拍摄模式与光圈例外（模式是切回
              M/A/S 的唯一入口，光圈常有镜头环控制的机身）。 */}
          <div
            ref={quickBarRef}
            className={`absolute z-20 ${barPos === null ? "bottom-3 left-1/2 w-max max-w-[calc(100%-2rem)] -translate-x-1/2" : ""}`}
            style={barPos === null ? undefined : { left: barPos.x, top: barPos.y }}
            onClick={(e) => e.stopPropagation()}
            data-testid="tether-quickbar"
          >
            {/* flex-wrap：放不下时控件折行而不是压缩成竖排文字 */}
            <div className="flex flex-wrap items-center justify-center gap-1 rounded-full border border-edge bg-surface/95 px-2.5 py-1.5 shadow-xl backdrop-blur">
              {/* 拖动把手 */}
              <button
                type="button"
                onPointerDown={beginBarDrag}
                className="flex h-7 w-6 shrink-0 cursor-grab touch-none items-center justify-center rounded-full text-text-muted/70 transition-colors hover:bg-panel hover:text-text-secondary active:cursor-grabbing"
                title={t("tether.quickbarDrag")}
                aria-label={t("tether.quickbarDrag")}
                data-testid="tether-quickbar-grip"
              >
                <svg viewBox="0 0 16 16" width="12" height="12" fill="currentColor" aria-hidden="true">
                  <circle cx="5.5" cy="3.5" r="1.2" /><circle cx="10.5" cy="3.5" r="1.2" />
                  <circle cx="5.5" cy="8" r="1.2" /><circle cx="10.5" cy="8" r="1.2" />
                  <circle cx="5.5" cy="12.5" r="1.2" /><circle cx="10.5" cy="12.5" r="1.2" />
                </svg>
              </button>
              {quick.map((setting) => (
                <div key={setting.id} className="relative shrink-0">
                  <button
                    type="button"
                    onClick={() => setQuickOpen((v) => (v === setting.id ? null : setting.id))}
                    disabled={(setting.id !== "expprogram" && !setting.writable) || !connected}
                    aria-expanded={quickOpen === setting.id}
                    title={
                      setting.id === "expprogram" && !setting.writable
                        ? t("tether.modeDialHint")
                        : !setting.writable
                          ? t("tether.settingReadonly")
                          : undefined
                    }
                    className="flex items-center gap-1.5 whitespace-nowrap rounded-full px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:cursor-not-allowed disabled:opacity-60 disabled:hover:bg-transparent"
                    data-testid={`tether-setting-${setting.id}`}
                  >
                    {settingLabel(setting.id, setting.label, t)}
                    <span className="font-mono tabular-nums text-text-primary">{setting.current}</span>
                    <svg viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                      <path d="M4 10l4-4 4 4" />
                    </svg>
                  </button>
                  {quickOpen === setting.id && setting.options.length > 0 && (
                    <div
                      className="sp-scroll absolute bottom-9 left-1/2 z-40 max-h-60 w-max min-w-28 -translate-x-1/2 overflow-y-auto rounded-lg border border-edge bg-surface p-1 shadow-xl"
                      data-testid={`tether-quick-${setting.id}-menu`}
                    >
                      {setting.options.map((option) => (
                        <button
                          key={option.value}
                          type="button"
                          onClick={() => {
                            setQuickOpen(null);
                            void applySetting(setting, option.value);
                          }}
                          className={`block w-full rounded px-2.5 py-1 text-left font-mono text-[11px] tabular-nums transition-colors hover:bg-panel ${
                            option.value === setting.current ? "bg-panel text-accent" : "text-text-secondary hover:text-accent"
                          }`}
                          data-testid={`tether-quick-${setting.id}-opt`}
                          data-value={option.value}
                        >
                          {option.label === "On" ? t("tether.on") : option.label === "Off" ? t("tether.off") : option.label}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              ))}
              <span className="h-4 w-px shrink-0 bg-edge" aria-hidden="true" />
              {/* AF 触发（画面中心对焦；绿边圆钮） */}
              <button
                type="button"
                onClick={() => void triggerAf()}
                disabled={!connected}
                className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-full border-2 border-emerald-400/80 text-[10px] font-bold tracking-wide text-emerald-400 shadow transition-all hover:bg-emerald-400/10 active:scale-95 disabled:cursor-not-allowed disabled:border-edge disabled:text-text-muted disabled:opacity-50 ${afFlash ? "bg-emerald-400/25" : ""}`}
                title={t("tether.param.autofocus")}
                aria-label={t("tether.param.autofocus")}
                data-testid="tether-af"
              >
                AF
              </button>
              <button
                type="button"
                onClick={() => void shoot()}
                disabled={capturing || !connected}
                className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full border-[3px] border-red-500/80 bg-red-500/15 shadow transition-all hover:bg-red-500/30 active:scale-95 disabled:cursor-not-allowed disabled:border-edge disabled:bg-panel disabled:opacity-50"
                title={t("tether.shutter")}
                aria-label={t("tether.shutter")}
                data-testid="tether-shutter"
              >
                {capturing ? (
                  <span className="h-2 w-2 animate-pulse rounded-full bg-red-400" />
                ) : (
                  <span className="h-3 w-3 rounded-full bg-red-400/80" />
                )}
              </button>
            </div>
            {quickError !== undefined && (
              <p className="mt-1.5 text-center text-[11px] text-red-400" data-testid="tether-quick-error" role="status">
                {t("tether.settingFailed")}：{quickError}
              </p>
            )}
          </div>
        </div>

        {/* 右栏：相机参数 + 快门 */}
        <aside className="sp-scroll flex w-64 shrink-0 flex-col gap-4 overflow-y-auto border-l border-edge bg-surface p-3" data-testid="tether-settings-panel">
          <div className="flex items-center justify-between">
            <h2 className="text-[10px] font-semibold uppercase tracking-wider text-text-muted">{t("tether.settings")}</h2>
            <button
              type="button"
              onClick={() => {
                if (sessionId !== "") void tetheringSettings(sessionId).then((next) => {
                  if (next !== null) setSession((prev) => (prev === null ? prev : { ...prev, settings: next }));
                });
              }}
              className="rounded border border-edge px-1.5 py-0.5 text-[10px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
              data-testid="tether-settings-refresh"
            >
              {t("tether.refreshSettings")}
            </button>
          </div>
          {panel.length === 0 ? (
            <p className="text-[11px] leading-relaxed text-text-muted" data-testid="tether-settings-empty">
              {t("tether.noAdjustable")}
            </p>
          ) : (
            panel.map((setting) =>
            setting.kind === "action" ? (
              <button
                key={setting.id}
                type="button"
                onClick={() => void applySetting(setting, "1")}
                disabled={!setting.writable || !connected}
                className="w-full rounded-md border border-accent/50 bg-accent/10 px-3 py-1.5 text-xs font-medium text-accent transition-colors hover:bg-accent/20 disabled:cursor-not-allowed disabled:border-edge disabled:bg-panel disabled:text-text-muted"
                data-testid={`tether-setting-${setting.id}`}
              >
                {settingLabel(setting.id, setting.label, t)}
              </button>
            ) : setting.kind === "range" ? (
              <div key={setting.id} className="flex flex-col gap-1 text-xs text-text-secondary" data-testid={`tether-setting-${setting.id}`}>
                <span className="flex items-center justify-between">
                  {settingLabel(setting.id, setting.label, t)}
                  <span className="tabular-nums text-text-primary">
                    {rangeDrafts[setting.id] ?? setting.current}
                    {!setting.writable && <span className="ml-1 text-[10px] text-text-muted">{t("tether.settingReadonly")}</span>}
                  </span>
                </span>
                <input
                  type="range"
                  min={setting.min ?? 0}
                  max={setting.max ?? 0}
                  step={setting.step ?? 1}
                  value={rangeDrafts[setting.id] ?? setting.current}
                  disabled={!setting.writable || !connected}
                  onInput={(e) => {
                    const draft = e.currentTarget.value;
                    setRangeDrafts((prev) => ({ ...prev, [setting.id]: draft }));
                  }}
                  onPointerUp={() => commitRange(setting)}
                  onKeyUp={() => commitRange(setting)}
                  onBlur={() => commitRange(setting)}
                  className="w-full accent-[var(--color-accent)] disabled:opacity-40"
                />
                {settingErrors[setting.id] !== undefined && (
                  <span className="text-[10px] text-red-400">
                    {t("tether.settingFailed")}：{settingErrors[setting.id]}
                  </span>
                )}
              </div>
            ) : (
              <label key={setting.id} className="flex flex-col gap-1 text-xs text-text-secondary" data-testid={`tether-setting-${setting.id}`}>
                <span className="flex items-center justify-between">
                  {settingLabel(setting.id, setting.label, t)}
                  {!setting.writable && <span className="text-[10px] text-text-muted">{t("tether.settingReadonly")}</span>}
                </span>
                <select
                  value={setting.current}
                  disabled={!setting.writable || !connected}
                  onChange={(e) => void applySetting(setting, e.target.value)}
                  className="w-full rounded-md border border-edge bg-bg px-2 py-1.5 text-xs text-text-primary outline-none transition-colors focus:border-accent disabled:opacity-40"
                >
                  {setting.options.length > 0 ? (
                    setting.options.map((option) => (
                      <option key={option.value} value={option.value}>
                        {option.label === "On" ? t("tether.on") : option.label === "Off" ? t("tether.off") : option.label}
                      </option>
                    ))
                  ) : (
                    <option value={setting.current}>{setting.current}</option>
                  )}
                </select>
                {settingErrors[setting.id] !== undefined && (
                  <span className="text-[10px] text-red-400">
                    {t("tether.settingFailed")}：{settingErrors[setting.id]}
                  </span>
                )}
              </label>
            )
            )
          )}
        </aside>
      </div>

      {/* 底部：会话胶片条 */}
      <div className="flex h-24 shrink-0 items-center gap-2 overflow-x-auto border-t border-edge bg-surface px-3" data-testid="tether-filmstrip">
        {photos.length === 0 && (
          <span className="text-[11px] text-text-muted">{t("tether.noPhotos")}</span>
        )}
        {[...photos].reverse().map((photo) => (
          <div
            key={photo.id}
            className="flex h-[88px] w-[88px] shrink-0 items-center justify-center overflow-hidden rounded-md border border-edge bg-bg"
            title={photo.name}
            data-testid={`tether-film-${photo.id}`}
          >
            {previews[photo.id] !== undefined ? (
              <img src={previews[photo.id]} alt={photo.name} className="h-full w-full object-cover" />
            ) : (
              <span className="text-[10px] text-text-muted">…</span>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
