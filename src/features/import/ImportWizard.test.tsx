import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import ImportWizard, {
  TILE_SIZE_KEY,
  VIEW_MODE_STORAGE_KEY,
  resetThumbCacheForTests,
  fetchThumbUrl,
} from "./ImportWizard";
import { resetImportStoreForTests, useImportStore, type SourceFile } from "@/stores/importStore";
import { useSettingsStore } from "@/stores/settingsStore";
import {
  albumCreate,
  albumList,
  albumSubgroups,
  deviceFiles,
  deviceList,
  folderScan,
  importStart,
  photoLibraryList,
  platformCapabilities,
  thumbGet,
  deviceThumbGet,
  type ImportPlan,
  type PhotoLibrary,
} from "@/ipc/api";

/**
 * 导入向导测试（对齐 2026-10-09 之后的 UI 与契约）：
 * - 三步流：来源（设备卡片/选择文件夹/最近使用）→ 挑选（列表/缩略图网格）
 *   → 确认抽屉（原生 <dialog>，jsdom 需补 showModal 桩）。
 * - 契约：导入目标 = 照片库（photo_library_list；ImportPlan.targetLibraryId），
 *   落盘布局固定纯时间 `{库root}/{拍摄年}/{拍摄月}/{原文件名}`。
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    importStart: vi.fn(),
    folderScan: vi.fn(),
    fsListDirs: vi.fn(),
    deviceFiles: vi.fn(),
    deviceList: vi.fn(),
    photoLibraryList: vi.fn(),
    platformCapabilities: vi.fn(),
    thumbGet: vi.fn(),
    deviceThumbGet: vi.fn(),
    albumList: vi.fn(),
    albumSubgroups: vi.fn(),
    albumCreate: vi.fn(),
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
import { open as openDialog } from "@tauri-apps/plugin-dialog";

// 覆盖全局 core mock：convertFileSrc 可控（默认返回空串=预览模式无 asset 协议）
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));
import { convertFileSrc } from "@tauri-apps/api/core";

const startMock = vi.mocked(importStart);
const scanMock = vi.mocked(folderScan);
const openMock = vi.mocked(openDialog);
const deviceFilesMock = vi.mocked(deviceFiles);
const deviceListMock = vi.mocked(deviceList);
const platformCapsMock = vi.mocked(platformCapabilities);
const convertMock = vi.mocked(convertFileSrc);
const thumbMock = vi.mocked(thumbGet);
const deviceThumbMock = vi.mocked(deviceThumbGet);
const albumListMock = vi.mocked(albumList);
const albumSubgroupsMock = vi.mocked(albumSubgroups);
const albumCreateMock = vi.mocked(albumCreate);
const photoLibraryListMock = vi.mocked(photoLibraryList);

/** 目标照片库（2026-10-09 单库多照片库：导入目标 = 选照片库，替代旧 targetRoot） */
const PHOTO_LIB: PhotoLibrary = {
  id: "lib1",
  name: "主照片库",
  rootPath: "Y:\\照片",
  createdAt: "2026-10-09T00:00:00Z",
  status: "online",
  assetCount: 0,
  sizeBytes: 0,
};

function volumeDevice() {
  return {
    id: "E:",
    name: "SanDisk 64G",
    kind: "volume" as const,
    filesByKind: { photo: 2, raw: 2, other: 0 },
    bytesTotal: 1024 * 1024 * 100,
    newFiles: 4,
  };
}

function mtpDevice() {
  return {
    id: "MTP:CAM",
    name: "EOS R5",
    kind: "mtp" as const,
    filesByKind: { photo: 2, raw: 0, other: 0 },
    bytesTotal: 1024 * 1024 * 10,
    newFiles: 2,
  };
}

function folderSnapshot(id = "FOLDER:D:\\老照片", name = "老照片") {
  return {
    id,
    name,
    kind: "folder" as const,
    filesByKind: { photo: 3, raw: 1, other: 0 },
    bytesTotal: 1024 * 1024 * 10,
    newFiles: 4,
  };
}

function files(): SourceFile[] {
  const mk = (dir: string, name: string, size: number, kind: SourceFile["kind"]): SourceFile => ({
    path: `E:/${dir}/${name}`,
    dir,
    name,
    size,
    kind,
  });
  return [
    mk("DCIM/100CANON", "IMG_0001.CR3", 45 * 1024 * 1024, "raw"),
    mk("DCIM/100CANON", "IMG_0002.CR3", 44 * 1024 * 1024, "raw"),
    mk("DCIM/101CANON", "IMG_0003.JPG", 5 * 1024 * 1024, "photo"),
    mk("DCIM/101CANON", "IMG_0004.JPG", 6 * 1024 * 1024, "photo"),
  ];
}

function seedSession(): void {
  useImportStore.setState({
    devices: [volumeDevice(), mtpDevice()],
    sourceFiles: { "E:": files() },
  });
  photoLibraryListMock.mockResolvedValue([PHOTO_LIB]);
}

function TasksProbe() {
  return <div data-testid="tasks-probe">TASKS_PAGE</div>;
}

function GalleryProbe() {
  return <div data-testid="gallery-probe">GALLERY_PAGE</div>;
}

function SettingsProbe() {
  return <div data-testid="settings-probe">SETTINGS_PAGE</div>;
}

function StorageProbe() {
  return <div data-testid="storage-probe">STORAGE_PAGE</div>;
}

function renderWizard(query = "") {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[`/import${query}`]}>
        <Routes>
          <Route path="/import" element={<ImportWizard />} />
          <Route path="/tasks" element={<TasksProbe />} />
          <Route path="/gallery" element={<GalleryProbe />} />
          <Route path="/settings" element={<SettingsProbe />} />
          <Route path="/storage" element={<StorageProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

/** 渲染并走到确认抽屉（等待照片库/相册等异步初始值就绪）；返回 render 结果供卸载 */
async function openReview(user: ReturnType<typeof userEvent.setup>, query = "?device=E:") {
  const view = renderWizard(query);
  await screen.findByTestId("wizard-table-stats");
  const next = await screen.findByTestId("wizard-review-next");
  await waitFor(() => expect(next).toBeEnabled());
  await user.click(next);
  await screen.findByTestId("wizard-review-drawer");
  // 等默认相册「未分组」预选就位（相册必选契约）
  await waitFor(() => expect(screen.getByTestId("wizard-album-select")).toHaveValue("1"));
  return view;
}

// jsdom 无布局：offsetWidth/offsetHeight 恒 0，react-virtual 视口为空会一行都不渲染。
// 统一 mock 出非零视口（组件逻辑不依赖具体尺寸；真实布局由浏览器提供）。
// 另：jsdom 未实现 <dialog>.showModal/close（确认抽屉用原生模态），测试内补桩。
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get: () => 1200,
  });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
    configurable: true,
    get: () => 800,
  });
  HTMLDialogElement.prototype.showModal = function (this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function (this: HTMLDialogElement) {
    this.removeAttribute("open");
  };
});

