import { useState, useRef, useEffect, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { deviceFiles, deviceCopyOnly, platformCapabilities, kindFromName, albumList, albumSubgroups, folderScan, albumCreate,
  importStart, photoLibraryList, type AlbumDto, type ImportMode, type ImportPlan, type PhotoLibrary, type PlatformCapabilities } from "@/ipc/api";
import { useImportStore, seedDevicesFromBackend, type RecentSource } from "@/stores/importStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { importRootOf, timeLayoutPreview } from "@/features/onboarding/onboardingConfig";
import { isUngroupedAlbum } from "@/features/albums/lib/ungroupedAlbum";
import { groupByDir, type DirGroup } from "../SourceFileViews";
import { useImportLayout } from "./useImportLayout";

export type ImportStep = "source" | "photos" | "review";

export function useImportWizard() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [step, setStep] = useState<ImportStep>("source");
  const [scanningFolder, setScanningFolder] = useState<string | null>(null);
  const folderRequest = useRef(0);
  const startGuard = useRef(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; folderRequest.current += 1; };
  }, []);
  const [searchParams, setSearchParams] = useSearchParams();

  const devices = useImportStore((s) => s.devices);
  const sourceFilesMap = useImportStore((s) => s.sourceFiles);
  const refreshDevice = useImportStore((s) => s.refreshDevice);
  const addDevice = useImportStore((s) => s.addDevice);
  const setSourceFiles = useImportStore((s) => s.setSourceFiles);
  const recentSources = useImportStore((s) => s.recentSources);
  const recordRecentSource = useImportStore((s) => s.recordRecentSource);

  const importSettings = useSettingsStore((s) => s.settings.import);
  // 目标照片库（2026-10-09 单库多照片库：导入目标 = 选照片库，替代旧 targetRoot）。
  // TODO-M3：存储页就绪后向导加照片库选择器；当前默认取第一个在线库。
  const [photoLibraries, setPhotoLibraries] = useState<PhotoLibrary[]>([]);
  useEffect(() => {
    let cancelled = false;
    void photoLibraryList().then((list) => {
      if (!cancelled) setPhotoLibraries(list);
    });
    return () => {
      cancelled = true;
    };
  }, []);
  const targetLibrary = photoLibraries.find((lib) => lib.status === "online") ?? null;

  // 用户选择或设备深链指定来源；来源失效时不自动改选其他设备。
  // 显式 ID 优先于 URL，避免路径编码或更新批次影响当前选择。
  const [explicitId, setExplicitId] = useState<string | null>(null);
  const urlDevice = searchParams.get("device");
  const explicitValid = explicitId !== null && devices.some((d) => d.id === explicitId && d.mediaPresent !== false);
  const urlValid = urlDevice !== null && devices.some((d) => d.id === urlDevice && d.mediaPresent !== false);
  const selectedId = explicitValid
    ? (explicitId as string)
    : urlValid
      ? (urlDevice as string)
      : null;
  const device = devices.find((d) => d.id === selectedId) ?? null;
  useEffect(() => {
    if (urlValid && explicitId === null) setStep("photos");
  }, [urlValid, explicitId]);

  // 选中源变化时拉取文件清单（device_files）；已有缓存的源不重复拉取。
  // 扫描中由文件增量填充；仅已完成且没有缓存的源使用兼容读取。
  const [filesLoading, setFilesLoading] = useState(false);
  useEffect(() => {
    if (!selectedId || device?.mediaPresent === false || device?.scanStatus === "scanning" || device?.scanStatus === "failed") {
      setFilesLoading(false);
      return;
    }
    if (sourceFilesMap[selectedId]) { setFilesLoading(false); return; }
    let cancelled = false;
    setFilesLoading(true);
    void deviceFiles(selectedId).then((entries) => {
      if (cancelled) return;
      setFilesLoading(false);
      if (!entries) return; // IPC 失败/不可用：保持空态（预览模式）
      setSourceFiles(
        selectedId,
        entries.map((e) => {
          const slash = e.relPath.lastIndexOf("/");
          const dir = slash >= 0 ? e.relPath.slice(0, slash) : "";
          const name = slash >= 0 ? e.relPath.slice(slash + 1) : e.relPath;
          return { objectId: e.id, mtime: e.mtime, path: e.relPath, dir, name, size: e.size, kind: kindFromName(name) };
        }),
      );
    });
    return () => {
      cancelled = true;
    };
    // sourceFilesMap[selectedId] 变为存在即触发跳过分支，无需进依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedId, device?.mediaPresent, device?.scanStatus, sourceFilesMap[selectedId ?? ""]]);

  const files = useMemo(
    () => (selectedId ? sourceFilesMap[selectedId] ?? [] : []).filter((file) => file.kind === "photo" || file.kind === "raw"),
    [selectedId, sourceFilesMap],
  );
  // 扫描中按到达顺序追加，结束后再排序，避免每批重排所有文件。
  const groups = useMemo(() => groupByDir(files, device?.scanStatus === "scanning"), [files, device?.scanStatus]);

  // 来源卡片只列真实设备；文件夹走系统选择器或最近来源，仍保留在任务源映射中。
  const visibleDevices = useMemo(
    () => devices.filter((d) => d.kind !== "folder"),
    [devices],
  );

  // 选择状态（路径集合）；设备切换时重置为全选（默认导入全部）
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const selectIncoming = useRef(true);
  const seenFiles = useRef<{ id: string | null; paths: Set<string> }>({ id: null, paths: new Set() });
  useEffect(() => {
    const next = new Set(files.map((f) => f.path));
    const previous = seenFiles.current;
    if (previous.id !== selectedId) {
      selectIncoming.current = true;
      setSelected(next);
      setCollapsed(new Set());
    } else {
      setSelected((current) => new Set([...next].filter((path) => current.has(path) || (selectIncoming.current && !previous.paths.has(path)))));
    }
    seenFiles.current = { id: selectedId, paths: next };
  }, [selectedId, files]);

  // 方案状态：目标根从目标照片库合成（只读）。落盘布局固定为纯时间
  // `{库root}/{拍摄年}/{拍摄月}/{原文件名}`（2026-10-09 定案，物理层无相册
  // 维度），预览展示公式；具体每文件的拍摄年/月由后端按 EXIF 落位。
  const targetRoot = targetLibrary ? importRootOf(targetLibrary.rootPath) : "";
  const [platformCaps, setPlatformCaps] = useState<PlatformCapabilities | null>(null);
  useEffect(() => {
    let cancelled = false;
    void platformCapabilities()
      .then((caps) => {
        if (!cancelled) setPlatformCaps(caps);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);
  const [duplicatePolicy, setDuplicatePolicy] = useState(importSettings.duplicatePolicy);
  const [skipImported, setSkipImported] = useState(importSettings.skipImported);
  // 存入相册（规格修订后必选）：无「不添加」分支；默认预选系统保底相册「未分组」，
  // 清单到位后按名匹配回填；新建分支照旧。
  const [albumChoice, setAlbumChoice] = useState<"existing" | "new">("existing");
  const [albumId, setAlbumId] = useState<number | null>(null);
  const [newAlbumName, setNewAlbumName] = useState("");
  const [albums, setAlbums] = useState<AlbumDto[]>([]);
  const [albumError, setAlbumError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    void albumList().then((list) => {
      if (!cancelled) setAlbums(list);
    });
    return () => {
      cancelled = true;
    };
  }, []);
  // 默认预选「未分组」（后端按名幂等自动创建；仅在未手选时回填）
  useEffect(() => {
    if (albums.length === 0) return;
    const ungrouped = albums.find((a) => isUngroupedAlbum(a));
    if (ungrouped) setAlbumId((prev) => (prev === null ? ungrouped.id : prev));
  }, [albums]);
  // 子分组（B4 定案，可选）：所选相册的现有子分组名（datalist 提示）；换相册即清空
  const [albumSubgroup, setAlbumSubgroup] = useState("");
  const [albumSubgroupNames, setAlbumSubgroupNames] = useState<string[]>([]);
  useEffect(() => {
    setAlbumSubgroup("");
    setAlbumSubgroupNames([]);
    if (albumChoice !== "existing" || albumId === null) return;
    let cancelled = false;
    void albumSubgroups(albumId).then((list) => {
      if (!cancelled) setAlbumSubgroupNames(list.map((g) => g.name));
    });
    return () => {
      cancelled = true;
    };
  }, [albumChoice, albumId]);
  // 目标路径预览（纯时间布局公式，2026-10-09 定案）：`{库root}/{拍摄年}/{拍摄月}/
  // {原文件名}`——具体每文件的拍摄年/月由后端按 EXIF 落位，前端只展示公式。
  const importTargetPreview = timeLayoutPreview(targetRoot);
  // 双目的地（M2）：默认关；移动模式互斥（后端拒 move+secondTarget）
  const [secondEnabled, setSecondEnabled] = useState(false);
  const [secondRoot, setSecondRoot] = useState("");
  // 双目的地第二份预览：第二根目录 + 同一纯时间公式
  const secondImportTargetPreview = secondRoot.trim() ? timeLayoutPreview(secondRoot.trim()) : "";
  const [starting, setStarting] = useState(false);
  // 启动失败文案：优先透出后端 Err；invoke 不可用时为通用文案（null → 用 i18n 兜底）
  const [startError, setStartError] = useState<string | null>(null);
  // 源选择失败通过确认对话框展示，选中其他源时清除。
  const [sourceError, setSourceError] = useState<string | null>(null);
  // 顶部导入模式：复制、移动或只在数据库登记原文件。
  const [mode, setMode] = useState<ImportMode>("copy");
  const [sourcePolicy, setSourcePolicy] = useState<{ id: string; copyOnly: boolean } | null>(null);
  useEffect(() => {
    const id = device?.id;
    if (!id) { setSourcePolicy(null); return; }
    let cancelled = false;
    setMode("copy");
    setSourcePolicy(null);
    void deviceCopyOnly(id).then((copyOnly) => {
      if (!cancelled) setSourcePolicy({ id, copyOnly });
    }).catch(() => {
      // 元数据不可用视为未知；默认复制，仍允许用户手动引用。
      if (!cancelled) setSourcePolicy({ id, copyOnly: device?.kind === "mtp" });
    });
    return () => { cancelled = true; };
  }, [device?.id, device?.kind]);
  const copyOnly = device?.kind === "mtp" || (sourcePolicy?.id === device?.id && sourcePolicy?.copyOnly === true);
  const policyPending = Boolean(device && sourcePolicy?.id !== device.id);
  useEffect(() => { if (copyOnly) setMode("copy"); }, [copyOnly]);
  const { viewMode, setViewMode, tileSize, setTileSize } = useImportLayout();

  // 设备初值补拉：后端启动枚举的 deviceScanned 事件早于 webview 订阅，
  // 错过事件的在位设备（如已连接的相机）从这里直接出现
  useEffect(() => {
    void seedDevicesFromBackend();
  }, []);

  const isMtp = device?.kind === "mtp";
  // 并发流数（应用级，2026-10-09 起不再是库属性）：默认 4；MTP 受协议限制恒 1。
  // TODO-M2：应用级导入并发设置项实装后从设置合成。
  const effectiveStreams = isMtp ? 1 : 4;
  // 双目的地开启且第二目标根目录为空 → 必填校验拦住开始
  const secondReady = !secondEnabled || secondRoot.trim().length > 0;
  const canReview = Boolean(device && targetLibrary) && scanningFolder === null && !filesLoading && device?.scanStatus !== "scanning"
    && device?.scanStatus !== "failed" && !policyPending && selected.size > 0 && !starting;
  const albumReady = albumChoice === "existing" ? albumId !== null : newAlbumName.trim().length > 0;
  const canStart = canReview && Boolean(targetRoot) && !(mode !== "copy" && copyOnly) && secondReady && albumReady;

  const selectedCount = selected.size;
  const selectedBytes = files
    .filter((f) => selected.has(f.path))
    .reduce((sum, f) => sum + f.size, 0);

  function toggleFile(path: string): void {
    selectIncoming.current = false;
    setSelected((previous) => previous.has(path)
      ? new Set([...previous].filter((value) => value !== path))
      : new Set([...previous, path]));
  }

  function toggleGroup(group: DirGroup): void {
    selectIncoming.current = false;
    setSelected((prev) => {
      const next = new Set(prev);
      const allIn = group.files.every((f) => next.has(f.path));
      for (const f of group.files) {
        if (allIn) next.delete(f.path);
        else next.add(f.path);
      }
      return next;
    });
  }

  function toggleCollapse(dir: string): void {
    setCollapsed((previous) => previous.has(dir)
      ? new Set([...previous].filter((value) => value !== dir))
      : new Set([...previous, dir]));
  }

  function selectAll(): void {
    selectIncoming.current = true;
    setSelected(new Set(files.map((f) => f.path)));
  }

  function clearSelection(): void {
    selectIncoming.current = false;
    setSelected(new Set());
  }

  function invertSelection(): void {
    selectIncoming.current = false;
    setSelected((prev) => new Set(files.filter((f) => !prev.has(f.path)).map((f) => f.path)));
  }

  // --- 源选择 -------------------------------------------------------------------

  /** 手动从设备列表选择：显式选中 + 切 URL + 记入最近使用 */
  function selectDevice(id: string): void {
    const snapshot = devices.find((d) => d.id === id);
    if (!snapshot || snapshot.mediaPresent === false) return;
    folderRequest.current += 1;
    setScanningFolder(null);
    setStep("photos");
    recordRecentSource({ id: snapshot.id, name: snapshot.name, kind: snapshot.kind });
    setExplicitId(id);
    setSourceError(null);
    setSearchParams(id ? { device: id } : {});
  }

  /** 选中文件夹为源：folderScan 成快照则入库并显式选中；失败（IPC 不可用等）提示并保持原源 */
  async function selectFolder(path: string): Promise<boolean> {
    if (!mounted.current) return false;
    const request = ++folderRequest.current;
    setScanningFolder(path);
    setSourceError(null);
    try {
      const snapshot = await folderScan(path, true);
      if (request !== folderRequest.current) return false;
      if (!snapshot) { setSourceError(t("wizard.fs.scanFailed", { dir: path })); return false; }
      addDevice(snapshot);
      recordRecentSource({ id: snapshot.id, name: snapshot.name, kind: snapshot.kind });
      setExplicitId(snapshot.id);
      setSearchParams({ device: snapshot.id });
      setStep("photos");
      return true;
    } catch (error) {
      if (request === folderRequest.current) setSourceError(`${t("wizard.fs.scanFailed", { dir: path })}：${error instanceof Error ? error.message : String(error)}`);
      return false;
    } finally {
      if (request === folderRequest.current) setScanningFolder(null);
    }
  }

  /** 选中文件夹后扫描并进入挑选页；取消系统选择器不改变现有来源。 */
  async function browseFolder(): Promise<void> {
    try {
      const dir = await openDialog({ directory: true });
      if (typeof dir === "string" && dir.length > 0) await selectFolder(dir);
    } catch (error) {
      setSourceError(t("wizard.flow.folderPickerFailed", { error: error instanceof Error ? error.message : String(error) }));
    }
  }

  /** 双目的地「浏览…」：系统目录选择器选第二目标根目录 */
  async function browseSecondRoot(): Promise<void> {
    try {
      const dir = await openDialog({ directory: true });
      if (typeof dir === "string" && dir.length > 0) setSecondRoot(dir);
    } catch (error) {
      setStartError(t("wizard.flow.folderPickerFailed", { error: error instanceof Error ? error.message : String(error) }));
    }
  }

  /** 设备仍在位才选中；文件夹重扫，避免最近来源直接使用过期状态。 */
  function selectRecent(entry: RecentSource): void {
    if (entry.kind === "folder") { void selectFolder(entry.id.slice("FOLDER:".length)); return; }
    selectDevice(entry.id);
  }

  // --- 方案与启动 ----------------------------------------------------------------

  async function resolveAlbum(): Promise<number | undefined> {
    // 相册必选（规格修订）：存入已有相册需选中；新建分支先建相册拿 id（重名等错误透出行内提示并中止启动）
    let resolvedAlbumId: number | undefined;
    if (albumChoice === "existing") {
      if (albumId === null) {
        setAlbumError(t("wizard.album.required"));
        return;
      }
      resolvedAlbumId = albumId;
    } else {
      const name = newAlbumName.trim();
      if (name === "") {
        setAlbumError(t("wizard.album.nameRequired"));
        return;
      }
      const created = await albumCreate(name);
      if (!created.ok) {
        setAlbumError(created.error ?? t("albums.createFailed"));
        return;
      }
      resolvedAlbumId = created.album.id;
      setAlbums((prev) => [created.album, ...prev]);
      setAlbumId(created.album.id);
      setAlbumChoice("existing");
    }
    return resolvedAlbumId;
  }

  async function startImport(): Promise<void> {
    if (!device || !canStart || startGuard.current) return;
    startGuard.current = true;
    setStarting(true);
    setStartError(null);
    try {
      const resolvedAlbumId = await resolveAlbum();
      if (resolvedAlbumId === undefined) return;
      setAlbumError(null);
      const plan: ImportPlan = {
        sourceId: device.id,
        // 目标照片库（2026-10-09 定案）：落盘布局固定纯时间，由后端按库 root
        // + EXIF 时间落位（dirTemplate/nameTemplate 退役不再下发）
        targetLibraryId: targetLibrary?.id ?? "",
        duplicatePolicy,
        skipImported,
        streams: effectiveStreams,
        mode,
        secondTarget:
          mode === "copy" && secondEnabled && secondRoot.trim()
            ? { targetRoot: secondRoot.trim() }
            : undefined,
        // 勾选即范围：只导入选中的文件（rel_path 集合），引擎按此过滤
        include: files.filter((f) => selected.has(f.path)).map((f) => f.path),
        // 添加到相册（可选）：导入完成后新入库照片加入该相册
        albumId: resolvedAlbumId,
        // 子分组（B4 定案，可选）：留空 = 相册根
        albumSubgroup:
          albumChoice === "existing" && albumSubgroup.trim() !== ""
            ? albumSubgroup.trim()
            : undefined,
      };
      // 竞态防护：sessionStarted 事件可能先于 import_start 返回到达，先挂待归位模式/源类型
      useImportStore.getState().setPendingJobMode(mode);
      useImportStore.getState().setPendingJobSource(device.kind);
      const result = await importStart(plan);
      if (!result.ok) {
        // error=null 表示 invoke 不可用：用通用文案；否则透出后端 Err 原文
        setStartError(result.error ?? t("wizard.startError"));
        return;
      }
      // 双保险：事件先到时 sessionStarted 已用 pending 归位，这里幂等覆盖
      useImportStore.getState().recordJobMode(result.jobId, mode);
      useImportStore.getState().recordJobSource(result.jobId, device.kind);
      // 每次导入可调项回写全局设置（作为后续新建库的默认值；库属性不再回写）
      const { update, save } = useSettingsStore.getState();
      const settings = useSettingsStore.getState().settings;
      update({
        import: {
          ...settings.import,
          duplicatePolicy,
          skipImported,
        },
      });
      // 导入任务已成功启动；偏好保存失败由 store 记录，不能令任务重复启动。
      void save(useSettingsStore.getState().settings).catch(() => {});
      // 任务启动后继续在后台处理；进度由全局任务抽屉展示。
      if (mounted.current) navigate("/gallery");
    } catch (error) {
      setStartError(error instanceof Error ? error.message : String(error));
    } finally {
      startGuard.current = false;
      setStarting(false);
    }
  }

  const blockedReason = scanningFolder !== null ? t("wizard.flow.folderScanning", { path: scanningFolder })
    : !device ? t("wizard.flow.sourceLost")
    : filesLoading || device.scanStatus === "scanning" ? t("wizard.scanBeforeImport")
    : device.scanStatus === "failed" ? t("wizard.deviceScanFailed")
    : policyPending ? t("wizard.flow.checkingSource")
    : !targetLibrary || !targetRoot ? t("wizard.location.noLibrary")
    : selectedCount === 0 ? t("wizard.selectPhotosHint")
    : !albumReady ? t(albumChoice === "new" ? "wizard.album.nameRequired" : "wizard.album.required")
    : !secondReady ? t("wizard.second.required") : null;

  return {
    step, setStep, scanningFolder, selectedId, device, visibleDevices, recentSources, platformCaps,
    selectDevice, browseFolder, selectRecent, filesLoading, files, selected, selectedCount, selectedBytes,
    viewMode, setViewMode, tileSize, setTileSize, selectAll, clearSelection, invertSelection, refreshDevice,
    groups, collapsed, toggleFile, toggleGroup, toggleCollapse, canReview, blockedReason, canStart, starting,
    startImport, targetLibrary, albumChoice, setAlbumChoice, albumId, setAlbumId, albums, newAlbumName,
    setNewAlbumName, albumError, setAlbumError, startError, setStartError, sourceError, setSourceError,
    copyOnly, policyPending, mode, setMode, secondEnabled, setSecondEnabled, importTargetPreview,
    albumSubgroup, setAlbumSubgroup, albumSubgroupNames, navigate, targetRoot, duplicatePolicy,
    setDuplicatePolicy, skipImported, setSkipImported, secondRoot, setSecondRoot, browseSecondRoot,
    secondImportTargetPreview,
  };
}

export type ImportWizardFlow = ReturnType<typeof useImportWizard>;
