import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isMacPlatform } from "@/lib/platform";
import { motion } from "motion/react";

import { motionInitial, TRANS, useMotionOn } from "@/lib/motion";
import ViewerOverlay from "@/features/gallery/components/ViewerOverlay";
import { useAssetViewer } from "@/features/gallery/lib/useAssetViewer";

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
  assetsByIds,
  type AssetDto,
  type TetherCameraSetting,
  type TetherSessionDto,
} from "@/ipc/api";

import { filmCards, type FilmCard } from "./filmstrip";

/**
 * 联机拍摄独立窗口（/tethering?session=<id>；后端 tethering_start 创建本窗口，
 * 窗口销毁即自动结束会话）。LR 式布局：
 * - 顶部：自绘标题栏（decorations=false）——相册名/相机名/连接状态 + 最小化/关闭
 * - 中央：实时取景（tethering_frame 轮询，帧率 15/30/60 档可调；不支持时回落最近一张成片）
 * - 右栏：相机参数（shutter/aperture/iso 下拉；tethering_setting_set 即时生效）
 * - 底部：本次会话胶片条（tetheringPhotoAdded 事件驱动；一次快门一张卡，
 *   RAW+JPG 同 stem 合并 + 格式角标；点击复用画廊查看器全屏预览）
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

/** 滑条取值范围：后端嵌套 range 优先，旧扁平 min/max/step 兼容；
 *  max ≤ min 或 step ≤ 0 视为无效（回退下拉/单值展示，不渲染滑条）。 */
function sliderRange(s: TetherCameraSetting): { min: number; max: number; step: number } | null {
  const r = s.range ?? (s.min !== undefined && s.max !== undefined
    ? { min: s.min, max: s.max, step: s.step ?? 1 }
    : null);
  if (r === null || !(r.max > r.min) || !(r.step > 0)) return null;
  return r;
}

/** 面板排序：常用拍摄参数在前，其余按标签字母序，动作按钮最后。 */
const SETTING_ORDER = [
  "shutterspeed", "f-number", "iso", "expprogram", "exposuremetermode", "exposurecompensation",
  "whitebalance", "colortemperature", "imagequality", "capturemode", "focusmode", "focusarea",
  "imagestabilization", "flashmode", "shuttertype", "silentmode", "dro", "aspectratio",
  "imagesize", "pcsaveimgsize", "liveviewsettingeffect", "manualfocus", "focusmagnifier",
] as const;

/** 悬浮工具栏保留拍摄模式与曝光三要素：常驻取景画面
 *  底部，随手可调不用翻面板。choice 形态才进工具栏（range/action 走右栏）。 */
const QUICK_IDS = ["expprogram", "shutterspeed", "f-number", "iso"] as const;

/** 工具栏常驻例外：只读也保留展示——
 *  - expprogram（拍摄模式）：只读时提示使用机身转盘；
 *  - f-number（光圈）：镜头环控制的机身常见，只读也要能看到当前值。 */
const QUICK_ALWAYS_SHOW = new Set<string>(["expprogram", "f-number"]);

/** 机身模式切换（A/S 档 ↔ M 档）会改变参数可写性——settings 轮询间隔：
 *  只读的快门在 A 档隐藏、切回 M 档要能自动回来，靠这轮询刷新快照。 */
const SETTINGS_POLL_MS = 5000;

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

/** 工具栏参数：拍摄模式和曝光三要素；只读模式与光圈保留展示。
 *  可写但无候选（Range 型 ISO 等）不进工具栏（弹层只承载候选表，滑条走右栏）。 */
function quickSettings(settings: TetherCameraSetting[]): TetherCameraSetting[] {
  return QUICK_IDS.map((id) => settings.find((s) => s.id === id))
    .filter((s): s is TetherCameraSetting => {
      if (s === undefined || s.kind !== "choice") return false;
      return (s.writable && s.options.length > 0) || QUICK_ALWAYS_SHOW.has(s.id);
    });
}

/** 右栏参数：工具栏之外的可调参数。
 *  排除口径 = 工具栏实际渲染的参数（可写但无候选的 QUICK_ID 走右栏滑条，
 *  不能两头都不渲染）。 */
function panelSettings(settings: TetherCameraSetting[]): TetherCameraSetting[] {
  const quick = new Set(quickSettings(settings).map((s) => s.id));
  return orderSettings(settings).filter((s) => !quick.has(s.id) && s.writable);
}

