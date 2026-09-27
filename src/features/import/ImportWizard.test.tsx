import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import ImportWizard, {
  COL_WIDTHS_KEY,
  PANEL_COLLAPSE_KEY,
  TILE_SIZE_KEY,
  VIEW_MODE_STORAGE_KEY,
  resetThumbCacheForTests,
} from "./ImportWizard";
import { resetImportStoreForTests, useImportStore, type SourceFile } from "@/stores/importStore";
import { useSettingsStore } from "@/stores/settingsStore";
import {
  albumCreate,
  albumList,
  deviceFiles,
  deviceList,
  folderScan,
  fsListDirs,
  importStart,
  thumbGet,
  type ImportPlan,
} from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    importStart: vi.fn(),
    folderScan: vi.fn(),
    fsListDirs: vi.fn(),
    deviceFiles: vi.fn(),
    deviceList: vi.fn(),
    thumbGet: vi.fn(),
    albumList: vi.fn(),
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
const listMock = vi.mocked(fsListDirs);
const openMock = vi.mocked(openDialog);
const deviceFilesMock = vi.mocked(deviceFiles);
const deviceListMock = vi.mocked(deviceList);
const convertMock = vi.mocked(convertFileSrc);
const thumbMock = vi.mocked(thumbGet);
const albumListMock = vi.mocked(albumList);
const albumCreateMock = vi.mocked(albumCreate);

function volumeDevice() {
  return {
    id: "E:",
    name: "SanDisk 64G",
    kind: "volume" as const,
    filesByKind: { photo: 2, raw: 2, video: 0, other: 0 },
    bytesTotal: 1024 * 1024 * 100,
    newFiles: 4,
  };
}

function mtpDevice() {
  return {
    id: "MTP:CAM",
    name: "EOS R5",
    kind: "mtp" as const,
    filesByKind: { photo: 1, raw: 0, video: 1, other: 0 },
    bytesTotal: 1024 * 1024 * 10,
    newFiles: 2,
  };
}

function folderSnapshot(id = "FOLDER:D:\\老照片", name = "老照片") {
  return {
    id,
    name,
    kind: "folder" as const,
    filesByKind: { photo: 3, raw: 1, video: 0, other: 0 },
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
    mk("DCIM/101CANON", "VID_0004.MP4", 6 * 1024 * 1024, "video"),
  ];
}

function seedSession(): void {
  useImportStore.setState({
    devices: [volumeDevice(), mtpDevice()],
    sourceFiles: { "E:": files() },
  });
  useSettingsStore.setState((s) => ({
    settings: {
      ...s.settings,
      libraries: [
        {
          id: "lib1",
          name: "主库",
          dbDir: "I:\\SmartPhoto\\主库",
          photoRoot: "Y:\\照片",
          dirTemplate: "{YYYY}/{MM-DD}/{原文件名}",
          importSubdir: "SmartPhoto",
          configured: true,
        streams: 4,
        },
      ],
      activeLibraryId: "lib1",
    },
  }));
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

function renderWizard(query = "") {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[`/import${query}`]}>
        <Routes>
          <Route path="/import" element={<ImportWizard />} />
          <Route path="/tasks" element={<TasksProbe />} />
          <Route path="/gallery" element={<GalleryProbe />} />
          <Route path="/settings" element={<SettingsProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

// jsdom 无布局：offsetWidth/offsetHeight 恒 0，react-virtual 视口为空会一行都不渲染。
// 统一 mock 出非零视口（组件逻辑不依赖具体尺寸；真实布局由浏览器提供）。
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get: () => 1200,
  });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
    configurable: true,
    get: () => 800,
  });
});

beforeEach(() => {
  resetImportStoreForTests();
  resetThumbCacheForTests();
  startMock.mockReset().mockResolvedValue({ ok: false, error: null });
  scanMock.mockReset().mockResolvedValue(null);
  listMock.mockReset().mockResolvedValue([]);
  openMock.mockReset();
  deviceFilesMock.mockReset().mockResolvedValue(null);
  deviceListMock.mockReset().mockImplementation(async () => useImportStore.getState().devices);
  convertMock.mockReset().mockReturnValue("");
  thumbMock.mockReset().mockResolvedValue(null);
  albumListMock.mockReset().mockResolvedValue([
    { id: 3, name: "青海湖 2026", coverAssetId: null, itemCount: 12, createdAt: "2026-09-01" },
  ]);
  albumCreateMock.mockReset().mockResolvedValue({
    ok: true,
    album: { id: 9, name: "新相册", coverAssetId: null, itemCount: 0, createdAt: "2026-09-03" },
  });
  localStorage.removeItem(VIEW_MODE_STORAGE_KEY);
  localStorage.removeItem(PANEL_COLLAPSE_KEY);
  localStorage.removeItem(COL_WIDTHS_KEY);
  localStorage.removeItem(TILE_SIZE_KEY);
});