beforeEach(() => {
  resetImportStoreForTests();
  resetThumbCacheForTests();
  localStorage.clear();
  localStorage.setItem(VIEW_MODE_STORAGE_KEY, "list");
  deviceThumbMock.mockReset().mockResolvedValue(null);
  startMock.mockReset().mockResolvedValue({ ok: false, error: null });
  scanMock.mockReset().mockResolvedValue(null);
  openMock.mockReset();
  deviceFilesMock.mockReset().mockResolvedValue(null);
  deviceListMock.mockReset().mockImplementation(async () => useImportStore.getState().devices);
  // 默认无照片库（「未选库」用例的默认态）；有用例经 seedSession 指定目标库
  photoLibraryListMock.mockReset().mockResolvedValue([]);
  platformCapsMock.mockReset().mockResolvedValue({
    filesystemRoots: true,
    volumeDevices: true,
    portableDevices: true,
    hotplug: true,
    systemOpen: true,
    fileClipboard: true,
    fileReveal: true,
    documentUris: false,
  });
  convertMock.mockReset().mockReturnValue("");
  thumbMock.mockReset().mockResolvedValue(null);
  albumListMock.mockReset().mockResolvedValue([
    { id: 1, name: "未分组", coverAssetId: null, itemCount: 0, createdAt: "2026-09-01" },
    { id: 3, name: "青海湖 2026", coverAssetId: null, itemCount: 12, createdAt: "2025-12-15" },
  ]);
  albumSubgroupsMock.mockReset().mockResolvedValue([]);
  albumCreateMock.mockReset().mockResolvedValue({
    ok: true,
    album: { id: 9, name: "新相册", coverAssetId: null, itemCount: 0, createdAt: "2026-09-03" },
  });
});

// --- 来源步（设备卡片 / 选择文件夹 / 最近使用） ---------------------------------------

describe("来源步", () => {
  it("无设备时空态文案，选择文件夹入口仍在", async () => {
    renderWizard();

    expect(await screen.findByText(/未检测到设备/)).toBeInTheDocument();
    expect(screen.getByTestId("wizard-choose-folder")).toBeInTheDocument();
    expect(screen.queryByTestId("wizard-device-item")).not.toBeInTheDocument();
  });

  it("原生设备发现未适配时提示使用文件夹导入", async () => {
    platformCapsMock.mockResolvedValue({
      filesystemRoots: true,
      volumeDevices: false,
      portableDevices: false,
      hotplug: false,
      systemOpen: true,
      fileClipboard: true,
      fileReveal: true,
      documentUris: false,
    });
    renderWizard();

    expect(
      await screen.findByText(/暂不支持自动发现相机设备/),
    ).toBeInTheDocument();
  });

  it("设备初值补拉：错过启动事件的在位设备出现（device_list 兜底）", async () => {
    deviceListMock.mockResolvedValue([volumeDevice()]);

    renderWizard();

    expect((await screen.findAllByText("SanDisk 64G"))[0]).toBeInTheDocument();
    expect(deviceListMock).toHaveBeenCalled();
  });

  it("设备卡片列表：名称与文件数徽标；点击进入挑选步；未插卡行禁用", async () => {
    useImportStore.setState({ devices: [volumeDevice(), mtpDevice()] });
    const user = userEvent.setup();
    renderWizard();

    const rows = await screen.findAllByTestId("wizard-device-item");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveAttribute("data-device-id", "E:");
    expect(rows[0]).toHaveTextContent("SanDisk 64G");
    expect(rows[0]).toHaveTextContent("4 文件");
    expect(rows[1]).toHaveTextContent("EOS R5");
    expect(rows[1]).toHaveTextContent("2 文件");

    // 点击进入挑选步：统计条出现且来源切换按钮显示设备名
    await user.click(rows[0]);
    expect(await screen.findByTestId("wizard-table-stats")).toBeInTheDocument();
    expect(screen.getByTestId("wizard-source-toggle")).toHaveTextContent("SanDisk 64G");
    expect(screen.getByTestId("wizard-source-toggle")).toHaveTextContent("更换来源");
  });

  it("URL ?device= 直达挑选步；来源切换按钮回来源步", async () => {
    seedSession();
    const user = userEvent.setup();
    renderWizard("?device=E:");

    expect(await screen.findByTestId("wizard-table-stats")).toBeInTheDocument();
    expect(screen.getByTestId("wizard-source-toggle")).toHaveTextContent("SanDisk 64G");

    await user.click(screen.getByTestId("wizard-source-toggle"));
    expect(await screen.findByTestId("wizard-source-picker")).toBeInTheDocument();
  });

  it("选中源后自动拉取 device_files 填充清单（无需手动 seed）", async () => {
    // 只 seed 设备，不 seed sourceFiles：应触发 deviceFiles → setSourceFiles
    useImportStore.setState({ devices: [volumeDevice()] });
    deviceFilesMock.mockResolvedValue([
      { id: "0", relPath: "DCIM/100CANON/IMG_0009.NEF", size: 3000, mtime: "2026-01-01T00:00:00Z" },
      { id: "1", relPath: "DCIM/101CANON/IMG_0010.JPG", size: 4000, mtime: "2026-01-01T00:00:01Z" },
    ]);

    renderWizard("?device=E:");

    const list = await screen.findByTestId("wizard-file-list");
    expect(await within(list).findByText("IMG_0009.NEF")).toBeInTheDocument();
    expect(within(list).getByText("IMG_0010.JPG")).toBeInTheDocument();
    expect(await screen.findByTestId("wizard-table-stats")).toHaveTextContent("已选 2 / 2");
    expect(deviceFilesMock).toHaveBeenCalledWith("E:");
  });

  it("多设备切换：经来源步换设备重新拉取清单；切回用缓存不重复拉", async () => {
    seedSession(); // E:(volume, 4 文件) + MTP:CAM(mtp, 2 文件)；sourceFiles 只 seed 了 E:
    deviceFilesMock.mockResolvedValue([
      { id: "0", relPath: "DCIM/B.JPG", size: 10, mtime: "2026-01-01T00:00:00Z" },
      { id: "1", relPath: "DCIM/C.JPG", size: 20, mtime: "2026-01-01T00:00:01Z" },
    ]);
    const user = userEvent.setup();
    renderWizard("?device=E:");
    await screen.findByTestId("wizard-table-stats");

    // 回来源步点 MTP 卡片：清单随切换重新拉取
    await user.click(screen.getByTestId("wizard-source-toggle"));
    await user.click(
      (await screen.findAllByTestId("wizard-device-item")).find(
        (el) => el.getAttribute("data-device-id") === "MTP:CAM",
      )!,
    );
    await waitFor(() => expect(deviceFilesMock).toHaveBeenCalledWith("MTP:CAM"));
    const list = await screen.findByTestId("wizard-file-list");
    expect(await within(list).findByText("B.JPG")).toBeInTheDocument();

    // 切回 E:（清单已缓存，不重复拉取）
    await user.click(screen.getByTestId("wizard-source-toggle"));
    await user.click(
      (await screen.findAllByTestId("wizard-device-item")).find(
        (el) => el.getAttribute("data-device-id") === "E:",
      )!,
    );
    expect(await within(await screen.findByTestId("wizard-file-list")).findByText("IMG_0001.CR3")).toBeInTheDocument();
    expect(deviceFilesMock).toHaveBeenCalledTimes(1); // 仅 MTP:CAM 拉过一次
  });

  it("空读卡器显示未插卡：卡片禁用，不被请求扫描清单", async () => {
    useImportStore.setState({ devices: [
      { ...volumeDevice(), id: "G:", name: "读卡器 (G:)", mediaPresent: false, filesByKind: { photo: 0, raw: 0, other: 0 } },
      volumeDevice(),
    ] });
    renderWizard();

    const rows = await screen.findAllByTestId("wizard-device-item");
    expect(rows[0]).toBeDisabled();
    expect(rows[0]).toHaveTextContent("未插卡");
    expect(rows[1]).toBeEnabled();
    expect(deviceFilesMock).not.toHaveBeenCalledWith("G:");
  });
});