export default function TetheringWindowPage() {
  const { t } = useTranslation();
  const [params] = useSearchParams();
  const sessionId = params.get("session") ?? "";

  const [session, setSession] = useState<TetherSessionDto | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [frame, setFrame] = useState<string | null>(null);
  /** 相机回报的当前 AF 区域（归一化 x/y/w/h；随帧更新，null=本帧无数据）。 */
  const [afRegion, setAfRegion] = useState<[number, number, number, number] | null>(null);
  const [fps, setFps] = useState<number>(loadStoredFps);
  const [capturing, setCapturing] = useState(false);
  const [captureError, setCaptureError] = useState<string | null>(null);
  const [settingErrors, setSettingErrors] = useState<Record<string, string>>({});
  const [pendingSettings, setPendingSettings] = useState<Record<string, string>>({});
  const pendingValues = useRef<Record<string, string>>({});
  const settingQueue = useRef(new Map<string, { setting: TetherCameraSetting; value: string }>());
  const writingRef = useRef(false);
  const captureRef = useRef(false);
  const focusingRef = useRef(false);
  const refreshingRef = useRef(false);
  const settingsDirty = useRef(false);
  const settingsVersion = useRef(0);
  const alive = useRef(true);
  const refreshTimer = useRef<number | null>(null);
  const [previews, setPreviews] = useState<Record<number, string>>({});
  const photosRef = useRef<number[]>([]);
  /** Range 滑条本地草稿（id → 未提交值；松手才下发，避免拖动连发）。 */
  const [rangeDrafts, setRangeDrafts] = useState<Record<string, string>>({});
  /** 点击对焦标记（归一化坐标 + 2s 自动消失）。 */
  const [focusMark, setFocusMark] = useState<{ x: number; y: number } | null>(null);
  /** 工具栏参数弹层：当前展开的设置 id（单开；点外部收起）。 */
  const [quickOpen, setQuickOpen] = useState<string | null>(null);
  const quickBarRef = useRef<HTMLDivElement | null>(null);
  /** 每次打开默认底部居中；手动拖动的位置仅保留在当前窗口。 */
  const [barPos, setBarPos] = useState<{ x: number; y: number } | null>(null);
  /** AF 触发反馈（600ms 绿闪）。 */
  const [afFlash, setAfFlash] = useState(false);
  /** 待收计数（前端近似）：快门成功触发 +1、photoAdded 事件/快照新片 -1；
   *  后端事件暂无收据 pending 真值，以此近似显示「N 待收」。 */
  const [pendingShots, setPendingShots] = useState(0);
  /** 点击胶片条打开的查看器结果集（打开时按会话照片全量拉一次，≤64；
   *  复用画廊 useAssetViewer + ViewerOverlay——左右切换/RAW↔JPG 版本切换
   *  同一结果集内完成）。 */
  const [viewerAssets, setViewerAssets] = useState<AssetDto[]>([]);
  const viewerGroups = useMemo(() => [{ key: "__tether__", date: null, assets: viewerAssets }], [viewerAssets]);
  const { viewer, openAsset, closeViewer, navigateTo, selectVersion } = useAssetViewer(viewerGroups, viewerAssets);
  const motionOn = useMotionOn();

  const liveViewSupported = session?.camera.capabilities.liveView === true;
  /** 远程拍摄能力（标准 PTP 或厂商扩展，任一即可）——能力位驱动，不做品牌假设。 */
  const captureSupported = session?.camera.capabilities.standardCapture === true
    || session?.camera.capabilities.vendorCaptureNikon === true;
  const connected = session?.connected === true;
  const receiving = session?.receiving === true;
  const photos = session?.photos ?? [];
  /** 待收显示 = 后端 FIFO 真值优先（RAW+JPG/连拍按队列实数计），本地
   *  快门计数兜底（快照未回之间的间隙）。 */
  const pendingTotal = Math.max(pendingShots, session?.pendingReceipts ?? 0);
  /** 胶片条合并卡（一次快门一张卡；RAW+JPG 同 stem 合并，角标标注格式）。 */
  const cards = useMemo(() => filmCards(photos), [photos]);
  const settingsBusy = Object.keys(pendingSettings).length > 0;
  /** 待收中的收片遇错（会话错误/拍摄错误）→ 轻提示转为「重试中」。 */
  const receivingRetry = pendingTotal > 0 && (captureError !== null || session?.error != null);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      settingsVersion.current += 1;
      settingQueue.current.clear();
      if (refreshTimer.current !== null) window.clearTimeout(refreshTimer.current);
    };
  }, [sessionId]);

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
  /** 全量参数快照刷新（轮询 / 改参联动 / 手动刷新按钮共用）；失败静默。 */
  const refreshSettings = useCallback(async function refresh() {
    if (sessionId === "" || !alive.current || refreshingRef.current || writingRef.current || captureRef.current || focusingRef.current) return;
    const version = settingsVersion.current;
    settingsDirty.current = false;
    refreshingRef.current = true;
    try {
      const next = await tetheringSettings(sessionId);
      if (next !== null && alive.current && version === settingsVersion.current && !writingRef.current && !captureRef.current) {
        setSession((prev) => (prev === null ? prev : { ...prev, settings: next }));
      }
    } catch {
      /* 变化通知与低频快照均会重试，单次失败不打断操作。 */
    } finally {
      refreshingRef.current = false;
      // 读取期间又有变化通知：结束后补一次，不丢变化，也不并发读取。
      if (settingsDirty.current && alive.current && !writingRef.current && !captureRef.current && !focusingRef.current) {
        if (refreshTimer.current !== null) window.clearTimeout(refreshTimer.current);
        refreshTimer.current = window.setTimeout(() => { refreshTimer.current = null; void refresh(); }, 200);
      }
    }
  }, [sessionId]);

  const scheduleSettingsRefresh = useCallback((delay = 50) => {
    settingsDirty.current = true;
    if (refreshTimer.current !== null) window.clearTimeout(refreshTimer.current);
    refreshTimer.current = window.setTimeout(() => {
      refreshTimer.current = null;
      void refreshSettings();
    }, delay);
  }, [refreshSettings]);

  useEffect(() => {
    if (sessionId === "" || !connected) return;
    let stopped = false;
    let timer: number;
    const tick = async () => {
      await refreshSettings();
      if (!stopped) timer = window.setTimeout(() => void tick(), SETTINGS_POLL_MS);
    };
    timer = window.setTimeout(() => void tick(), SETTINGS_POLL_MS);
    return () => { stopped = true; window.clearTimeout(timer); };
  }, [sessionId, connected, refreshSettings]);

  // 会话快照：挂载拉一次；photoAdded/status 事件增量合并（轻路径，不整页重拉）
  const mergeSession = useCallback((next: TetherSessionDto) => {
    setSession(next);
    const ids = next.photos.map((p) => p.id);
    // 待收对账：快照里已到的新片冲抵待收计数（photoAdded 事件丢失/竞态时兜底）
    const arrived = Math.max(0, next.photos.length - photosRef.current.length);
    if (arrived > 0) setPendingShots((n) => Math.max(0, n - arrived));
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
    /** photoAdded 后去抖重拉快照：后端 FIFO 真值（pendingReceipts）与快照
     *  对账兜底都在快照里——RAW+JPG 第二张在途时「N 待收」不失真。 */
    let resnapshot: number | null = null;
    const scheduleResnapshot = () => {
      if (resnapshot !== null) window.clearTimeout(resnapshot);
      resnapshot = window.setTimeout(() => {
        resnapshot = null;
        void tetheringSession(sessionId).then((dto) => {
          if (dto !== null && dto !== undefined && alive.current) mergeSession(dto);
        });
      }, 400);
    };
    void subscribeAppEvents((event) => {
      if (event.type === "tetheringPhotoAdded" && event.sessionId === sessionId) {
        setPendingShots((n) => Math.max(0, n - 1));
        setSession((prev) =>
          prev === null || prev.photos.some((photo) => photo.id === event.assetId)
            ? prev
            : {
                ...prev,
                photos: [...prev.photos, { id: event.assetId, name: event.name, kind: event.kind }].slice(-64),
              },
        );
        scheduleResnapshot();
      } else if (event.type === "tetheringStatus" && event.sessionId === sessionId) {
        setSession((prev) =>
          prev === null ? prev : { ...prev, connected: event.connected, error: event.error },
        );
      } else if (event.type === "tetheringSettingsChanged" && event.sessionId === sessionId) {
        settingsVersion.current += 1;
        scheduleSettingsRefresh();
      }
    })
      .then((fn) => {
        if (cancelled) fn();
        else off = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      if (resnapshot !== null) window.clearTimeout(resnapshot);
      off?.();
    };
  }, [sessionId, scheduleSettingsRefresh, mergeSession]);

  // 实时取景轮询：拿不到帧保持上一帧（不闪烁）；拍摄/收片期间后端返回 null
  useEffect(() => {
    if (sessionId === "" || !liveViewSupported || !connected || capturing || settingsBusy) return;
    let stopped = false;
    let timer: number | null = null;
    let emptyFrames = 0;
    const tick = () => {
      void tetheringFrame(sessionId).then((payload) => {
        if (stopped || !alive.current) return;
        if (payload !== null) {
          setFrame(payload.url);
          setAfRegion(payload.af);
          emptyFrames = 0;
        } else {
          emptyFrames += 1;
        }
        // 相机忙或暂时拿不到画面时退让，避免高帧率重复失败请求。
        const delay = payload === null ? Math.min(500, framePollMs(fps) * Math.min(emptyFrames, 15)) : framePollMs(fps);
        timer = window.setTimeout(tick, delay);
      });
    };
    tick();
    return () => {
      stopped = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [sessionId, liveViewSupported, connected, fps, capturing, settingsBusy]);

  // 胶片条预览：为还没有预览的合并卡代表拉缩略图（新片到达后缩略图生成有延迟，
  // 重试 3 次；只拉代表卡——JPG 优先，缩略图解码稳定）
  useEffect(() => {
    if (sessionId === "") return;
    let cancelled = false;
    const missing = cards.map((c) => c.repId).filter((id) => previews[id] === undefined);
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
  }, [sessionId, cards, previews]);

  /** 点击胶片条卡片 → 全屏查看器（复用画廊 ViewerOverlay；结果集 = 本次
   *  会话照片，打开时拉取——左右切换/版本切换都在会话集内）。 */
  async function openCard(card: FilmCard): Promise<void> {
    if (sessionId === "") return;
    try {
      const list = await assetsByIds(photos.map((p) => p.id));
      const rep = list.find((a) => a.id === card.repId);
      if (rep === undefined || !alive.current) return;
      setViewerAssets(list);
      openAsset(rep);
    } catch {
      /* 照片已删除/库不可访问：不打开 */
    }
  }

  async function closeWindow(): Promise<void> {
    alive.current = false;
    settingQueue.current.clear();
    try {
      if (sessionId !== "") await tetheringStop(sessionId);
    } finally {
      void getCurrentWindow().close();
    }
  }

  async function applySetting(setting: TetherCameraSetting, value: string): Promise<void> {
    if (sessionId === "" || !connected || captureRef.current || !alive.current || !setting.writable
      || value === (pendingValues.current[setting.id] ?? setting.current)) {
      return;
    }
    setSettingErrors((prev) => {
      const next = { ...prev };
      delete next[setting.id];
      return next;
    });
    settingsVersion.current += 1;
    pendingValues.current[setting.id] = value;
    setPendingSettings({ ...pendingValues.current });
    // 同一参数只保留最新的未发送值；所有参数写入串行，避免旧回包覆盖新值。
    settingQueue.current.set(setting.id, { setting, value });
    if (writingRef.current) return;
    writingRef.current = true;
    try {
      while (settingQueue.current.size > 0 && alive.current) {
        const [id, request] = settingQueue.current.entries().next().value!;
        settingQueue.current.delete(id);
        try {
          const next = await tetheringSettingSet(sessionId, id, request.value);
          if (alive.current) setSession((prev) => (prev === null ? prev : { ...prev, settings: next }));
        } catch (err) {
          if (alive.current && pendingValues.current[id] === request.value) {
            const message = err instanceof Error ? err.message : String(err);
            setSettingErrors((prev) => ({ ...prev, [id]: message }));
          }
        } finally {
          if (pendingValues.current[id] === request.value && !settingQueue.current.has(id)) delete pendingValues.current[id];
          if (alive.current) setPendingSettings({ ...pendingValues.current });
        }
      }
    } finally {
      writingRef.current = false;
      if (alive.current) scheduleSettingsRefresh(0);
    }
  }

  /** 点击取景画面对焦：坐标归一化下发 + 十字标记（2s 消失）。 */
  async function focusAtView(e: React.MouseEvent<HTMLDivElement>): Promise<void> {
    if (sessionId === "" || !liveViewSupported || !connected || captureRef.current || writingRef.current || focusingRef.current) return;
    const img = e.currentTarget.querySelector("img");
    if (img === null) return;
    const viewBox = e.currentTarget.getBoundingClientRect();
    const rect = contentRect === null ? img.getBoundingClientRect() : {
      left: viewBox.left + viewBox.width * contentRect.left / 100,
      top: viewBox.top + viewBox.height * contentRect.top / 100,
      width: viewBox.width * contentRect.width / 100,
      height: viewBox.height * contentRect.height / 100,
    };
    if (rect.width <= 0 || rect.height <= 0) return;
    const x = (e.clientX - rect.left) / rect.width;
    const y = (e.clientY - rect.top) / rect.height;
    if (x < 0 || x > 1 || y < 0 || y > 1) return;
    setFocusMark({
      x: viewBox.width > 0 ? (e.clientX - viewBox.left) / viewBox.width : x,
      y: viewBox.height > 0 ? (e.clientY - viewBox.top) / viewBox.height : y,
    });
    window.setTimeout(() => setFocusMark(null), 2000);
    try {
      focusingRef.current = true;
      await tetheringFocusAt(sessionId, x, y);
    } catch {
      /* 相机不支持/模式不符：标记自然消失即可，不打断取景 */
    } finally {
      focusingRef.current = false;
      if (alive.current && settingsDirty.current) scheduleSettingsRefresh();
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
    if (sessionId === "" || !connected || !captureSupported || captureRef.current || writingRef.current || focusingRef.current) return;
    captureRef.current = true;
    settingsVersion.current += 1;
    setCapturing(true);
    setCaptureError(null);
    try {
      const result = await tetheringCapture(sessionId);
      if (!result.ok && alive.current) setCaptureError(result.error ?? t("tether.captureFailed"));
      // 快门已触发 → 进入待收（photoAdded 事件/快照新片到账后冲抵）
      if (result.ok && alive.current) setPendingShots((n) => n + 1);
      const next = await tetheringSession(sessionId);
      if (next !== null && alive.current) mergeSession(next);
    } catch (err) {
      if (alive.current) setCaptureError(err instanceof Error ? err.message : t("tether.captureFailed"));
    } finally {
      captureRef.current = false;
      if (alive.current) {
        setCapturing(false);
        scheduleSettingsRefresh();
      }
    }
  }

  /** 工具栏 AF 触发：画面中心对焦（不支持/模式不符静默）+ 绿闪反馈。
   *  点对焦是 live view 操作——相机未上报 liveView 能力时不触发。 */
  async function triggerAf(): Promise<void> {
    if (sessionId === "" || !connected || !liveViewSupported || captureRef.current || writingRef.current || focusingRef.current) return;
    focusingRef.current = true;
    setAfFlash(true);
    window.setTimeout(() => setAfFlash(false), 600);
    try {
      await tetheringFocusAt(sessionId, 0.5, 0.5);
    } catch {
      /* 相机不支持：绿闪照常消失 */
    } finally {
      focusingRef.current = false;
      if (alive.current && settingsDirty.current) scheduleSettingsRefresh();
    }
  }

  /** 工具栏拖动：grip 按下 → pointer 跟随（限取景容器内）。 */
  function beginBarDrag(e: React.PointerEvent<HTMLButtonElement>): void {
    if (e.button !== 0) return;
    const bar = quickBarRef.current;
    if (bar === null || bar.parentElement === null) return;
    setQuickOpen(null);
    const offX = e.clientX - bar.getBoundingClientRect().left;
    const offY = e.clientY - bar.getBoundingClientRect().top;
    const onMove = (ev: PointerEvent) => {
      const view = quickBarRef.current?.parentElement;
      const barEl = quickBarRef.current;
      if (view === null || view === undefined || barEl === null) return;
      const vr = view.getBoundingClientRect();
      const x = Math.min(Math.max(0, ev.clientX - vr.left - offX), Math.max(0, vr.width - barEl.offsetWidth));
      const y = Math.min(Math.max(0, ev.clientY - vr.top - offY), Math.max(0, vr.height - barEl.offsetHeight));
      setBarPos({ x, y });
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }

  // 主画面（渲染与内容矩形测量共用）：live view 帧；不支持时回落最近成片。
  const lastCard = cards.length > 0 ? cards[cards.length - 1] : null;
  const mainImage = liveViewSupported ? frame : lastCard !== null ? (previews[lastCard.repId] ?? null) : null;

  // --- 取景画面内容矩形（AF 框叠加定位基准） ------------------------------------------
  // object-contain 的实际显示区居中于取景容器，AF 框（归一化）必须相对内容区
  // 定位而非容器；容器百分比几何在等比缩放下不变，帧到达/窗口变形时重算，
  // 测不到（未布局/测试环境）为 null（不渲染叠加）。
  const viewRef = useRef<HTMLDivElement | null>(null);
  const [contentRect, setContentRect] = useState<{ left: number; top: number; width: number; height: number } | null>(null);
  const measureContentRect = useCallback(() => {
    const view = viewRef.current;
    const img = view?.querySelector("img") ?? null;
    if (view === null || img === null) {
      setContentRect(null);
      return;
    }
    const viewBox = view.getBoundingClientRect();
    let imgBox = img.getBoundingClientRect();
    if (img.naturalWidth > 0 && img.naturalHeight > 0) {
      const scale = Math.min(viewBox.width / img.naturalWidth, viewBox.height / img.naturalHeight);
      const width = img.naturalWidth * scale;
      const height = img.naturalHeight * scale;
      imgBox = { ...imgBox, width, height, left: viewBox.left + (viewBox.width - width) / 2,
        top: viewBox.top + (viewBox.height - height) / 2 };
    }
    if (viewBox.width <= 0 || viewBox.height <= 0 || imgBox.width <= 0 || imgBox.height <= 0) {
      setContentRect(null);
      return;
    }
    setContentRect({
      left: ((imgBox.left - viewBox.left) / viewBox.width) * 100,
      top: ((imgBox.top - viewBox.top) / viewBox.height) * 100,
      width: (imgBox.width / viewBox.width) * 100,
      height: (imgBox.height / viewBox.height) * 100,
    });
  }, []);
  useEffect(() => {
    measureContentRect();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measureContentRect);
    if (viewRef.current !== null) observer?.observe(viewRef.current);
    window.addEventListener("resize", measureContentRect);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", measureContentRect);
    };
  }, [mainImage, measureContentRect]);

  useEffect(() => {
    const bar = quickBarRef.current;
    const view = viewRef.current;
    if (bar === null || view === null) return;
    const keepBarVisible = () => {
      const bounds = view.getBoundingClientRect();
      if (bounds.width <= 0 || bounds.height <= 0) return;
      const maxX = Math.max(0, bounds.width - bar.offsetWidth);
      const maxY = Math.max(0, bounds.height - bar.offsetHeight);
      setBarPos((position) => {
        if (position === null) return null;
        const x = Math.min(Math.max(0, position.x), maxX);
        const y = Math.min(Math.max(0, position.y), maxY);
        return x === position.x && y === position.y ? position : { x, y };
      });
    };
    keepBarVisible();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(keepBarVisible);
    observer?.observe(view);
    observer?.observe(bar);
    window.addEventListener("resize", keepBarVisible);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", keepBarVisible);
    };
  }, [loaded, session !== null]);

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

  const quick = quickSettings(session.settings);
  const panel = panelSettings(session.settings);
  const quickError = quick.map((s) => settingErrors[s.id]).find(Boolean);

  return (
    <div className={`flex h-screen flex-col bg-bg text-text-primary ${isMacPlatform() ? "mac-window-content" : ""}`} data-testid="tether-window">
      {/* 自绘标题栏（decorations=false） */}
      {!isMacPlatform() && <div className="relative flex h-10 shrink-0 select-none items-center gap-3 border-b border-edge bg-surface pl-3" data-testid="tether-titlebar">
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
      </div>}

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
        {/* 中央：取景画面（点击画面对焦；不支持 live view 的相机回落最近成片，
            容器 tooltip 说明原因——能力位驱动，不做品牌假设） */}
        <div
          ref={viewRef}
          className={`relative flex min-h-0 min-w-0 flex-1 items-center justify-center overflow-hidden bg-black ${liveViewSupported && connected ? "cursor-crosshair" : ""}`}
          data-testid="tether-view"
          title={!liveViewSupported ? t("tether.liveViewUnsupported") : undefined}
          onClick={(e) => void focusAtView(e)}
        >
          {mainImage !== null ? (
            <img src={mainImage} alt="live view" className="h-full w-full object-contain" onLoad={measureContentRect} data-testid="tether-view-img" />
          ) : (
            <div className="flex flex-col items-center gap-2 text-xs text-white/40">
              <span className={liveViewSupported ? "animate-pulse" : undefined}>{liveViewSupported ? t("tether.liveViewWaiting") : t("tether.liveViewUnsupported")}</span>
              {!liveViewSupported && lastCard === null && <span className="text-[11px]">{t("tether.noPhotos")}</span>}
            </div>
          )}
          {/* 相机回报的当前 AF 框（随帧更新；内容区内定位）——点击对焦后
              机身落点可见，坐标映射是否正确一目了然 */}
          {afRegion !== null && contentRect !== null && (
            <div
              className="pointer-events-none absolute rounded-sm border-2 border-emerald-400/90 shadow-[0_0_0_1px_rgba(0,0,0,0.35)]"
              style={{
                left: `${(contentRect.left + afRegion[0] * contentRect.width).toFixed(3)}%`,
                top: `${(contentRect.top + afRegion[1] * contentRect.height).toFixed(3)}%`,
                width: `${(afRegion[2] * contentRect.width).toFixed(3)}%`,
                height: `${(afRegion[3] * contentRect.height).toFixed(3)}%`,
              }}
              data-testid="tether-af-region"
            />
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
          {/* 待收轻提示：快门已触发、收片未到账（前端计数近似，后端收据事件
              在途）；收片遇错转为「重试中」。photoAdded 到账后自动消失。 */}
          {pendingTotal > 0 && (
            <motion.div
              className="pointer-events-none absolute left-3 top-3 z-10 rounded-full bg-black/60 px-2.5 py-1 text-[11px] text-white/90 backdrop-blur"
              role="status"
              initial={motionInitial(motionOn, { y: -8, opacity: 0 })}
              animate={{ y: 0, opacity: 1 }}
              transition={TRANS.slide}
              data-testid="tether-pending-receipts"
            >
              {receivingRetry
                ? t("tether.receivingRetry")
                : t("tether.pendingShots", { count: pendingTotal })}
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

          {/* 拍摄模式、曝光三要素与快门浮条，默认底部居中并支持拖动。 */}
          <div
            ref={quickBarRef}
            className={`absolute z-20 w-max max-w-full ${barPos === null ? "bottom-3 left-1/2 -translate-x-1/2" : ""}`}
            style={barPos === null ? undefined : { left: barPos.x, top: barPos.y }}
            onClick={(e) => e.stopPropagation()}
            data-testid="tether-quickbar"
          >
            <div className="flex flex-wrap items-center gap-1 rounded-full border border-edge bg-surface/95 px-2.5 py-1.5 shadow-xl backdrop-blur">
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
                    disabled={!setting.writable || !connected || capturing}
                    aria-busy={pendingSettings[setting.id] !== undefined}
                    aria-expanded={quickOpen === setting.id}
                    title={
                      setting.id === "expprogram" && !setting.writable
                        ? t("tether.modeDialHint")
                        : !setting.writable ? t("tether.settingReadonly") : undefined
                    }
                    className="flex items-center gap-1.5 whitespace-nowrap rounded-full px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:cursor-not-allowed disabled:opacity-60 disabled:hover:bg-transparent"
                    data-testid={`tether-setting-${setting.id}`}
                  >
                    {settingLabel(setting.id, setting.label, t)}
                    <span className="font-mono tabular-nums text-text-primary">{setting.options.find(option => option.value === (pendingSettings[setting.id] ?? setting.current))?.label ?? pendingSettings[setting.id] ?? setting.current}</span>
                    {pendingSettings[setting.id] !== undefined && <span className="h-2.5 w-2.5 animate-spin rounded-full border border-accent border-t-transparent" aria-hidden="true" />}
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
                          disabled={capturing}
                          onClick={() => {
                            setQuickOpen(null);
                            void applySetting(setting, option.value);
                          }}
                          className={`block w-full rounded px-2.5 py-1 text-left font-mono text-[11px] tabular-nums transition-colors hover:bg-panel ${
                            option.value === (pendingSettings[setting.id] ?? setting.current) ? "bg-panel text-accent" : "text-text-secondary hover:text-accent"
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
              <button
                type="button"
                onClick={() => void shoot()}
                disabled={capturing || !connected || !captureSupported || settingsBusy}
                className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full border-[3px] border-red-500/80 bg-red-500/15 shadow transition-all hover:bg-red-500/30 active:scale-95 disabled:cursor-not-allowed disabled:border-edge disabled:bg-panel disabled:opacity-50"
                title={!captureSupported ? t("tether.captureUnsupported") : t("tether.shutter")}
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

        {/* 右栏：其他相机参数与自动对焦 */}
        <aside className="sp-scroll flex w-64 shrink-0 flex-col gap-4 overflow-y-auto border-l border-edge bg-surface p-3" data-testid="tether-settings-panel">
          <div className="flex items-center justify-between">
            <h2 className="text-[10px] font-semibold uppercase tracking-wider text-text-muted">{t("tether.settings")}</h2>
            <button
              type="button"
              onClick={refreshSettings}
              className="rounded border border-edge px-1.5 py-0.5 text-[10px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
              data-testid="tether-settings-refresh"
            >
              {t("tether.refreshSettings")}
            </button>
          </div>
          <div className="flex items-center gap-3" data-testid="tether-camera-actions">
              {/* AF 触发（画面中心对焦；绿边圆钮）。点对焦是 live view 操作：
                  未上报 liveView 能力的相机禁用并说明原因 */}
              <button
                type="button"
                onClick={() => void triggerAf()}
                disabled={!connected || !liveViewSupported || capturing || settingsBusy}
                className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-full border-2 border-emerald-400/80 text-[10px] font-bold tracking-wide text-emerald-400 shadow transition-all hover:bg-emerald-400/10 active:scale-95 disabled:cursor-not-allowed disabled:border-edge disabled:text-text-muted disabled:opacity-50 ${afFlash ? "bg-emerald-400/25" : ""}`}
                title={!liveViewSupported ? t("tether.focusNeedsLiveView") : t("tether.param.autofocus")}
                aria-label={t("tether.param.autofocus")}
                data-testid="tether-af"
              >
                AF
              </button>

          </div>
          {panel.length === 0 ? (
            <p className="text-[11px] leading-relaxed text-text-muted" data-testid="tether-settings-empty">
              {t("tether.noAdjustable")}
            </p>
          ) : (
            panel.map((setting) => {
            // 滑条判定：无离散 options 且带有效 range（嵌套优先，扁平 min/max/step
            // 兼容）才渲染滑条；有 options 一律下拉（候选表比连续区间更精确）。
            const bounds = setting.kind !== "action" && setting.options.length === 0
              ? sliderRange(setting)
              : null;
            return setting.kind === "action" ? (
              <button
                key={setting.id}
                type="button"
                onClick={() => void applySetting(setting, "1")}
                disabled={!setting.writable || !connected || capturing || settingsBusy}
                className="w-full rounded-md border border-accent/50 bg-accent/10 px-3 py-1.5 text-xs font-medium text-accent transition-colors hover:bg-accent/20 disabled:cursor-not-allowed disabled:border-edge disabled:bg-panel disabled:text-text-muted"
                data-testid={`tether-setting-${setting.id}`}
              >
                {settingLabel(setting.id, setting.label, t)}
              </button>
            ) : bounds !== null ? (
              <div key={setting.id} className="flex flex-col gap-1 text-xs text-text-secondary" data-testid={`tether-setting-${setting.id}`}>
                <span className="flex items-center justify-between">
                  {settingLabel(setting.id, setting.label, t)}
                  <span className="tabular-nums text-text-primary" data-testid={`tether-range-value-${setting.id}`}>
                    {rangeDrafts[setting.id] ?? pendingSettings[setting.id] ?? setting.current}
                    {!setting.writable && <span className="ml-1 text-[10px] text-text-muted">{t("tether.settingReadonly")}</span>}
                  </span>
                </span>
                <input
                  type="range"
                  min={bounds.min}
                  max={bounds.max}
                  step={bounds.step}
                  value={rangeDrafts[setting.id] ?? pendingSettings[setting.id] ?? setting.current}
                  disabled={!setting.writable || !connected || capturing}
                  aria-label={settingLabel(setting.id, setting.label, t)}
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
              <label key={setting.id} className="flex flex-col gap-1 text-xs text-text-secondary" data-testid={`tether-setting-${setting.id}`}
                title={setting.id === "expprogram" && !setting.writable ? t("tether.modeDialHint") : undefined}>
                <span className="flex items-center justify-between">
                  {settingLabel(setting.id, setting.label, t)}
                  {!setting.writable && <span className="text-[10px] text-text-muted">{t("tether.settingReadonly")}</span>}
                </span>
                <select
                  value={pendingSettings[setting.id] ?? setting.current}
                  disabled={!setting.writable || !connected || capturing}
                  aria-busy={pendingSettings[setting.id] !== undefined}
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
            );
            })
          )}
        </aside>
      </div>

      {/* 底部：会话胶片条（一次快门一张卡；RAW+JPG 合并 + 格式角标；点击全屏预览） */}
      <div className="flex h-24 shrink-0 items-center gap-2 overflow-x-auto border-t border-edge bg-surface px-3" data-testid="tether-filmstrip">
        {cards.length === 0 && (
          <span className="text-[11px] text-text-muted">{t("tether.noPhotos")}</span>
        )}
        {[...cards].reverse().map((card) => (
          <button
            key={card.repId}
            type="button"
            onClick={() => void openCard(card)}
            className="relative flex h-[88px] w-[88px] shrink-0 cursor-zoom-in items-center justify-center overflow-hidden rounded-md border border-edge bg-bg transition-colors hover:border-accent"
            title={card.title}
            data-testid={`tether-film-${card.repId}`}
          >
            {previews[card.repId] !== undefined ? (
              <img src={previews[card.repId]} alt={card.title} className="h-full w-full object-cover" />
            ) : (
              <span className="text-[10px] text-text-muted">…</span>
            )}
            {card.badge !== null && (
              <span
                className="pointer-events-none absolute right-1 top-1 rounded bg-black/60 px-1 py-0.5 text-[9px] font-semibold leading-none text-white/90 backdrop-blur"
                data-testid={`tether-film-badge-${card.repId}`}
              >
                {card.badge}
              </span>
            )}
          </button>
        ))}
      </div>

      {/* 点击胶片条的全屏预览（复用画廊查看器；结果集 = 会话照片） */}
      {viewer && (
        <ViewerOverlay
          asset={viewer.asset}
          group={viewer.group}
          index={viewer.index}
          onNavigate={navigateTo}
          onClose={closeViewer}
          onVersionSelect={selectVersion}
        />
      )}
    </div>
  );
}