describe("ImportWizard 布局与设备", () => {
  it("无设备时左栏显示空态", () => {
    renderWizard();

    expect(screen.getByText(/未检测到设备/)).toBeInTheDocument();
  });

  it("设备初值补拉：错过启动事件的在位设备出现（device_list 兜底）", async () => {
    deviceListMock.mockResolvedValue([volumeDevice()]);

    renderWizard();

    expect(await screen.findByText("SanDisk 64G")).toBeInTheDocument();
    expect(deviceListMock).toHaveBeenCalled();
  });

  it("URL device 参数选中指定设备并展示信息卡", async () => {
    seedSession();

    renderWizard("?device=E:");

    expect(await screen.findByText("SanDisk 64G")).toBeInTheDocument();
    const info = screen.getByTestId("wizard-device-info");
    expect(info).toHaveTextContent("读卡器");
    expect(info).toHaveTextContent("100 MB");
    expect(info).toHaveTextContent("4");
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

  it("设备列表：多设备行可点切换，信息卡与中栏清单随选中切换", async () => {
    seedSession(); // E:(volume, 4 文件) + MTP:CAM(mtp, 2 文件)；sourceFiles 只 seed 了 E:
    deviceFilesMock.mockResolvedValue([
      { id: "0", relPath: "DCIM/B.JPG", size: 10, mtime: "2026-01-01T00:00:00Z" },
      { id: "1", relPath: "DCIM/C.JPG", size: 20, mtime: "2026-01-01T00:00:01Z" },
    ]);
    const user = userEvent.setup();
    renderWizard();

    // 列表两行：E: 默认选中；徽标显示文件总数
    const rows = await screen.findAllByTestId("wizard-device-item");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveAttribute("data-device-id", "E:");
    expect(rows[0]).toHaveAttribute("data-selected", "true");
    expect(rows[0]).toHaveTextContent("SanDisk 64G");
    expect(rows[0]).toHaveTextContent("4 文件");
    expect(rows[1]).toHaveAttribute("data-device-id", "MTP:CAM");
    expect(rows[1]).toHaveAttribute("data-selected", "false");
    expect(rows[1]).toHaveTextContent("2 文件");

    // 点击 MTP 行：选中态切换、信息卡跟随（相机）、中栏随切换重新拉取（device_files(MTP:CAM)）
    await user.click(rows[1]);
    const mtpRow = screen
      .getAllByTestId("wizard-device-item")
      .find((el) => el.getAttribute("data-device-id") === "MTP:CAM");
    expect(mtpRow).toHaveAttribute("data-selected", "true");
    const info = await screen.findByTestId("wizard-device-info");
    expect(info).toHaveTextContent("相机");
    await waitFor(() => expect(deviceFilesMock).toHaveBeenCalledWith("MTP:CAM"));
    const list = await screen.findByTestId("wizard-file-list");
    expect(await within(list).findByText("B.JPG")).toBeInTheDocument();

    // 切回 E:（清单已缓存，不重复拉取）
    await user.click(screen.getAllByTestId("wizard-device-item")[0]);
    expect(await screen.findByTestId("wizard-device-info")).toHaveTextContent("读卡器");
    await within(await screen.findByTestId("wizard-file-list")).findByText("IMG_0001.CR3");
    expect(deviceFilesMock).toHaveBeenCalledTimes(1); // 仅 MTP:CAM 拉过一次
  });
});

describe("源文件树与文件列表", () => {
  it("按目录分组折叠展示；表头统计默认全选；默认列表视图不读缩略图", async () => {
    seedSession();
    renderWizard("?device=E:");

    const tree = await screen.findByTestId("wizard-tree");
    expect(tree).toHaveTextContent("DCIM/100CANON");
    expect(tree).toHaveTextContent("DCIM/101CANON");
    expect(await screen.findByTestId("wizard-table-stats")).toHaveTextContent("已选 4 / 4");
    // 中栏列表（默认视图）：分组行 + 文件行 + 虚拟化滚动容器；无 img
    const list = screen.getByTestId("wizard-file-list");
    expect(screen.getByTestId("wizard-list-scroll")).toBeInTheDocument();
    expect(within(list).getAllByTestId("wizard-list-group")).toHaveLength(2);
    expect(within(list).getByText("IMG_0001.CR3")).toBeInTheDocument();
    expect(within(list).getByText("VID_0004.MP4")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("目录组复选框整组反选；全选/反选按钮生效", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    const stats = await screen.findByTestId("wizard-table-stats");
    expect(stats).toHaveTextContent("已选 4 / 4");

    // 取消整组 DCIM/100CANON
    await user.click(screen.getByRole("checkbox", { name: '选择目录 DCIM/100CANON' }));
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 2 / 4");

    // 反选：另一组被取消，本组恢复
    await user.click(screen.getByRole("button", { name: "反选" }));
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 2 / 4");

    // 全选恢复
    await user.click(screen.getByRole("button", { name: "全选" }));
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 4 / 4");
  });

  it("勾选即导入范围：只选 1 个文件时 plan.include 只含该文件（回归：反选后导入全量的真机 bug）", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();
    startMock.mockResolvedValueOnce({ ok: true, jobId: 9 });

    expect(await screen.findByTestId("wizard-table-stats")).toHaveTextContent("已选 4 / 4");
    // 反选 → 全不选；再只勾 IMG_0003.JPG
    await user.click(screen.getByRole("button", { name: "反选" }));
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 0 / 4");
    await user.click(within(screen.getByTestId("wizard-file-list")).getByRole("checkbox", { name: "IMG_0003.JPG" }));
    expect(screen.getByTestId("wizard-table-stats")).toHaveTextContent("已选 1 / 4");

    await user.click(await screen.findByRole("button", { name: "开始导入" }));

    const plan = startMock.mock.calls[0][0];
    expect(plan.include).toEqual(["E:/DCIM/101CANON/IMG_0003.JPG"]);
  });

  it("折叠目录组隐藏组内文件（左树与中栏共享折叠态）", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    const list = await screen.findByTestId("wizard-file-list");
    await within(list).findByText("IMG_0001.CR3");
    // 点击中栏分组头折叠（按钮包含目录名）
    await user.click(within(list).getByRole("button", { name: /DCIM\/100CANON/ }));
    expect(screen.queryByText("IMG_0001.CR3")).not.toBeInTheDocument();
    expect(within(list).getByText("IMG_0003.JPG")).toBeInTheDocument();
  });
});

describe("方案面板", () => {
  it("导入位置为只读库属性：信息卡展示目标根/模板/预览，无输入框", async () => {
    seedSession();
    renderWizard("?device=E:");

    const card = await screen.findByTestId("wizard-location-card");
    expect(card).toHaveTextContent("Y:\\照片\\SmartPhoto");
    expect(card).toHaveTextContent("{YYYY}/{MM-DD}/{原文件名}");
    // 示例预览沿用 onboarding 的示例值（库模板去掉文件名令牌后渲染）
    expect(screen.getByTestId("wizard-preview")).toHaveTextContent(
      "Y:\\照片\\SmartPhoto\\2026\\09-18\\IMG_0001.CR3",
    );
    // 目标根/模板均不可编辑（原输入与下拉已移除）
    expect(screen.queryByLabelText("目标根目录")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("目录模板")).not.toBeInTheDocument();
    expect(screen.getByTestId("wizard-location-edit")).toBeInTheDocument();
  });

  it("库属性徽标文案为「在设置中修改」，点击跳设置页", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    const badge = await screen.findByTestId("wizard-location-edit");
    expect(badge).toHaveTextContent("库属性 · 在设置中修改");
    expect(badge).not.toHaveTextContent("去选择器修改");

    await user.click(badge);
    expect(await screen.findByTestId("settings-probe")).toBeInTheDocument();
    expect(screen.queryByTestId("wizard-location-card")).not.toBeInTheDocument();
  });

  it("无激活库时信息卡显示未选库提示，开始导入禁用", async () => {
    useImportStore.setState({ devices: [volumeDevice()], sourceFiles: { "E:": files() } });
    // 显式清空库（真实 store 跨用例共享，避免残留上一个 seed）
    useSettingsStore.setState((s) => ({
      settings: { ...s.settings, libraries: [], activeLibraryId: null },
    }));
    renderWizard("?device=E:");

    const card = await screen.findByTestId("wizard-location-card");
    expect(card).toHaveTextContent("尚未选择库");
    expect(screen.getByRole("button", { name: "开始导入" })).toBeDisabled();
  });

  it("MTP 源强制单流（设备信息卡提示），向导不再有并发流数控件", async () => {
    seedSession();
    renderWizard("?device=MTP:CAM");

    const info = await screen.findByTestId("wizard-device-info");
    const mtpRow = within(info).getByTestId("wizard-mtp-streams");
    expect(mtpRow).toHaveTextContent("并发流数");
    expect(mtpRow).toHaveTextContent("1");
    // 协议限制完整说明挂在 title 上
    expect(mtpRow.querySelector("dd")).toHaveAttribute(
      "title",
      "相机（MTP）直连受协议限制，仅支持单流。",
    );
    // 并发流数是库属性：向导方案面板不再渲染该控件
    expect(screen.queryByLabelText("并发流数")).not.toBeInTheDocument();
    expect(screen.queryByTestId("wizard-streams-value")).not.toBeInTheDocument();
  });

  it("plan.streams 从库属性合成：库值 3 → 3；MTP 恒 1", async () => {
    useImportStore.setState({ devices: [volumeDevice(), mtpDevice()] });
    useSettingsStore.setState((s) => ({
      settings: {
        ...s.settings,
        libraries: [
          {
            id: "lib1",
            name: "主库",
            dbDir: "I:\\SmartPhoto\\主库",
            photoRoot: "Y:\\照片",
            dirTemplate: "{YYYY}/{MM-DD}/{原文件名}",
            importSubdir: "SmartPhoto",
            streams: 3,
            configured: true,
          },
        ],
        activeLibraryId: "lib1",
      },
    }));
    const user = userEvent.setup();
    startMock.mockResolvedValue({ ok: true, jobId: 21 });

    renderWizard("?device=E:");
    await user.click(await screen.findByRole("button", { name: "开始导入" }));
    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(startMock.mock.calls[0][0].streams).toBe(3);

    // MTP：设备协议限制恒单流（覆盖库值）
    renderWizard("?device=MTP:CAM");
    await user.click(await screen.findByRole("button", { name: "开始导入" }));
    expect(startMock.mock.calls[1][0].streams).toBe(1);
  });

  it("开始导入：按库属性组装 plan、成功后回画廊（LR 式后台导入，不跳任务中心）", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();
    startMock.mockResolvedValueOnce({ ok: true, jobId: 7 });

    const startButton = await screen.findByRole("button", { name: "开始导入" });
    await user.click(startButton);

    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(screen.queryByTestId("tasks-probe")).not.toBeInTheDocument();
    expect(startMock).toHaveBeenCalledTimes(1);
    const plan: ImportPlan = startMock.mock.calls[0][0];
    expect(plan).toEqual({
      sourceId: "E:",
      targetRoot: "Y:\\照片\\SmartPhoto",
      dirTemplate: "{YYYY}/{MM-DD}",
      nameTemplate: "{原文件名}",
      duplicatePolicy: "skip",
      skipImported: true,
      streams: 4,
      mode: "copy",
      include: [
        "E:/DCIM/100CANON/IMG_0001.CR3",
        "E:/DCIM/100CANON/IMG_0002.CR3",
        "E:/DCIM/101CANON/IMG_0003.JPG",
        "E:/DCIM/101CANON/VID_0004.MP4",
      ],
    });
    // 库属性不回写全局设置（模板仍是全局默认值）
    expect(useSettingsStore.getState().settings.import.dirTemplate).toBe(
      "{YYYY}/{MM-DD}/{原文件名}",
    );
  });

  it("启动失败：透出后端 Err 原文；invoke 不可用时用通用文案", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    // 后端逻辑错误（嵌套守卫）：原文透出
    startMock.mockResolvedValueOnce({ ok: false, error: "目标目录不能位于源目录内" });
    await user.click(await screen.findByRole("button", { name: "开始导入" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("目标目录不能位于源目录内");
    expect(screen.queryByTestId("tasks-probe")).not.toBeInTheDocument();
    expect(screen.queryByTestId("gallery-probe")).not.toBeInTheDocument();

    // invoke 不可用（error=null）：通用文案
    startMock.mockResolvedValueOnce({ ok: false, error: null });
    await user.click(await screen.findByRole("button", { name: "开始导入" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("启动失败");
  });
});

describe("文件系统目录树（LR 式源面板）", () => {
  it("渲染盘符根；hasSubdirs=false 无展开箭头", async () => {
    seedSession();
    listMock.mockResolvedValue([
      { name: "C:", path: "C:\\", hasSubdirs: true },
      { name: "D:", path: "D:\\", hasSubdirs: true },
      { name: "E:", path: "E:\\", hasSubdirs: false },
    ]);
    renderWizard();

    const fs = await screen.findByTestId("wizard-fs");
    expect(fs).toHaveTextContent("C:");
    expect(fs).toHaveTextContent("D:");
    expect(fs).toHaveTextContent("E:");
    expect(screen.getByRole("button", { name: "展开 C:\\" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "展开 E:\\" })).not.toBeInTheDocument();
  });

  it("展开懒加载子级；空目录显示（空）", async () => {
    seedSession();
    const user = userEvent.setup();
    listMock
      .mockResolvedValueOnce([{ name: "C:", path: "C:\\", hasSubdirs: true }])
      .mockResolvedValueOnce([]);

    renderWizard();
    await user.click(await screen.findByRole("button", { name: "展开 C:\\" }));

    expect(await screen.findByText("（空）")).toBeInTheDocument();
    expect(listMock).toHaveBeenCalledTimes(2);
    expect(listMock).toHaveBeenLastCalledWith("C:\\");
  });

  it("点选文件夹：folderScan 入库并选中（树高亮），设备区不出现文件夹条目", async () => {
    seedSession();
    const user = userEvent.setup();
    listMock
      .mockResolvedValueOnce([{ name: "D:", path: "D:\\", hasSubdirs: true }])
      .mockResolvedValueOnce([{ name: "照片", path: "D:\\照片", hasSubdirs: false }]);
    scanMock.mockResolvedValue(folderSnapshot("FOLDER:D:\\照片", "照片"));

    renderWizard();
    await user.click(await screen.findByRole("button", { name: "展开 D:\\" }));
    await user.click(await screen.findByRole("button", { name: "照片" }));

    expect(scanMock).toHaveBeenCalledWith("D:\\照片", true);
    expect(useImportStore.getState().devices.some((d) => d.id === "FOLDER:D:\\照片")).toBe(true);
    // 树节点高亮 = 文件夹选中态
    const node = screen
      .getAllByTestId("wizard-fs-node")
      .find((el) => el.getAttribute("data-path") === "D:\\照片");
    expect(node?.getAttribute("data-selected")).toBe("true");
    // 设备区列表不含 folder 条目；无任何行高亮；信息卡隐藏（统计在中栏，不重复）
    const rows = screen.getAllByTestId("wizard-device-item");
    expect(rows).toHaveLength(2);
    expect(rows.some((el) => el.getAttribute("data-device-id")?.startsWith("FOLDER:"))).toBe(false);
    expect(rows.every((el) => el.getAttribute("data-selected") === "false")).toBe(true);
    expect(screen.queryByTestId("wizard-device-info")).not.toBeInTheDocument();
  });

  it("修复回归：设备在场 + URL device 值失效时点文件夹仍能选中（显式选中优先于回落）", async () => {
    // 复现真机「设备在场时点文件系统文件夹无反应」：
    // 根因是 selectedId 只认 URL——URL 读回值变形/写歪时 some() 匹配失败，
    // 回落 devices[0]（在场旧设备），点文件夹表现为何都没发生。
    // 此处用「无效 ?device= 值」模拟 URL 失效：显式选中必须优先生效。
    seedSession();
    const user = userEvent.setup();
    listMock.mockResolvedValue([{ name: "照片", path: "D:\\照片", hasSubdirs: false }]);
    scanMock.mockResolvedValue(folderSnapshot("FOLDER:D:\\照片", "照片"));

    renderWizard("?device=BROKEN"); // URL 值不在设备表 → 无显式选中时回落 devices[0]=E:
    await user.click(await screen.findByRole("button", { name: "照片" }));

    // 文件夹被显式选中（而非回落 E:）：设备区无 folder 行、无任何行高亮、信息卡隐藏；
    // 选中态由树节点高亮表达
    const rows = screen.getAllByTestId("wizard-device-item");
    expect(rows).toHaveLength(2);
    expect(rows.every((el) => el.getAttribute("data-selected") === "false")).toBe(true);
    expect(screen.queryByTestId("wizard-device-info")).not.toBeInTheDocument();
    const node = screen
      .getAllByTestId("wizard-fs-node")
      .find((el) => el.getAttribute("data-path") === "D:\\照片");
    expect(node?.getAttribute("data-selected")).toBe("true");
  });

  it("浏览…选目录等价于选中该文件夹（openDialog→folderScan 链路）", async () => {
    seedSession();
    const user = userEvent.setup();
    openMock.mockResolvedValue("D:\\老照片");
    scanMock.mockResolvedValue(folderSnapshot());

    renderWizard("?device=E:");
    await user.click(await screen.findByTestId("wizard-fs-browse"));

    expect(openMock).toHaveBeenCalledWith({ directory: true });
    expect(scanMock).toHaveBeenCalledWith("D:\\老照片", true);
    // 选中源切到文件夹：设备区无高亮行、信息卡隐藏（folder 不是设备条目）
    const rows = await screen.findAllByTestId("wizard-device-item");
    expect(rows).toHaveLength(2); // E: + MTP:CAM，无 FOLDER 行
    expect(rows.every((el) => el.getAttribute("data-selected") === "false")).toBe(true);
    expect(screen.queryByTestId("wizard-device-info")).not.toBeInTheDocument();
    // 中栏统计条正常呈现文件夹清单（统计不重复放设备区）
    expect(await screen.findByTestId("wizard-table-stats")).toBeInTheDocument();
  });

  it("folderScan 失败（IPC 不可用）时保持原源并给出红字提示", async () => {
    seedSession();
    const user = userEvent.setup();
    openMock.mockResolvedValue("D:\\老照片");
    scanMock.mockResolvedValue(null);

    renderWizard("?device=E:");
    await user.click(await screen.findByTestId("wizard-fs-browse"));

    expect(scanMock).toHaveBeenCalledWith("D:\\老照片", true);
    expect(useImportStore.getState().devices).toHaveLength(2);
    const info = screen.getByTestId("wizard-device-info");
    expect(info).toHaveTextContent("读卡器");
    // 失败不再静默：中栏一行红字（含目录名），选中其他源后清除
    const error = screen.getByTestId("wizard-source-error");
    expect(error).toHaveTextContent("D:\\老照片");
  });
});

describe("最近使用", () => {
  it("手动选择设备后出现在最近使用，点击可重选并持久化 localStorage", async () => {
    seedSession();
    const user = userEvent.setup();
    renderWizard("?device=E:");

    // 设备列表点击 MTP 行 = 选中该设备
    const mtpRow = (await screen.findAllByTestId("wizard-device-item")).find(
      (el) => el.getAttribute("data-device-id") === "MTP:CAM",
    );
    await user.click(mtpRow!);

    const recent = await screen.findByTestId("wizard-recent");
    expect(recent).toHaveTextContent("EOS R5");
    const stored = JSON.parse(
      localStorage.getItem("smartphoto.import.recentSources") ?? "[]",
    ) as Array<{ id: string }>;
    expect(stored[0]?.id).toBe("MTP:CAM");

    // 切回 E: 后，从最近使用重选相机
    const eRow = screen
      .getAllByTestId("wizard-device-item")
      .find((el) => el.getAttribute("data-device-id") === "E:");
    await user.click(eRow!);
    await user.click(within(recent).getByText("EOS R5"));
    expect(await screen.findByTestId("wizard-device-info")).toHaveTextContent("相机");
  });

  it("选中文件夹也计入最近使用", async () => {
    seedSession();
    const user = userEvent.setup();
    openMock.mockResolvedValue("D:\\老照片");
    scanMock.mockResolvedValue(folderSnapshot());

    renderWizard("?device=E:");
    await user.click(await screen.findByTestId("wizard-fs-browse"));

    const recent = await screen.findByTestId("wizard-recent");
    expect(recent).toHaveTextContent("老照片");
  });
});

describe("导入模式分段条（LR 式顶部切换）", () => {
  it("默认复制：选中态与复制说明文案", async () => {
    seedSession();
    renderWizard("?device=E:");

    expect(await screen.findByTestId("wizard-mode-copy")).toHaveAttribute("aria-checked", "true");
    expect(screen.getByTestId("wizard-mode-move")).toHaveAttribute("aria-checked", "false");
    expect(screen.getByText("复制：源文件保持不动。")).toBeInTheDocument();
  });

  it("切移动：说明切换，plan.mode=move 并记录 jobMode", async () => {
    seedSession();
    const user = userEvent.setup();
    startMock.mockResolvedValueOnce({ ok: true, jobId: 11 });
    renderWizard("?device=E:");

    await user.click(await screen.findByRole("radio", { name: "移动 · 不保留" }));
    expect(screen.getByTestId("wizard-mode-move")).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText(/入库后删除源文件/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "开始导入" }));
    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(startMock.mock.calls[0][0].mode).toBe("move");
    expect(useImportStore.getState().jobModes[11]).toBe("move");
    // 竞态防护：模式已预挂（sessionStarted 事件先到也能归位）
    expect(useImportStore.getState().pendingJobMode).toBe("move");
    // 源类型同样随任务记录（清卡入口判定用；此处 E:=volume）
    expect(useImportStore.getState().pendingJobSource).toBe("volume");
  });
});

describe("查看方式：列表（默认）/ 缩略图网格", () => {
  it("默认列表视图无 img；切到缩略图出现网格并持久化偏好", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    expect(await screen.findByTestId("wizard-file-list")).toBeInTheDocument();
    expect(screen.queryByTestId("wizard-file-grid")).not.toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();

    await user.click(screen.getByTestId("wizard-view-grid"));
    expect(screen.getByTestId("wizard-file-grid")).toBeInTheDocument();
    expect(screen.getByTestId("wizard-grid-scroll")).toBeInTheDocument();
    expect(screen.queryByTestId("wizard-file-list")).not.toBeInTheDocument();
    expect(localStorage.getItem(VIEW_MODE_STORAGE_KEY)).toBe("grid");
  });

  it("缩略图网格：占位块渲染、点击块切换勾选、统计变化、全选恢复", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("wizard-view-grid"));
    const grid = await screen.findByTestId("wizard-file-grid");
    // 预览模式（convertFileSrc 不可用）→ 全部占位
    expect(within(grid).getAllByTestId("tile-raw")).toHaveLength(2);
    expect(within(grid).getByTestId("tile-video")).toBeInTheDocument();
    expect(within(grid).getByTestId("tile-photo")).toBeInTheDocument();
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

  it("M2 缩略图：photo 走 thumb_get(256)→convertFileSrc(小图路径)；RAW/视频不调 thumb_get", async () => {
    seedSession();
    const user = userEvent.setup();
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    thumbMock.mockImplementation(async (path: string) =>
      path === "E:/DCIM/101CANON/IMG_0003.JPG" ? "C:\\thumbCache\\0003_256.jpg" : null,
    );
    renderWizard("?device=E:");

    await user.click(await screen.findByTestId("wizard-view-grid"));
    const img = await screen.findByRole("img", { name: "IMG_0003.JPG" });
    // img src = asset 协议的后端缩略图文件路径（不再解码原图）
    expect(img).toHaveAttribute("src", "asset://C:\\thumbCache\\0003_256.jpg");
    expect(thumbMock).toHaveBeenCalledWith("E:/DCIM/101CANON/IMG_0003.JPG", 256);
    expect(convertMock).toHaveBeenCalledWith("C:\\thumbCache\\0003_256.jpg");
    // RAW/视频不请求缩略图（后端返回 null 的语义在前端直接短路）
    expect(thumbMock).not.toHaveBeenCalledWith(expect.stringContaining("IMG_0001.CR3"), 256);
    expect(screen.queryByRole("img", { name: "IMG_0001.CR3" })).not.toBeInTheDocument();
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
    renderWizard();
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("wizard-view-grid"));
    expect(thumbMock).toHaveBeenCalledWith("D:\\老照片/DCIM/A.JPG", 256);
    expect(await screen.findByRole("img", { name: "A.JPG" })).toHaveAttribute(
      "src",
      "asset://C:\\thumbCache\\a_256.jpg",
    );
  });

  it("MTP 源无文件系统路径：photo 恒占位、不调 thumb_get", async () => {
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
    expect(within(grid).getByTestId("tile-photo")).toBeInTheDocument();
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
    expect(within(grid).getByTestId("tile-photo")).toBeInTheDocument();
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
    expect(within(grid).getByTestId("tile-photo")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("缩略图加载中占位带骨架动画（sp-skeleton）；结算无图退静态", async () => {
    seedSession();
    let resolveThumb: (value: string | null) => void = () => {};
    thumbMock.mockImplementationOnce(
      () => new Promise<string | null>((resolve) => (resolveThumb = resolve)),
    );
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
      within(screen.getByTestId("wizard-file-grid")).getByTestId("tile-photo"),
    ).toBeInTheDocument();
  });
});

describe("左栏分区折叠（LR 式）", () => {
  it("点击标题行折叠/展开设备区并写入 localStorage", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    const toggle = await screen.findByTestId("wizard-section-toggle-devices");
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByTestId("wizard-device-info")).toBeInTheDocument();

    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByTestId("wizard-device-info")).not.toBeInTheDocument();
    expect(JSON.parse(localStorage.getItem(PANEL_COLLAPSE_KEY) ?? "{}")).toEqual({
      devices: true,
      fs: false,
      recent: false,
      source: false,
    });

    await user.click(toggle);
    expect(await screen.findByTestId("wizard-device-info")).toBeInTheDocument();
  });

  it("折叠状态跨挂载恢复；其他分区不受影响", async () => {
    seedSession();
    localStorage.setItem(PANEL_COLLAPSE_KEY, JSON.stringify({ fs: true }));
    renderWizard("?device=E:");

    expect(screen.getByTestId("wizard-section-toggle-fs")).toHaveAttribute("aria-expanded", "false");
    // 折叠时文件系统树内容不渲染（默认后端不可用文案也不出现）
    expect(screen.queryByText(/目录树不可用/)).not.toBeInTheDocument();
    expect(screen.getByTestId("wizard-section-toggle-devices")).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByTestId("wizard-device-info")).toBeInTheDocument();
  });
});