describe("选择文件夹与最近使用", () => {
  it("选择文件夹：openDialog → folderScan 快照入册并自动选中进挑选步", async () => {
    useImportStore.setState({ devices: [volumeDevice(), mtpDevice()] });
    openMock.mockResolvedValue("D:\\老照片");
    scanMock.mockResolvedValue(folderSnapshot());
    deviceFilesMock.mockResolvedValue([
      { id: "0", relPath: "DCIM/A.JPG", size: 10, mtime: "2026-01-01T00:00:00Z" },
    ]);
    const user = userEvent.setup();
    renderWizard();

    await user.click(await screen.findByTestId("wizard-choose-folder"));

    expect(openMock).toHaveBeenCalledWith({ directory: true });
    expect(scanMock).toHaveBeenCalledWith("D:\\老照片", true);
    // 文件夹成为当前源（挑选步呈现其清单）；设备区不含 folder 条目（来源步才可见）
    expect(useImportStore.getState().devices.some((d) => d.id === "FOLDER:D:\\老照片")).toBe(true);
    expect(await screen.findByTestId("wizard-table-stats")).toHaveTextContent("已选 1 / 1");
    expect(await within(await screen.findByTestId("wizard-file-list")).findByText("A.JPG")).toBeInTheDocument();
  });

  it("folderScan 失败（IPC 不可用等）：提示错误并停留来源步，不产生死条目", async () => {
    useImportStore.setState({ devices: [volumeDevice()] });
    openMock.mockResolvedValue("D:\\老照片");
    scanMock.mockResolvedValue(null);
    const user = userEvent.setup();
    renderWizard();

    await user.click(await screen.findByTestId("wizard-choose-folder"));

    expect(scanMock).toHaveBeenCalledWith("D:\\老照片", true);
    expect(useImportStore.getState().devices).toHaveLength(1);
    // 停留来源步（设备卡片仍在），错误经 ErrorModal 呈现并带目录
    expect(await screen.findByTestId("wizard-source-picker")).toBeInTheDocument();
    const error = await screen.findByRole("alertdialog");
    expect(error).toHaveTextContent("D:\\老照片");
  });

  it("选中文件夹计入最近使用；从最近使用可重选", async () => {
    useImportStore.setState({ devices: [volumeDevice(), mtpDevice()] });
    openMock.mockResolvedValue("D:\\老照片");
    scanMock.mockResolvedValue(folderSnapshot());
    const user = userEvent.setup();
    renderWizard();

    await user.click(await screen.findByTestId("wizard-choose-folder"));
    expect(useImportStore.getState().recentSources[0]?.id).toBe("FOLDER:D:\\老照片");
    const stored = JSON.parse(
      localStorage.getItem("smartphoto.import.recentSources") ?? "[]",
    ) as Array<{ id: string }>;
    expect(stored[0]?.id).toBe("FOLDER:D:\\老照片");

    // 回来源步：最近使用区可见该文件夹，点击重选（重新 folderScan）
    await user.click(screen.getByTestId("wizard-source-toggle"));
    const recent = await screen.findByTestId("wizard-recent");
    expect(recent).toHaveTextContent("老照片");
    await user.click(within(recent).getByTestId("wizard-recent-item"));
    expect(scanMock).toHaveBeenCalledWith("D:\\老照片", true);
  });
});

// --- 挑选步：列表 / 缩略图网格 ---------------------------------------------------------

