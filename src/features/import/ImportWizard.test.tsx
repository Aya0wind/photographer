import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import ImportWizard, {
  COL_WIDTHS_KEY,
  PANEL_COLLAPSE_KEY,
  TILE_SIZE_KEY,
  VIEW_MODE_STORAGE_KEY,
} from "./ImportWizard";
import { resetImportStoreForTests, useImportStore, type SourceFile } from "@/stores/importStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { deviceFiles, folderScan, fsListDirs, importStart, type ImportPlan } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    importStart: vi.fn(),
    folderScan: vi.fn(),
    fsListDirs: vi.fn(),
    deviceFiles: vi.fn(),
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
const convertMock = vi.mocked(convertFileSrc);

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
        { id: "lib1", name: "主库", dbDir: "I:\\SmartPhoto\\主库", photoRoot: "Y:\\照片" },
      ],
      activeLibraryId: "lib1",
    },
  }));
}

function TasksProbe() {
  return <div data-testid="tasks-probe">TASKS_PAGE</div>;
}

function renderWizard(query = "") {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[`/import${query}`]}>
        <Routes>
          <Route path="/import" element={<ImportWizard />} />
          <Route path="/tasks" element={<TasksProbe />} />
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
  startMock.mockReset().mockResolvedValue(null);
  scanMock.mockReset().mockResolvedValue(null);
  listMock.mockReset().mockResolvedValue([]);
  openMock.mockReset();
  deviceFilesMock.mockReset().mockResolvedValue(null);
  convertMock.mockReset().mockReturnValue("");
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
  it("目标根目录默认取激活库 photoRoot+导入子目录；切换预设实时预览", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    const rootInput = await screen.findByLabelText("目标根目录");
    expect(rootInput).toHaveValue("Y:\\照片\\SmartPhoto");

    const preview = screen.getByTestId("wizard-preview");
    // 默认预设来自设置 {YYYY}/{MM-DD}/{原文件名} → 年/日期
    expect(preview).toHaveTextContent("Y:\\照片\\SmartPhoto\\2026\\09-18\\IMG_0001.CR3");

    await user.selectOptions(screen.getByLabelText("目录模板"), "ym");
    expect(screen.getByTestId("wizard-preview")).toHaveTextContent("Y:\\照片\\SmartPhoto\\2026\\09\\IMG_0001.CR3");

    await user.selectOptions(screen.getByLabelText("目录模板"), "orig");
    expect(screen.getByTestId("wizard-preview")).toHaveTextContent(
      "Y:\\照片\\SmartPhoto\\DCIM\\100CANON\\IMG_0001.CR3",
    );
  });

  it("MTP 源强制单流并禁用调节", async () => {
    seedSession();
    renderWizard("?device=MTP:CAM");

    expect(await screen.findByText(/仅支持单流/)).toBeInTheDocument();
    expect(screen.getByTestId("wizard-streams-value")).toHaveTextContent("1");
    expect(screen.getByLabelText("并发流数")).toBeDisabled();
  });

  it("开始导入：按面板组装 plan、跳转任务中心，并回写设置", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();
    startMock.mockResolvedValueOnce(7);

    const startButton = await screen.findByRole("button", { name: "开始导入" });
    await user.click(startButton);

    expect(await screen.findByTestId("tasks-probe")).toBeInTheDocument();
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
    });
    // 方案回写设置
    expect(useSettingsStore.getState().settings.import.dirTemplate).toBe("{YYYY}/{MM-DD}");
  });

  it("启动失败（后端不可用）时显示错误且不跳转", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();
    startMock.mockResolvedValueOnce(null);

    await user.click(await screen.findByRole("button", { name: "开始导入" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("启动失败");
    expect(screen.queryByTestId("tasks-probe")).not.toBeInTheDocument();
  });

  it("自定义模板出现输入框并可校验未知令牌", async () => {
    seedSession();
    renderWizard("?device=E:");

    await userEvent.selectOptions(await screen.findByLabelText("目录模板"), "custom");
    const input = screen.getByLabelText("自定义…");
    // 注意：userEvent.type 会把 {XX} 当作特殊按键语法，这里用 change 直填
    fireEvent.change(input, { target: { value: "{YYYY}/{XX}" } });
    expect(screen.getByRole("alert")).toHaveTextContent("未知令牌：{XX}");
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

  it("点选文件夹：folderScan 入库+选中+高亮，信息卡显示本地文件夹", async () => {
    seedSession();
    const user = userEvent.setup();
    listMock
      .mockResolvedValueOnce([{ name: "D:", path: "D:\\", hasSubdirs: true }])
      .mockResolvedValueOnce([{ name: "照片", path: "D:\\照片", hasSubdirs: false }]);
    scanMock.mockResolvedValue(folderSnapshot("FOLDER:D:\\照片", "照片"));

    renderWizard();
    await user.click(await screen.findByRole("button", { name: "展开 D:\\" }));
    await user.click(await screen.findByRole("button", { name: "照片" }));

    expect(scanMock).toHaveBeenCalledWith("D:\\照片");
    expect(useImportStore.getState().devices.some((d) => d.id === "FOLDER:D:\\照片")).toBe(true);
    const info = await screen.findByTestId("wizard-device-info");
    expect(info).toHaveTextContent("本地文件夹");
    expect(info).toHaveTextContent("照片");
    expect(info).toHaveTextContent("10.0 MB");
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
    expect(scanMock).toHaveBeenCalledWith("D:\\老照片");
    const info = await screen.findByTestId("wizard-device-info");
    expect(info).toHaveTextContent("本地文件夹");
    // 选中源切到文件夹（下拉框当前值为 FOLDER: id）
    expect(screen.getByLabelText("选择设备")).toHaveValue("FOLDER:D:\\老照片");
  });

  it("folderScan 失败（IPC 不可用）时静默保持原源", async () => {
    seedSession();
    const user = userEvent.setup();
    openMock.mockResolvedValue("D:\\老照片");
    scanMock.mockResolvedValue(null);

    renderWizard("?device=E:");
    await user.click(await screen.findByTestId("wizard-fs-browse"));

    expect(scanMock).toHaveBeenCalledWith("D:\\老照片");
    expect(useImportStore.getState().devices).toHaveLength(2);
    const info = screen.getByTestId("wizard-device-info");
    expect(info).toHaveTextContent("读卡器");
  });
});

describe("最近使用", () => {
  it("手动选择设备后出现在最近使用，点击可重选并持久化 localStorage", async () => {
    seedSession();
    const user = userEvent.setup();
    renderWizard("?device=E:");

    await user.selectOptions(await screen.findByLabelText("选择设备"), "MTP:CAM");

    const recent = await screen.findByTestId("wizard-recent");
    expect(recent).toHaveTextContent("EOS R5");
    const stored = JSON.parse(
      localStorage.getItem("smartphoto.import.recentSources") ?? "[]",
    ) as Array<{ id: string }>;
    expect(stored[0]?.id).toBe("MTP:CAM");

    // 切回 E: 后，从最近使用重选相机
    await user.selectOptions(screen.getByLabelText("选择设备"), "E:");
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
    startMock.mockResolvedValueOnce(11);
    renderWizard("?device=E:");

    await user.click(await screen.findByRole("radio", { name: "移动 · 不保留" }));
    expect(screen.getByTestId("wizard-mode-move")).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText(/入库后删除源文件/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "开始导入" }));
    expect(await screen.findByTestId("tasks-probe")).toBeInTheDocument();
    expect(startMock.mock.calls[0][0].mode).toBe("move");
    expect(useImportStore.getState().jobModes[11]).toBe("move");
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

  it("volume 源 photo 走 asset 协议：src = convertFileSrc(设备id/relPath)；RAW/视频无 img", async () => {
    seedSession();
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderWizard("?device=E:");
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("wizard-view-grid"));
    const img = await screen.findByRole("img", { name: "IMG_0003.JPG" });
    expect(img).toHaveAttribute("src", "asset://E:/DCIM/101CANON/IMG_0003.JPG");
    expect(convertMock).toHaveBeenCalledWith("E:/DCIM/101CANON/IMG_0003.JPG");
    expect(screen.queryByRole("img", { name: "IMG_0001.CR3" })).not.toBeInTheDocument();
  });

  it("folder 源 absPath = id 去 FOLDER: 前缀 + / + relPath", async () => {
    useImportStore.setState({
      devices: [folderSnapshot()],
      sourceFiles: {
        "FOLDER:D:\\老照片": [
          { path: "DCIM/A.JPG", dir: "DCIM", name: "A.JPG", size: 10, kind: "photo" },
        ],
      },
    });
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderWizard();
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("wizard-view-grid"));
    expect(convertMock).toHaveBeenCalledWith("D:\\老照片/DCIM/A.JPG");
    expect(await screen.findByRole("img", { name: "A.JPG" })).toHaveAttribute(
      "src",
      "asset://D:\\老照片/DCIM/A.JPG",
    );
  });

  it("MTP 源无文件系统路径：photo 恒占位、不调 convertFileSrc", async () => {
    useImportStore.setState({
      devices: [mtpDevice()],
      sourceFiles: {
        "MTP:CAM": [{ path: "DCIM/B.JPG", dir: "DCIM", name: "B.JPG", size: 10, kind: "photo" }],
      },
    });
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderWizard("?device=MTP:CAM");
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("wizard-view-grid"));
    expect(convertMock).not.toHaveBeenCalled();
    const grid = await screen.findByTestId("wizard-file-grid");
    expect(within(grid).getByTestId("tile-photo")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("convertFileSrc 抛错时静默占位不崩溃", async () => {
    seedSession();
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