describe("三栏列宽拖动", () => {
  it("默认 270/320；拖左条加宽左栏并持久化，中列 1fr 自动补偿", async () => {
    seedSession();
    renderWizard("?device=E:");
    await screen.findByTestId("wizard-file-list");

    const cols = screen.getByTestId("wizard-columns");
    expect(cols.style.gridTemplateColumns).toBe("270px 6px minmax(0, 1fr) 6px 320px");

    const handle = screen.getByTestId("wizard-col-handle-left");
    fireEvent.pointerDown(handle, { pointerId: 1, clientX: 300 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 340 });
    fireEvent.pointerUp(handle, { pointerId: 1 });

    expect(cols.style.gridTemplateColumns).toBe("310px 6px minmax(0, 1fr) 6px 320px");
    expect(JSON.parse(localStorage.getItem(COL_WIDTHS_KEY) ?? "{}")).toEqual({
      left: 310,
      right: 320,
    });
  });

  it("拖右条右拖变窄并钳制 260；双击分隔条恢复默认", async () => {
    seedSession();
    renderWizard("?device=E:");
    await screen.findByTestId("wizard-file-list");
    const cols = screen.getByTestId("wizard-columns");

    const handle = screen.getByTestId("wizard-col-handle-right");
    // 右拖 200px：右栏 320-200=120 → 钳制到下限 260（中列自动变宽）
    fireEvent.pointerDown(handle, { pointerId: 1, clientX: 100 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 300 });
    fireEvent.pointerUp(handle, { pointerId: 1 });
    expect(cols.style.gridTemplateColumns).toBe("270px 6px minmax(0, 1fr) 6px 260px");

    fireEvent.dblClick(handle);
    expect(cols.style.gridTemplateColumns).toBe("270px 6px minmax(0, 1fr) 6px 320px");
    expect(JSON.parse(localStorage.getItem(COL_WIDTHS_KEY) ?? "{}")).toEqual({
      left: 270,
      right: 320,
    });
  });

  it("左栏钳制 200–400", async () => {
    seedSession();
    renderWizard("?device=E:");
    await screen.findByTestId("wizard-file-list");
    const cols = screen.getByTestId("wizard-columns");

    const handle = screen.getByTestId("wizard-col-handle-left");
    fireEvent.pointerDown(handle, { pointerId: 1, clientX: 0 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: -500 });
    fireEvent.pointerUp(handle, { pointerId: 1 });
    expect(cols.style.gridTemplateColumns).toBe("200px 6px minmax(0, 1fr) 6px 320px");

    fireEvent.pointerDown(handle, { pointerId: 1, clientX: 0 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 999 });
    fireEvent.pointerUp(handle, { pointerId: 1 });
    expect(cols.style.gridTemplateColumns).toBe("400px 6px minmax(0, 1fr) 6px 320px");
  });
});