describe("挑选步：列表与网格", () => {
  it("按目录分组折叠展示；表头统计默认全选；列表视图不读缩略图", async () => {
    seedSession();
    renderWizard("?device=E:");

    const list = await screen.findByTestId("wizard-file-list");
    expect(await screen.findByTestId("wizard-table-stats")).toHaveTextContent("已选 4 / 4");
    expect(screen.getByTestId("wizard-list-scroll")).toBeInTheDocument();
    expect(within(list).getAllByTestId("wizard-list-group")).toHaveLength(2);
    expect(within(list).getByText("DCIM/100CANON")).toBeInTheDocument();
    expect(within(list).getByText("IMG_0001.CR3")).toBeInTheDocument();
    expect(within(list).getByText("IMG_0004.JPG")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("目录组复选框整组反选；全选/反选按钮生效（反选在「更多操作」弹出内）", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    expect(await screen.findByTestId("wizard-table-stats")).toHaveTextContent("已选 4 / 4");

    // 取消整组 DCIM/100CANON
    await user.click(within(screen.getByTestId("wizard-file-list")).getByRole("checkbox", { name: "选择目录 DCIM/100CANON" }));
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 2 / 4");

    // 反选（弹出菜单）：另一组被取消，本组恢复
    await user.click(screen.getByTitle("更多操作"));
    await user.click(screen.getByRole("button", { name: "反选" }));
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 2 / 4");

    // 全选恢复（全选后按钮语义翻转为「取消全选」）
    await user.click(screen.getByRole("button", { name: "全选" }));
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 4 / 4");
    expect(screen.getByRole("button", { name: "取消全选" })).toBeInTheDocument();
  });

  it("折叠目录组隐藏组内文件", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    const list = await screen.findByTestId("wizard-file-list");
    await within(list).findByText("IMG_0001.CR3");
    await user.click(within(list).getByRole("button", { name: /DCIM\/100CANON/ }));
    expect(screen.queryByText("IMG_0001.CR3")).not.toBeInTheDocument();
    expect(within(list).getByText("IMG_0003.JPG")).toBeInTheDocument();
  });

  it("中间的目录勾选支持部分选中、整组取消，并同步两个视图", async () => {
    seedSession();
    const user = userEvent.setup();
    renderWizard("?device=E:");
    const list = await screen.findByTestId("wizard-file-list");
    const group = within(list).getByRole("checkbox", { name: "选择目录 DCIM/100CANON" });
    expect(group).toBeChecked();
    await user.click(within(list).getByRole("checkbox", { name: "IMG_0001.CR3" }));
    expect(group).toBePartiallyChecked();
    await user.click(group);
    expect(group).toBeChecked();
    await user.click(group);
    expect(group).not.toBeChecked();
    expect(within(list).getByText("IMG_0001.CR3")).toBeInTheDocument();
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 2 / 4");
    await user.click(screen.getByTestId("wizard-view-grid"));
    const grid = screen.getByTestId("wizard-file-grid");
    const gridGroup = within(grid).getByRole("checkbox", { name: "选择目录 DCIM/100CANON" });
    expect(gridGroup).not.toBeChecked();
    await user.click(gridGroup);
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 4 / 4");
  });

  it("默认缩略图网格（无偏好时）；切列表持久化偏好", async () => {
    seedSession();
    localStorage.removeItem(VIEW_MODE_STORAGE_KEY);
    renderWizard("?device=E:");
    const user = userEvent.setup();

    expect(await screen.findByTestId("wizard-file-grid")).toBeInTheDocument();
    expect(screen.queryByTestId("wizard-file-list")).not.toBeInTheDocument();

    await user.click(screen.getByTestId("wizard-view-list"));
    expect(screen.getByTestId("wizard-file-list")).toBeInTheDocument();
    expect(screen.queryByTestId("wizard-file-grid")).not.toBeInTheDocument();
    expect(localStorage.getItem(VIEW_MODE_STORAGE_KEY)).toBe("list");
  });

  it("缩略图网格：占位块渲染、点击块切换勾选、统计变化、全选恢复", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("wizard-view-grid"));
    const grid = await screen.findByTestId("wizard-file-grid");
    // 预览模式（convertFileSrc 不可用）→ 全部占位
    expect(within(grid).getAllByTestId("tile-raw")).toHaveLength(2);
    expect(within(grid).getAllByTestId("tile-photo")).toHaveLength(2);
    expect(document.querySelector("img")).toBeNull();
    expect(within(grid).getAllByTestId("wizard-grid-group")).toHaveLength(2);

    // 点击 photo 块取消勾选
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 4 / 4");
    const tile = within(grid)
      .getAllByTestId("wizard-tile")
      .find((el) => el.getAttribute("data-path") === "E:/DCIM/101CANON/IMG_0003.JPG");
    expect(tile).toBeDefined();
    await user.click(tile!);
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 3 / 4");
    expect(tile).toHaveAttribute("data-selected", "false");

    // 工具栏全选恢复
    await user.click(screen.getByRole("button", { name: "全选" }));
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 4 / 4");
  });

  it("切换视图不丢勾选", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    // 列表视图取消 IMG_0001
    const list = await screen.findByTestId("wizard-file-list");
    await user.click(within(list).getByRole("checkbox", { name: "IMG_0001.CR3" }));
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 3 / 4");

    await user.click(screen.getByTestId("wizard-view-grid"));
    const grid = await screen.findByTestId("wizard-file-grid");
    const tile1 = within(grid)
      .getAllByTestId("wizard-tile")
      .find((el) => el.getAttribute("data-path") === "E:/DCIM/100CANON/IMG_0001.CR3");
    const tile3 = within(grid)
      .getAllByTestId("wizard-tile")
      .find((el) => el.getAttribute("data-path") === "E:/DCIM/101CANON/IMG_0003.JPG");
    expect(tile1).toHaveAttribute("data-selected", "false");
    expect(tile3).toHaveAttribute("data-selected", "true");
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 3 / 4");
  });

  it("照片和 RAW 都通过后端缩略图显示，不加载原文件", async () => {
    seedSession();
    const user = userEvent.setup();
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    thumbMock.mockImplementation(async (path: string) =>
      path.endsWith("IMG_0003.JPG") ? "C:\\thumbCache\\0003_256.jpg" : path.endsWith("IMG_0001.CR3") ? "C:\\thumbCache\\0001_256.jpg" : null,
    );
    renderWizard("?device=E:");

    await user.click(await screen.findByTestId("wizard-view-grid"));
    const img = await screen.findByRole("img", { name: "IMG_0003.JPG" });
    // img src = asset 协议的后端缩略图文件路径（不再解码原图）
    expect(img).toHaveAttribute("src", "asset://C:\\thumbCache\\0003_256.jpg");
    expect(thumbMock).toHaveBeenCalledWith("E:/DCIM/101CANON/IMG_0003.JPG", 256);
    expect(convertMock).toHaveBeenCalledWith("C:\\thumbCache\\0003_256.jpg");
    expect(thumbMock).toHaveBeenCalledWith("E:/DCIM/100CANON/IMG_0001.CR3", 256);
    expect(await screen.findByRole("img", { name: "IMG_0001.CR3" })).toHaveAttribute("src", "asset://C:\\thumbCache\\0001_256.jpg");
  });

  it("folder 源 absPath = id 去 FOLDER: 前缀 + / + relPath（作为 thumb_get 入参）", async () => {
    useImportStore.setState({
      devices: [folderSnapshot()],
      sourceFiles: {
        "FOLDER:D:\\老照片": [
          { path: "DCIM/A.JPG", dir: "DCIM", name: "A.JPG", size: 10, kind: "photo" },
        ],
      },
    });
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    thumbMock.mockResolvedValue("C:\\thumbCache\\a_256.jpg");
    const user = userEvent.setup();
    renderWizard(`?device=${encodeURIComponent("FOLDER:D:\\老照片")}`);

    await user.click(await screen.findByTestId("wizard-view-grid"));
    expect(thumbMock).toHaveBeenCalledWith("D:\\老照片/DCIM/A.JPG", 256);
    expect(await screen.findByRole("img", { name: "A.JPG" })).toHaveAttribute(
      "src",
      "asset://C:\\thumbCache\\a_256.jpg",
    );
  });

  it("MTP 清单保留持久对象 ID，扫描完成后读取相机缩略资源", async () => {
    seedSession();
    useImportStore.setState({ devices: [{ ...mtpDevice(), scanStatus: "scanning" }], sourceFiles: {
      "MTP:CAM": [{ path: "DCIM/A.NEF", dir: "DCIM", name: "A.NEF", size: 42, kind: "raw", objectId: "persistent-42", mtime: "today" }],
    } });
    localStorage.setItem(VIEW_MODE_STORAGE_KEY, "grid");
    deviceThumbMock.mockResolvedValue("C:\\thumbCache\\mtp.jpg");
    convertMock.mockImplementation((path) => `asset://${path}`);
    renderWizard("?device=MTP:CAM");
    expect(await screen.findByText("扫描后加载预览")).toBeInTheDocument();
    expect(deviceThumbMock).not.toHaveBeenCalled();
    act(() => useImportStore.setState({ devices: [{ ...mtpDevice(), scanStatus: "ready" }] }));
    expect(await screen.findByRole("img", { name: "A.NEF" })).toHaveAttribute("src", "asset://C:\\thumbCache\\mtp.jpg");
    expect(deviceThumbMock).toHaveBeenCalledWith("MTP:CAM", "persistent-42", "today:42", 256);
    expect(thumbMock).not.toHaveBeenCalled();
  });

  it("没有对象 ID 的旧 MTP 清单保持明确占位，不错误请求本地路径", async () => {
    useImportStore.setState({
      devices: [mtpDevice()],
      sourceFiles: {
        "MTP:CAM": [{ path: "DCIM/B.JPG", dir: "DCIM", name: "B.JPG", size: 10, kind: "photo" }],
      },
    });
    renderWizard("?device=MTP:CAM");
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("wizard-view-grid"));
    expect(thumbMock).not.toHaveBeenCalled();
    const grid = await screen.findByTestId("wizard-file-grid");
    expect(within(grid).getAllByTestId("tile-photo")[0]).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("thumb_get 返回 null（后端无缩略图/RAW）：静默占位不崩溃", async () => {
    seedSession();
    thumbMock.mockResolvedValue(null);
    renderWizard("?device=E:");
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("wizard-view-grid"));
    await screen.findByTestId("wizard-file-grid");
    await waitFor(() => expect(thumbMock).toHaveBeenCalled());
    const grid = screen.getByTestId("wizard-file-grid");
    expect(within(grid).getAllByTestId("tile-photo")[0]).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("会话缓存：同 absPath 只调一次 thumb_get（视图切换复用不重复 IPC）", async () => {
    seedSession();
    thumbMock.mockResolvedValue("C:\\thumbCache\\x_256.jpg");
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderWizard("?device=E:");
    const user = userEvent.setup();

    // 进网格 → 回列表 → 再进网格：IMG_0003 的 thumb_get 只调一次
    await user.click(await screen.findByTestId("wizard-view-grid"));
    await screen.findByRole("img", { name: "IMG_0003.JPG" });
    await user.click(screen.getByTestId("wizard-view-list"));
    await screen.findByTestId("wizard-file-list");
    await user.click(screen.getByTestId("wizard-view-grid"));
    await screen.findByRole("img", { name: "IMG_0003.JPG" });

    const calls = thumbMock.mock.calls.filter(([p]) => p === "E:/DCIM/101CANON/IMG_0003.JPG");
    expect(calls).toHaveLength(1);
  });

  it("临时失败后可重试，图片实际加载完成才结束加载动画", async () => {
    seedSession();
    let failFirst = true;
    thumbMock.mockImplementation(async () => (failFirst ? null : "C:\\thumbCache\\ok.jpg"));
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderWizard("?device=E:");
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("wizard-view-grid"));
    // 第一次 null → 占位 + 重试按钮（瓦片各自带重试入口；取 IMG_0003 的）
    const grid = await screen.findByTestId("wizard-file-grid");
    const tile = within(grid)
      .getAllByTestId("wizard-tile")
      .find((el) => el.getAttribute("data-path") === "E:/DCIM/101CANON/IMG_0003.JPG")!;
    const retry = await within(tile).findByRole("button", { name: "重试" });
    failFirst = false;
    await user.click(retry);
    const img = await screen.findByRole("img", { name: "IMG_0003.JPG" });
    expect(img).toHaveAttribute("src", "asset://C:\\thumbCache\\ok.jpg");
  });

  it("convertFileSrc 抛错时静默占位不崩溃", async () => {
    seedSession();
    thumbMock.mockResolvedValue("C:\\thumbCache\\bad.jpg");
    convertMock.mockImplementation(() => {
      throw new Error("no tauri internals");
    });
    renderWizard("?device=E:");
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("wizard-view-grid"));
    const grid = await screen.findByTestId("wizard-file-grid");
    expect(within(grid).getAllByTestId("tile-photo")[0]).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("缩略图加载中占位带骨架动画（sp-skeleton）；结算无图退静态", async () => {
    seedSession();
    let resolveThumb: (value: string | null) => void = () => {};
    thumbMock.mockImplementation((path) => path.endsWith("IMG_0003.JPG")
      ? new Promise<string | null>((resolve) => (resolveThumb = resolve)) : Promise.resolve(null));
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderWizard("?device=E:");
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("wizard-view-grid"));
    // photo tile 在途：缩略区挂骨架类
    const area = document.querySelector('[data-testid="tile-photo"]')?.parentElement;
    expect(area?.className).toContain("sp-skeleton");

    // 结算为无图：退静态（无动画类）
    resolveThumb(null);
    await waitFor(() => expect(area?.className).not.toContain("sp-skeleton"));
    expect(
      within(screen.getByTestId("wizard-file-grid")).getAllByTestId("tile-photo")[0],
    ).toBeInTheDocument();
  });

  it("缩略图档位：标准 160px、大档 220px 两档并持久化", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("wizard-view-grid"));
    const grid = await screen.findByTestId("wizard-file-grid");
    const firstTile = within(grid).getAllByTestId("wizard-tile")[0];
    expect(firstTile.style.width).toBe("160px");

    await user.click(screen.getByTestId("wizard-tile-size-large"));
    expect(within(grid).getAllByTestId("wizard-tile")[0].style.width).toBe("220px");
    expect(localStorage.getItem(TILE_SIZE_KEY)).toBe("large");
    expect(within(screen.getByTestId("wizard-tile-size")).getAllByRole("radio")).toHaveLength(2);
  });

  it("扫描万张照片时列表只渲染可见行，仍能切换视图和勾选", async () => {
    seedSession();
    useImportStore.setState({ devices: [], sourceFiles: {} });
    useImportStore.getState().handleAppEvent({ type: "deviceArrived", id: "camera", kind: "mtp", name: "测试相机" });
    renderWizard("?device=camera");
    await act(async () => {
      useImportStore.getState().handleAppEvent({ type: "deviceFilesProgress", id: "camera", files:
        Array.from({ length: 10000 }, (_, i) => ({ id: String(i), relPath: `DCIM/IMG_${String(i).padStart(5, "0")}.JPG`, size: 100, mtime: "2026-09-19T00:00:00Z" })),
      });
    });
    const list = await screen.findByTestId("wizard-file-list");
    expect(await within(list).findByText("IMG_00000.JPG")).toBeInTheDocument();
    expect(list.querySelectorAll("[data-index]").length).toBeLessThan(100);
    const user = userEvent.setup();
    await user.click(within(list).getByRole("checkbox", { name: "IMG_00000.JPG" }));
    expect(within(list).getByRole("checkbox", { name: "IMG_00000.JPG" })).not.toBeChecked();
    await user.click(screen.getByTestId("wizard-view-grid"));
    expect(screen.getByTestId("wizard-file-grid")).toBeInTheDocument();
    await act(async () => {
      useImportStore.getState().handleAppEvent({ type: "deviceFilesProgress", id: "camera", files: [
        { id: "next", relPath: "DCIM/NEXT.JPG", size: 100, mtime: "2026-09-19T00:00:00Z" },
      ] });
    });
    expect(useImportStore.getState().sourceFiles.camera).toHaveLength(10001);
    expect(within(list).getByRole("checkbox", { name: "IMG_00000.JPG" })).not.toBeChecked();
    expect(useImportStore.getState().devices[0].scanStatus).toBe("scanning");
    expect(deviceFilesMock).not.toHaveBeenCalled();
  });
});

