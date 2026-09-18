import { beforeEach, describe, expect, it } from "vitest";

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";

import i18n from "@/i18n";
import DeviceDialog from "./DeviceDialog";
import { resetImportStoreForTests, useImportStore } from "@/stores/importStore";
import type { DeviceSnapshot } from "@/ipc/api";

function snapshot(id: string, name: string, kind: "volume" | "mtp", newFiles: number): DeviceSnapshot {
  return {
    id,
    name,
    kind,
    filesByKind: { photo: 100, raw: 40, video: 3, other: 1 },
    bytesTotal: 8_000_000_000,
    newFiles,
  };
}

function ImportProbe() {
  const location = useLocation();
  return <div data-testid="import-probe">{`${location.pathname}${location.search}`}</div>;
}

function renderDialog() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/"]}>
        <Routes>
          <Route path="/" element={<DeviceDialog />} />
          <Route path="/import" element={<ImportProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  resetImportStoreForTests();
});

describe("DeviceDialog", () => {
  it("队列为空时不渲染弹窗", () => {
    renderDialog();

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("展示设备名/类型徽标/分色统计/总量/新增数", () => {
    useImportStore.setState({
      devices: [snapshot("E:", "SanDisk 64G", "volume", 12)],
      promptQueue: ["E:"],
    });

    renderDialog();

    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText("SanDisk 64G")).toBeInTheDocument();
    expect(screen.getByText("读卡器")).toBeInTheDocument();
    // 分色统计：照片 100 / RAW 40 / 视频 3
    expect(screen.getByTestId("device-dialog-stats")).toHaveTextContent("100");
    expect(screen.getByTestId("device-dialog-stats")).toHaveTextContent("40");
    expect(screen.getByTestId("device-dialog-stats")).toHaveTextContent("3");
    // 总量 8 GB + 新增
    expect(screen.getByText("7.5 GB")).toBeInTheDocument();
    expect(screen.getByTestId("device-dialog-new")).toHaveTextContent("12 个新文件");
  });

  it("MTP 设备显示相机徽标", () => {
    useImportStore.setState({
      devices: [snapshot("MTP_CAM", "EOS R5", "mtp", 5)],
      promptQueue: ["MTP_CAM"],
    });

    renderDialog();

    expect(screen.getByText("相机")).toBeInTheDocument();
    expect(screen.getByText("EOS R5")).toBeInTheDocument();
  });

  it("开始导入：出队并携带 device 参数跳转导入向导", async () => {
    useImportStore.setState({
      devices: [snapshot("E:", "SanDisk 64G", "volume", 12)],
      promptQueue: ["E:"],
    });

    renderDialog();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "开始导入" }));

    expect(await screen.findByTestId("import-probe")).toHaveTextContent("/import?device=E%3A");
    expect(useImportStore.getState().promptQueue).toEqual([]);
  });

  it("忽略：关闭当前弹窗，队列中下一台接着弹出", async () => {
    useImportStore.setState({
      devices: [snapshot("E:", "SD 卡", "volume", 1), snapshot("F:", "CF 卡", "volume", 2)],
      promptQueue: ["E:", "F:"],
    });

    renderDialog();
    expect(screen.getByText("还有 1 台设备待处理")).toBeInTheDocument();

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "忽略" }));

    await waitFor(() => {
      expect(screen.getByText("CF 卡")).toBeInTheDocument();
    });
    expect(useImportStore.getState().promptQueue).toEqual(["F:"]);
  });
});