describe("缩略图档位", () => {
  it("默认标准 120px；大档 150；紧凑档 100 且信息条只显文件名", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("wizard-view-grid"));
    const grid = await screen.findByTestId("wizard-file-grid");
    const firstTile = within(grid).getAllByTestId("wizard-tile")[0];
    expect(firstTile.style.width).toBe("120px");
    expect(firstTile).toHaveTextContent("MB"); // 标准档信息条显示大小

    await user.click(screen.getByTestId("wizard-tile-size-large"));
    expect(within(grid).getAllByTestId("wizard-tile")[0].style.width).toBe("150px");
    expect(localStorage.getItem(TILE_SIZE_KEY)).toBe("large");

    await user.click(screen.getByTestId("wizard-tile-size-compact"));
    const tile = within(grid).getAllByTestId("wizard-tile")[0];
    expect(tile.style.width).toBe("100px");
    expect(tile).not.toHaveTextContent("MB"); // 紧凑档只显文件名
    expect(tile).toHaveTextContent("IMG_0001.CR3");
    expect(localStorage.getItem(TILE_SIZE_KEY)).toBe("compact");
  });
});

describe("双目的地（M2）", () => {
  it("默认关；开启后显示第二目录输入+浏览+必填校验拦住开始", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    expect(screen.getByTestId("wizard-second-toggle")).not.toBeChecked();
    expect(screen.queryByTestId("wizard-second-panel")).not.toBeInTheDocument();

    await user.click(screen.getByTestId("wizard-second-toggle"));
    const panel = await screen.findByTestId("wizard-second-panel");
    expect(within(panel).getByLabelText("第二目标根目录")).toBeInTheDocument();
    expect(panel).toHaveTextContent("目录模板与主目的地相同。");
    // 必填校验：第二目录为空 → 开始导入禁用 + 提示
    expect(screen.getByRole("button", { name: "开始导入" })).toBeDisabled();
    expect(panel).toHaveTextContent("请填写第二目标根目录");

    await user.type(screen.getByTestId("wizard-second-root"), "D:\\照片备份");
    expect(screen.getByRole("button", { name: "开始导入" })).toBeEnabled();
    expect(panel).not.toHaveTextContent("请填写第二目标根目录");
  });

  it("浏览…选择第二目标根目录（openDialog 回填）", async () => {
    seedSession();
    openMock.mockResolvedValue("E:\\备份盘");
    renderWizard("?device=E:");
    const user = userEvent.setup();

    await user.click(screen.getByTestId("wizard-second-toggle"));
    await user.click(await screen.findByTestId("wizard-second-browse"));

    expect(openMock).toHaveBeenCalledWith({ directory: true });
    expect(screen.getByTestId("wizard-second-root")).toHaveValue("E:\\备份盘");
  });

  it("开启并填写 → plan.secondTarget 携带主目录模板；默认关闭时 plan 无该字段", async () => {
    seedSession();
    startMock.mockResolvedValue({ ok: true, jobId: 31 });
    renderWizard("?device=E:");
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("wizard-second-toggle"));
    await user.type(screen.getByTestId("wizard-second-root"), "D:\\照片备份");
    await user.click(screen.getByRole("button", { name: "开始导入" }));

    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(startMock.mock.calls[0][0].secondTarget).toEqual({
      targetRoot: "D:\\照片备份",
      dirTemplate: "{YYYY}/{MM-DD}", // 与主目的地相同
    });
  });

  it("移动互斥：移动模式开关禁用+灰字；开启双目的地切移动自动关且 plan 无 secondTarget", async () => {
    seedSession();
    startMock.mockResolvedValue({ ok: true, jobId: 32 });
    renderWizard("?device=E:");
    const user = userEvent.setup();

    // 复制态开启双目的地并填写
    await user.click(await screen.findByTestId("wizard-second-toggle"));
    await user.type(screen.getByTestId("wizard-second-root"), "D:\\照片备份");
    expect(screen.getByTestId("wizard-second-toggle")).toBeChecked();

    // 切移动 → 开关自动关、面板消失、互斥提示出现
    await user.click(screen.getByRole("radio", { name: "移动 · 不保留" }));
    expect(screen.getByTestId("wizard-second-toggle")).not.toBeChecked();
    expect(screen.getByTestId("wizard-second-toggle")).toBeDisabled();
    expect(screen.queryByTestId("wizard-second-panel")).not.toBeInTheDocument();
    expect(screen.getByText("移动模式不支持双目的地。")).toBeInTheDocument();

    // 切回复制：开关恢复可用（保持关闭，需手动重开），互斥提示消失
    await user.click(screen.getByRole("radio", { name: "复制 · 保留原文件" }));
    expect(screen.getByTestId("wizard-second-toggle")).toBeEnabled();
    expect(screen.queryByText("移动模式不支持双目的地。")).not.toBeInTheDocument();

    // 再切移动并在该模式下启动：plan 不带 secondTarget（后端拒 move+secondTarget，前端互斥保证）
    await user.click(screen.getByRole("radio", { name: "移动 · 不保留" }));
    await user.click(screen.getByRole("button", { name: "开始导入" }));
    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(startMock.mock.calls[0][0].mode).toBe("move");
    expect(startMock.mock.calls[0][0].secondTarget).toBeUndefined();
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
    expect(screen.getByTestId("wizard-tree").className).toContain("sp-scroll");
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


it("扫描万张照片时文件树只渲染可见行，仍能切换视图和勾选", async () => {
  seedSession();
  useImportStore.setState({ devices: [], sourceFiles: {} });
  useImportStore.getState().handleAppEvent({ type: "deviceArrived", id: "camera", kind: "mtp", name: "测试相机" });
  renderWizard("?device=camera");
  await act(async () => {
    useImportStore.getState().handleAppEvent({ type: "deviceFilesProgress", id: "camera", files:
      Array.from({ length: 10000 }, (_, i) => ({ id: String(i), relPath: `DCIM/IMG_${String(i).padStart(5, "0")}.JPG`, size: 100, mtime: "2026-09-19T00:00:00Z" })),
    });
  });
  const tree = screen.getByTestId("wizard-tree");
  expect(await within(tree).findByText("IMG_00000.JPG")).toBeInTheDocument();
  expect(tree.querySelectorAll("[data-tree-row]").length).toBeLessThan(100);
  expect(useImportStore.getState().sourceFiles.camera).toHaveLength(10000);
  const user = userEvent.setup();
  await user.click(within(tree).getByRole("checkbox", { name: "IMG_00000.JPG" }));
  expect(within(tree).getByRole("checkbox", { name: "IMG_00000.JPG" })).not.toBeChecked();
  await user.click(screen.getByTestId("wizard-view-grid"));
  expect(screen.getByTestId("wizard-file-grid")).toBeInTheDocument();
  await act(async () => {
    useImportStore.getState().handleAppEvent({ type: "deviceFilesProgress", id: "camera", files: [
      { id: "next", relPath: "DCIM/NEXT.JPG", size: 100, mtime: "2026-09-19T00:00:00Z" },
    ] });
  });
  expect(useImportStore.getState().sourceFiles.camera).toHaveLength(10001);
  expect(within(tree).getByRole("checkbox", { name: "IMG_00000.JPG" })).not.toBeChecked();
  expect(useImportStore.getState().devices[0].scanStatus).toBe("scanning");
  expect(deviceFilesMock).not.toHaveBeenCalled();
});

// --- 添加到相册（可选）：无 / 选择已有 / 新建；albumId 随导入启动负载 -----------------------

describe("ImportWizard：添加到相册步骤", () => {
  it("默认「不添加」：plan.albumId 不携带（undefined）", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();
    startMock.mockResolvedValueOnce({ ok: true, jobId: 31 });

    await screen.findByTestId("wizard-table-stats");
    expect(screen.getByTestId("wizard-album-none")).toBeChecked();
    await user.click(await screen.findByRole("button", { name: "开始导入" }));

    const plan = startMock.mock.calls[0][0] as ImportPlan;
    expect(plan.albumId).toBeUndefined();
  });

  it("「选择已有」：下拉列 album_list，选中后 plan.albumId=该相册 id", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();
    startMock.mockResolvedValueOnce({ ok: true, jobId: 32 });

    await screen.findByTestId("wizard-table-stats");
    await user.click(screen.getByTestId("wizard-album-existing"));
    const select = await screen.findByTestId("wizard-album-select");
    expect(within(select).getAllByRole("option").map((o) => o.textContent)).toContain(
      "青海湖 2026（12）",
    );
    await user.selectOptions(select, "3");
    await user.click(screen.getByRole("button", { name: "开始导入" }));

    const plan = startMock.mock.calls[0][0] as ImportPlan;
    expect(plan.albumId).toBe(3);
  });

  it("「新建相册」：先建相册再启动，plan.albumId=新相册 id；重名错误行内提示且不启动", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();
    startMock.mockResolvedValue({ ok: true, jobId: 33 });
    albumCreateMock
      .mockResolvedValueOnce({ ok: false, error: "同名相册已存在" })
      .mockResolvedValueOnce({
        ok: true,
        album: { id: 9, name: "旅行", coverAssetId: null, itemCount: 0, createdAt: "2026-09-03" },
      });

    await screen.findByTestId("wizard-table-stats");
    await user.click(screen.getByTestId("wizard-album-new"));

    // 空名拦截
    await user.click(screen.getByRole("button", { name: "开始导入" }));
    expect(await screen.findByTestId("wizard-album-error")).toHaveTextContent("请填写新相册名称");
    expect(startMock).not.toHaveBeenCalled();

    // 重名错误透传
    const input = screen.getByTestId("wizard-album-new-name");
    await user.type(input, "旅行");
    await user.click(screen.getByRole("button", { name: "开始导入" }));
    expect(await screen.findByTestId("wizard-album-error")).toHaveTextContent("同名相册已存在");
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
});