it("扫描尚未结束时立即展示增量文件，完成后不二次读取相机", async () => {
  seedSession();
  useImportStore.setState({ devices: [], sourceFiles: {} });
  useImportStore.getState().handleAppEvent({ type: "deviceArrived", id: "camera", kind: "mtp", name: "测试相机" });
  renderWizard("?device=camera");
  await act(async () => {
    useImportStore.getState().handleAppEvent({ type: "deviceFilesProgress", id: "camera", files: [
      { id: "1", relPath: "DCIM/LIVE.JPG", size: 100, mtime: "2026-09-19T00:00:00Z" },
    ] });
  });
  expect((await screen.findAllByText("LIVE.JPG")).length).toBeGreaterThan(0);
  expect(useImportStore.getState().devices[0].scanStatus).toBe("scanning");
  expect(deviceFilesMock).not.toHaveBeenCalled();
  await act(async () => {
    useImportStore.getState().handleAppEvent({ type: "deviceScanned", id: "camera", kind: "mtp", name: "测试相机", snapshot: { ...mtpDevice(), id: "camera" } });
  });
  expect(screen.getAllByText("LIVE.JPG").length).toBeGreaterThan(0);
  expect(deviceFilesMock).not.toHaveBeenCalled();
});

it("快速滚动时跳过已离开预加载区域且尚未开始的预览请求", async () => {
  const releases: Array<() => void> = [];
  thumbMock.mockImplementation(() => new Promise((resolve) => releases.push(() => resolve(null))));
  const busy = Array.from({ length: 8 }, (_, i) => fetchThumbUrl(`busy-${i}.ARW`));
  await waitFor(() => expect(thumbMock).toHaveBeenCalledTimes(8));
  let visible = true;
  const skipped = fetchThumbUrl("offscreen.ARW", "", () => visible);
  visible = false;
  releases[0]();
  await expect(skipped).resolves.toBeNull();
  expect(thumbMock).not.toHaveBeenCalledWith("offscreen.ARW", 256);
  releases.slice(1).forEach((release) => release());
  await Promise.all(busy);
});

