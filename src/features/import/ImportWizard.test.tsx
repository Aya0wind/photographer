import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import ImportWizard from "./ImportWizard";
import { resetImportStoreForTests, useImportStore, type SourceFile } from "@/stores/importStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { importStart, type ImportPlan } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    importStart: vi.fn(),
  };
});

const startMock = vi.mocked(importStart);

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

beforeEach(() => {
  resetImportStoreForTests();
  startMock.mockReset().mockResolvedValue(null);
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
});

describe("源文件树与文件表", () => {
  it("按目录分组折叠展示；表头统计默认全选", async () => {
    seedSession();
    renderWizard("?device=E:");

    const tree = await screen.findByTestId("wizard-tree");
    expect(tree).toHaveTextContent("DCIM/100CANON");
    expect(tree).toHaveTextContent("DCIM/101CANON");
    expect(await screen.findByTestId("wizard-table-stats")).toHaveTextContent("已选 4 / 4");
    // 中栏表格：等宽文件名与类型徽标
    expect(screen.getByText("IMG_0001.CR3")).toBeInTheDocument();
    expect(screen.getByText("VID_0004.MP4")).toBeInTheDocument();
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

  it("折叠目录组隐藏组内文件", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    await screen.findByText("IMG_0001.CR3");
    // 点击组头折叠（按钮包含目录名）
    await user.click(screen.getByRole("button", { name: /DCIM\/100CANON/ }));
    expect(screen.queryByText("IMG_0001.CR3")).not.toBeInTheDocument();
    expect(screen.getByText("IMG_0003.JPG")).toBeInTheDocument();
  });
});

describe("方案面板", () => {
  it("目标根目录默认取激活库 photoRoot；切换预设实时预览", async () => {
    seedSession();
    renderWizard("?device=E:");
    const user = userEvent.setup();

    const rootInput = await screen.findByLabelText("目标根目录");
    expect(rootInput).toHaveValue("Y:\\照片");

    const preview = screen.getByTestId("wizard-preview");
    // 默认预设来自设置 {YYYY}/{MM-DD}/{原文件名} → 年/日期
    expect(preview).toHaveTextContent("Y:\\照片\\2026\\09-18\\IMG_0001.CR3");

    await user.selectOptions(screen.getByLabelText("目录模板"), "ym");
    expect(screen.getByTestId("wizard-preview")).toHaveTextContent("Y:\\照片\\2026\\09\\IMG_0001.CR3");

    await user.selectOptions(screen.getByLabelText("目录模板"), "orig");
    expect(screen.getByTestId("wizard-preview")).toHaveTextContent(
      "Y:\\照片\\DCIM\\100CANON\\IMG_0001.CR3",
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
      targetRoot: "Y:\\照片",
      dirTemplate: "{YYYY}/{MM-DD}",
      nameTemplate: "{原文件名}",
      duplicatePolicy: "skip",
      skipImported: true,
      streams: 4,
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