// --- 确认抽屉与导入方案（目标照片库 + 纯时间布局契约） ---------------------------------

describe("确认抽屉与导入方案", () => {
  it("无照片库：状态行提示「尚未选择照片库」，继续按钮禁用", async () => {
    useImportStore.setState({ devices: [volumeDevice()], sourceFiles: { "E:": files() } });
    renderWizard("?device=E:");

    await screen.findByTestId("wizard-table-stats");
    expect(screen.getByTestId("wizard-review-next")).toBeDisabled();
    expect(await screen.findByText(/尚未选择照片库/)).toBeInTheDocument();
  });

  it("进入抽屉与返回；成功启动后回画廊（LR 式后台导入，不跳任务中心）", async () => {
    seedSession();
    const user = userEvent.setup();
    startMock.mockResolvedValueOnce({ ok: true, jobId: 7 });
    await openReview(user);

    // 抽屉在档：返回按钮回挑选步
    expect(screen.getByTestId("wizard-review-drawer")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "关闭" }));
    expect(await screen.findByTestId("wizard-table-stats")).toBeInTheDocument();

    // 再进抽屉启动：回画廊、不跳任务中心
    await user.click(screen.getByTestId("wizard-review-next"));
    await screen.findByTestId("wizard-review-drawer");
    await user.click(await screen.findByRole("button", { name: "开始导入" }));
    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(screen.queryByTestId("tasks-probe")).not.toBeInTheDocument();
  });

  it("开始导入：按目标照片库组装 plan（targetLibraryId，纯时间布局无模板字段）", async () => {
    seedSession();
    const user = userEvent.setup();
    startMock.mockResolvedValueOnce({ ok: true, jobId: 7 });
    await openReview(user);

    await user.click(await screen.findByRole("button", { name: "开始导入" }));
    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(startMock).toHaveBeenCalledTimes(1);
    const plan: ImportPlan = startMock.mock.calls[0][0];
    expect(plan).toEqual({
      sourceId: "E:",
      // 2026-10-09 单库多照片库：目标 = 照片库 id（dirTemplate/nameTemplate 退役）
      targetLibraryId: "lib1",
      // 相册必选：默认预选系统保底相册「未分组」（纯逻辑引用）
      albumId: 1,
      duplicatePolicy: "skip",
      skipImported: true,
      streams: 4,
      mode: "copy",
      include: [
        "E:/DCIM/100CANON/IMG_0001.CR3",
        "E:/DCIM/100CANON/IMG_0002.CR3",
        "E:/DCIM/101CANON/IMG_0003.JPG",
        "E:/DCIM/101CANON/IMG_0004.JPG",
      ],
    });
  });

  it("勾选即导入范围：只选 1 个文件时 plan.include 只含该文件", async () => {
    seedSession();
    const user = userEvent.setup();
    startMock.mockResolvedValueOnce({ ok: true, jobId: 9 });
    renderWizard("?device=E:");

    expect(await screen.findByTestId("wizard-table-stats")).toHaveTextContent("已选 4 / 4");
    // 全不选；再只勾 IMG_0003.JPG
    await user.click(screen.getByTitle("更多操作"));
    await user.click(screen.getByRole("button", { name: "反选" }));
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 0 / 4");
    await user.click(within(screen.getByTestId("wizard-file-list")).getByRole("checkbox", { name: "IMG_0003.JPG" }));
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 1 / 4");

    await user.click(screen.getByTestId("wizard-review-next"));
    await screen.findByTestId("wizard-review-drawer");
    await user.click(await screen.findByRole("button", { name: "开始导入" }));
    await screen.findByTestId("gallery-probe");

    const plan = startMock.mock.calls[0][0];
    expect(plan.include).toEqual(["E:/DCIM/101CANON/IMG_0003.JPG"]);
  });

  it("plan.streams 应用级合成：默认 4", async () => {
    seedSession();
    const user = userEvent.setup();
    startMock.mockResolvedValue({ ok: true, jobId: 21 });

    await openReview(user);
    await user.click(await screen.findByRole("button", { name: "开始导入" }));
    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(startMock.mock.calls[0][0].streams).toBe(4);
  });

  it("MTP 源 plan.streams 恒 1（协议限制）", async () => {
    useImportStore.setState({
      devices: [volumeDevice(), mtpDevice()],
      sourceFiles: {
        "E:": files(),
        "MTP:CAM": [{ path: "A.JPG", dir: "", name: "A.JPG", kind: "photo", size: 10 }],
      },
    });
    photoLibraryListMock.mockResolvedValue([PHOTO_LIB]);
    const user = userEvent.setup();
    startMock.mockResolvedValue({ ok: true, jobId: 22 });

    await openReview(user, "?device=MTP:CAM");
    await user.click(await screen.findByRole("button", { name: "开始导入" }));
    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(startMock.mock.calls[0][0].streams).toBe(1);
  });

  it("导入位置卡：目标照片库 root 只读展示 + 纯时间公式预览；徽标跳存储页（M5）", async () => {
    seedSession();
    const user = userEvent.setup();
    await openReview(user);

    const card = screen.getByTestId("wizard-location-card");
    expect(card).toHaveTextContent("Y:\\照片");
    // 纯时间布局公式（物理层无相册/子组维度）
    expect(screen.getByTestId("wizard-album-path-preview")).toHaveTextContent(
      "导入位置预览",
    );
    expect(screen.getByTestId("wizard-album-path-preview")).toHaveTextContent(
      "Y:\\照片\\{拍摄年}\\{拍摄月}\\{原文件名}",
    );
    // 目标根/布局均不可编辑
    expect(screen.queryByLabelText("目标根目录")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("目录模板")).not.toBeInTheDocument();

    // 徽标「照片库属性 · 点击修改」→ 照片库登记管理在存储页（旧设置页库 tab 已退役）
    const badge = screen.getByTestId("wizard-location-edit");
    expect(badge).toHaveTextContent("照片库属性 · 点击修改");
    await user.click(badge);
    expect(await screen.findByTestId("storage-probe")).toBeInTheDocument();
    expect(screen.queryByTestId("wizard-location-card")).not.toBeInTheDocument();
  });

  it("切换/新建相册不改路径预览（相册只是逻辑引用，不参与落位）", async () => {
    seedSession();
    const user = userEvent.setup();
    await openReview(user);

    // 改选青海湖 2026（createdAt 2025-12-15）→ 预览仍是纯时间公式
    await user.selectOptions(screen.getByTestId("wizard-album-select"), "3");
    expect(screen.getByTestId("wizard-album-path-preview")).toHaveTextContent(
      "Y:\\照片\\{拍摄年}\\{拍摄月}\\{原文件名}",
    );

    // 新建相册输入名 → 预览同样不变
    await user.selectOptions(screen.getByTestId("wizard-album-select"), "new");
    await user.type(screen.getByTestId("wizard-album-new-name"), "婚礼0927");
    expect(screen.getByTestId("wizard-album-path-preview")).toHaveTextContent(
      "Y:\\照片\\{拍摄年}\\{拍摄月}\\{原文件名}",
    );
  });

  it("「选择已有」：下拉列 album_list 带张数，选中后 plan.albumId=该相册 id", async () => {
    seedSession();
    const user = userEvent.setup();
    startMock.mockResolvedValueOnce({ ok: true, jobId: 32 });
    await openReview(user);

    const select = screen.getByTestId("wizard-album-select");
    expect(within(select).getAllByRole("option").map((o) => o.textContent)).toContain(
      "青海湖 2026（12）",
    );
    await user.selectOptions(select, "3");
    await user.click(screen.getByRole("button", { name: "开始导入" }));
    await screen.findByTestId("gallery-probe");

    const plan = startMock.mock.calls[0][0] as ImportPlan;
    expect(plan.albumId).toBe(3);
  });

  it("「新建相册」：空名拦住开始；重名错误行内提示；修正后建册再启动 plan.albumId=新册 id", async () => {
    seedSession();
    const user = userEvent.setup();
    startMock.mockResolvedValue({ ok: true, jobId: 33 });
    albumCreateMock
      .mockResolvedValueOnce({ ok: false, error: "同名相册已存在" })
      .mockResolvedValueOnce({
        ok: true,
        album: { id: 9, name: "旅行", coverAssetId: null, itemCount: 0, createdAt: "2026-09-03" },
      });
    await openReview(user);

    await user.selectOptions(screen.getByTestId("wizard-album-select"), "new");
    const input = screen.getByTestId("wizard-album-new-name");

    // 空名：开始按钮禁用 + 底部状态行提示（相册必选契约）
    const start = screen.getByRole("button", { name: "开始导入" });
    expect(start).toBeDisabled();
    expect(screen.getByText("请填写新相册名称")).toBeInTheDocument();

    // 重名错误透传（行内 alert，不关抽屉、不启动）
    await user.type(input, "旅行");
    await user.click(screen.getByRole("button", { name: "开始导入" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("同名相册已存在");
    expect(albumCreateMock).toHaveBeenCalledWith("旅行");
    expect(startMock).not.toHaveBeenCalled();

    // 修正后成功：album_create → import_start(albumId=9)
    await user.clear(input);
    await user.type(input, "旅行 2");
    await user.click(screen.getByRole("button", { name: "开始导入" }));
    await waitFor(() => expect(startMock).toHaveBeenCalledTimes(1));
    expect(albumCreateMock).toHaveBeenLastCalledWith("旅行 2");
    const plan = startMock.mock.calls[0][0] as ImportPlan;
    expect(plan.albumId).toBe(9);
  });

  it("未选相册（清单为空）：开始禁用并提示「请选择相册」，不启动", async () => {
    seedSession();
    albumListMock.mockResolvedValue([]);
    const user = userEvent.setup();
    startMock.mockResolvedValueOnce({ ok: true, jobId: 35 });
    renderWizard("?device=E:");
    await screen.findByTestId("wizard-table-stats");
    await user.click((await screen.findAllByTestId("wizard-review-next"))[0]);
    await screen.findByTestId("wizard-review-drawer");

    const start = await screen.findByRole("button", { name: "开始导入" });
    expect(start).toBeDisabled();
    expect(screen.getByText("请选择相册")).toBeInTheDocument();
    expect(startMock).not.toHaveBeenCalled();
  });

  it("子分组（可选）：留空 = 相册根不带 albumSubgroup", async () => {
    seedSession();
    const user = userEvent.setup();
    startMock.mockResolvedValue({ ok: true, jobId: 36 });
    await openReview(user);

    if (!screen.getByTestId("wizard-advanced").hasAttribute("open")) await user.click(screen.getByText("更多导入选项"));
    expect(screen.getByTestId("wizard-album-subgroup")).toHaveValue("");

    await user.click(screen.getByRole("button", { name: "开始导入" }));
    await screen.findByTestId("gallery-probe");
    expect((startMock.mock.calls[0][0] as ImportPlan).albumSubgroup).toBeUndefined();
  });

  it("子分组：datalist 建议名前置去重；输入新名随 plan.albumSubgroup 下发", async () => {
    seedSession();
    const user = userEvent.setup();
    startMock.mockResolvedValue({ ok: true, jobId: 37 });
    await openReview(user);

    if (!screen.getByTestId("wizard-advanced").hasAttribute("open")) await user.click(screen.getByText("更多导入选项"));
    // 改选相册 3 → album_subgroups 供 datalist：建议 = [原片, 精选, 成片]（「原片」缺位前置，与既有去重）
    albumSubgroupsMock.mockResolvedValue([
      { name: "精选", itemCount: 2 },
      { name: "成片", itemCount: 1 },
    ]);
    await user.selectOptions(screen.getByTestId("wizard-album-select"), "3");
    const datalist = document.getElementById("wizard-subgroup-options") as HTMLDataListElement;
    await waitFor(() => expect(datalist?.options).toHaveLength(3));
    expect(Array.from(datalist.options).map((o) => o.value)).toEqual(["原片", "精选", "成片"]);

    // 输入新名随 plan 下发
    fireEvent.change(screen.getByTestId("wizard-album-subgroup"), { target: { value: "机内直出" } });
    await user.click(screen.getByRole("button", { name: "开始导入" }));
    await waitFor(() => expect(startMock).toHaveBeenCalledTimes(1));
    expect((startMock.mock.calls[0][0] as ImportPlan).albumSubgroup).toBe("机内直出");
  });

  it("导入模式：默认复制；切移动 plan.mode=move 并记录 jobMode；引用模式不设第二目的地", async () => {
    seedSession();
    const user = userEvent.setup();
    startMock.mockResolvedValue({ ok: true, jobId: 11 });
    const view = await openReview(user);

    const mode = screen.getByLabelText("导入模式");
    expect(mode).toHaveValue("copy");
    expect(screen.getByText("复制：源文件保持不动。")).toBeInTheDocument();

    // 切移动：说明切换 + plan.mode=move + jobMode 预挂（sessionStarted 先到也能归位）
    await user.selectOptions(mode, "move");
    expect(screen.getByText(/入库后删除源文件/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "开始导入" }));
    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(startMock.mock.calls[0][0].mode).toBe("move");
    expect(useImportStore.getState().jobModes[11]).toBe("move");
    expect(useImportStore.getState().pendingJobMode).toBe("move");
    expect(useImportStore.getState().pendingJobSource).toBe("volume");
    view.unmount();

    // 引用模式：仅登记，不设复制目的地
    await openReview(user);
    await user.selectOptions(screen.getByLabelText("导入模式"), "reference");
    await user.click(screen.getByRole("button", { name: "开始导入" }));
    await screen.findByTestId("gallery-probe");
    expect(startMock.mock.calls[1][0]).toMatchObject({ mode: "reference", secondTarget: undefined });
  });

  it("MTP 相机不提供引用/移动（copyOnly：模式下拉禁用且恒复制）", async () => {
    useImportStore.setState({
      devices: [mtpDevice()],
      sourceFiles: { "MTP:CAM": [{ path: "A.JPG", dir: "", name: "A.JPG", kind: "photo", size: 10 }] },
    });
    photoLibraryListMock.mockResolvedValue([PHOTO_LIB]);
    const user = userEvent.setup();
    renderWizard("?device=MTP:CAM");
    await screen.findByTestId("wizard-table-stats");
    await waitFor(() => expect(screen.getByTestId("wizard-review-next")).toBeEnabled());
    await user.click(screen.getByTestId("wizard-review-next"));
    await screen.findByTestId("wizard-review-drawer");

    // copyOnly：模式下拉禁用且值恒 copy
    const mode = await screen.findByLabelText("导入模式");
    expect(mode).toBeDisabled();
    expect(mode).toHaveValue("copy");
  });

  it("查重策略与跳过已导入：可调且成功启动后回写全局设置", async () => {
    seedSession();
    const user = userEvent.setup();
    startMock.mockResolvedValue({ ok: true, jobId: 40 });
    await openReview(user);

    if (!screen.getByTestId("wizard-advanced").hasAttribute("open")) await user.click(screen.getByText("更多导入选项"));
    await user.click(screen.getByRole("radio", { name: "重命名导入（追加 _1 后缀）" }));
    await user.click(screen.getByLabelText("跳过已导入文件（按内容指纹）"));
    await user.click(screen.getByRole("button", { name: "开始导入" }));
    await screen.findByTestId("gallery-probe");

    expect(startMock.mock.calls[0][0]).toMatchObject({ duplicatePolicy: "rename", skipImported: false });
    expect(useSettingsStore.getState().settings.import.duplicatePolicy).toBe("rename");
    expect(useSettingsStore.getState().settings.import.skipImported).toBe(false);
  });

  it("双目的地：默认关；开启后必填拦住开始；填写后 plan.secondTarget={targetRoot}；预览同公式", async () => {
    seedSession();
    const user = userEvent.setup();
    renderWizard("?device=E:");
    await screen.findByTestId("wizard-table-stats");
    await user.click(screen.getByTestId("wizard-review-next"));
    await screen.findByTestId("wizard-review-drawer");

    if (!screen.getByTestId("wizard-advanced").hasAttribute("open")) await user.click(screen.getByText("更多导入选项"));
    expect(screen.getByTestId("wizard-second-toggle")).not.toBeChecked();
    expect(screen.queryByTestId("wizard-second-panel")).not.toBeInTheDocument();

    await user.click(screen.getByTestId("wizard-second-toggle"));
    const panel = await screen.findByTestId("wizard-second-panel");
    expect(within(panel).getByLabelText("第二目标根目录")).toBeInTheDocument();
    // 必填校验：第二目录为空 → 开始禁用 + 提示（无路径预览）
    expect(screen.getByRole("button", { name: "开始导入" })).toBeDisabled();
    expect(panel).toHaveTextContent("请填写第二目标根目录");
    expect(screen.queryByTestId("wizard-second-path-preview")).not.toBeInTheDocument();

    await user.type(screen.getByTestId("wizard-second-root"), "D:\\照片备份");
    expect(screen.getByRole("button", { name: "开始导入" })).toBeEnabled();
    expect(panel).not.toHaveTextContent("请填写第二目标根目录");
    // 第二目的地预览 = 第二根目录 + 同一纯时间公式
    expect(screen.getByTestId("wizard-second-path-preview")).toHaveTextContent(
      "D:\\照片备份\\{拍摄年}\\{拍摄月}\\{原文件名}",
    );

    startMock.mockResolvedValueOnce({ ok: true, jobId: 31 });
    await user.click(screen.getByRole("button", { name: "开始导入" }));
    await screen.findByTestId("gallery-probe");
    expect(startMock.mock.calls[0][0].secondTarget).toEqual({
      targetRoot: "D:\\照片备份",
    });
  });

  it("移动互斥：移动模式下双目的地开关禁用；该模式下启动 plan 无 secondTarget", async () => {
    seedSession();
    const user = userEvent.setup();
    startMock.mockResolvedValue({ ok: true, jobId: 32 });
    renderWizard("?device=E:");
    await screen.findByTestId("wizard-table-stats");
    await user.click(screen.getByTestId("wizard-review-next"));
    await screen.findByTestId("wizard-review-drawer");

    // 复制态开启双目的地并填写
    if (!screen.getByTestId("wizard-advanced").hasAttribute("open")) await user.click(screen.getByText("更多导入选项"));
    await user.click(screen.getByTestId("wizard-second-toggle"));
    await user.type(screen.getByTestId("wizard-second-root"), "D:\\照片备份");
    expect(screen.getByTestId("wizard-second-toggle")).toBeChecked();

    // 切移动 → 开关自动关、面板消失、互斥提示出现
    await user.selectOptions(screen.getByLabelText("导入模式"), "move");
    expect(screen.getByTestId("wizard-second-toggle")).not.toBeChecked();
    expect(screen.getByTestId("wizard-second-toggle")).toBeDisabled();
    expect(screen.queryByTestId("wizard-second-panel")).not.toBeInTheDocument();
    expect(screen.getByText("移动模式不支持双目的地。")).toBeInTheDocument();

    // 移动模式启动：plan 不带 secondTarget（后端拒 move+secondTarget，前端互斥保证）
    await user.click(screen.getByRole("button", { name: "开始导入" }));
    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(startMock.mock.calls[0][0].mode).toBe("move");
    expect(startMock.mock.calls[0][0].secondTarget).toBeUndefined();
  });

  it("启动失败：透出后端 Err 原文；invoke 不可用时用通用文案", async () => {
    seedSession();
    const user = userEvent.setup();
    await openReview(user);

    // 后端逻辑错误：原文透出（行内 alert）
    startMock.mockResolvedValueOnce({ ok: false, error: "目标目录不能位于源目录内" });
    await user.click(await screen.findByRole("button", { name: "开始导入" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("目标目录不能位于源目录内");
    expect(screen.queryByTestId("tasks-probe")).not.toBeInTheDocument();
    // 失败留在抽屉可重试
    expect(screen.getByTestId("wizard-review-drawer")).toBeInTheDocument();

    // invoke 不可用（error=null）→ 通用文案
    startMock.mockResolvedValueOnce({ ok: false, error: null });
    await user.click(screen.getByRole("button", { name: "开始导入" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
  });
});

describe("全局滚动条主题", () => {
  it("app.css 含 webkit 滚动条规则与 sp-scroll；滚动容器挂 sp-scroll", async () => {
    // vitest 把 CSS import 转译为空模块，测试运行于 Node 直接读源文件断言规则存在
    // @ts-ignore 项目未安装 @types/node，仅测试内使用
    const { readFileSync } = await import("node:fs");
    // @ts-ignore 同上
    const { cwd } = await import("node:process");
    const css = readFileSync(`${cwd()}/src/styles/app.css`, "utf-8") as string;
    expect(css).toContain("::-webkit-scrollbar-thumb");
    expect(css).toContain("::-webkit-scrollbar-thumb:hover");
    expect(css).toContain("scrollbar-gutter: stable");

    seedSession();
    renderWizard("?device=E:");
    expect(await screen.findByTestId("wizard-file-list")).toBeInTheDocument();
    expect(screen.getByTestId("wizard-list-scroll").className).toContain("sp-scroll");
    const user = userEvent.setup();
    await user.click(screen.getByTestId("wizard-view-grid"));
    expect(screen.getByTestId("wizard-grid-scroll").className).toContain("sp-scroll");
  });
});
