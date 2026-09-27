import { beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import ImportDerivedDialog from "./ImportDerivedDialog";
import { resetThumbPipelineForTests } from "@/features/gallery/lib/thumbPipeline";
import { type LrExportCandidate } from "@/ipc/api";

/**
 * 「导入成片到相册」对话框（B4 子分组模型）：目录 → lr_export_scan 候选审核
 * （默认最高分/改选/待关联跳过/basis 徽标）→ 目标子分组（默认「成片」，datalist
 * 提示已有）→ lr_export_import(matches, albumId, subgroup) → 结果摘要 + 失败重试。
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    lrExportScan: vi.fn(),
    lrExportImport: vi.fn(),
    assetThumbGet: vi.fn(),
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn((p: string) => `asset://${p}`),
}));

import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  assetThumbGet,
  lrExportImport as importMockRaw,
  lrExportScan as scanMockRaw,
} from "@/ipc/api";

const scanMock = vi.mocked(scanMockRaw);
const importMock = vi.mocked(importMockRaw);
const dialogMock = vi.mocked(openDialog);
const thumbMock = vi.mocked(assetThumbGet);

const BASIS_FILENAME = JSON.stringify({ bases: ["filename"] });
const BASIS_TIME = JSON.stringify({ bases: ["exif_time"], time_delta_ms: 800 });

function row(path: string, size: number, candidates: LrExportCandidate["candidates"]): LrExportCandidate {
  return { path, size, candidates };
}

function makeRows(): LrExportCandidate[] {
  return [
    row("D:\\LR导出\\DSC_0001_edit_v1.jpg", 1024 * 1024, [
      { assetId: 11, name: "DSC_0001.NEF", score: 90, basis: BASIS_FILENAME },
      { assetId: 12, name: "DSC_0009.NEF", score: 40, basis: BASIS_TIME },
    ]),
    row("D:\\LR导出\\DSC_0002_edit_v1.jpg", 2 * 1024 * 1024, [
      { assetId: 13, name: "DSC_0002.NEF", score: 70, basis: BASIS_TIME },
    ]),
    row("D:\\LR导出\\unknown_edit.jpg", 512 * 1024, []),
  ];
}

function renderDialog(onImported = vi.fn(), onClose = vi.fn()) {
  render(
    <I18nextProvider i18n={i18n}>
      <ImportDerivedDialog
        albumId={7}
        albumName="婚礼"
        subgroups={["成片", "精选"]}
        onClose={onClose}
        onImported={onImported}
      />
    </I18nextProvider>,
  );
  return { onImported, onClose };
}

async function pickDirAndScan(user: ReturnType<typeof userEvent.setup>): Promise<void> {
  dialogMock.mockResolvedValue("D:\\LR导出");
  await user.click(screen.getByTestId("lr-pick-button"));
  await waitFor(() => expect(scanMock).toHaveBeenCalledWith("D:\\LR导出"));
  await screen.findAllByTestId("lr-row");
}

beforeEach(() => {
  scanMock.mockReset().mockResolvedValue(makeRows());
  importMock.mockReset().mockResolvedValue({ imported: 1, skipped: 0, failed: [] });
  dialogMock.mockReset().mockResolvedValue("D:\\LR导出");
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  resetThumbPipelineForTests();
});

describe("导入成片对话框：扫描与候选审核", () => {
  it("选目录 → lr_export_scan；行渲染（文件名/大小/候选）；默认选最高分；待关联计入统计", async () => {
    const user = userEvent.setup();
    renderDialog();

    await pickDirAndScan(user);
    const rows = screen.getAllByTestId("lr-row");
    expect(rows).toHaveLength(3);
    expect(within(rows[0]).getByTestId("lr-row-name")).toHaveTextContent("DSC_0001_edit_v1.jpg");
    expect(within(rows[0]).getByTestId("lr-row-size")).toHaveTextContent("1.0 MB");
    const best = within(rows[0]).getAllByTestId("lr-candidate").find((c) => c.getAttribute("data-asset-id") === "11");
    expect(best).toHaveAttribute("data-selected", "true");
    expect(within(rows[2]).getByTestId("lr-row-status")).toHaveTextContent("待关联");
    expect(screen.getByTestId("lr-stats-import")).toHaveTextContent("将导入 2 张");
    expect(screen.getByTestId("lr-stats-pending")).toHaveTextContent("待关联 1 张");
  });

  it("basis 徽标与改选/待关联：改选另一候选；标待关联后不入负载统计", async () => {
    const user = userEvent.setup();
    renderDialog();
    await pickDirAndScan(user);
    const rows = screen.getAllByTestId("lr-row");

    expect(within(rows[0]).getAllByTestId("lr-basis-badge")[0]).toHaveTextContent("文件名");
    expect(within(rows[1]).getByTestId("lr-basis-badge")).toHaveTextContent("拍摄时间");

    const low = within(rows[0]).getAllByTestId("lr-candidate").find((c) => c.getAttribute("data-asset-id") === "12");
    await user.click(low as HTMLElement);
    expect(
      within(rows[0]).getAllByTestId("lr-candidate").find((c) => c.getAttribute("data-asset-id") === "12"),
    ).toHaveAttribute("data-selected", "true");

    await user.click(within(rows[1]).getByTestId("lr-row-unmatched"));
    expect(rows[1]).toHaveAttribute("data-pending", "true");
    expect(screen.getByTestId("lr-stats-import")).toHaveTextContent("将导入 1 张");
    expect(screen.getByTestId("lr-stats-pending")).toHaveTextContent("待关联 2 张");
  });
});

describe("导入成片对话框：确认与结果摘要", () => {
  it("确认 → lr_export_import(matches, albumId=7, subgroup='成片')；basis 透传；待关联行排除", async () => {
    const user = userEvent.setup();
    renderDialog();
    await pickDirAndScan(user);

    // 目标子分组默认「成片」，可改
    expect(screen.getByTestId("lr-subgroup-input")).toHaveValue("成片");
    await user.clear(screen.getByTestId("lr-subgroup-input"));
    await user.type(screen.getByTestId("lr-subgroup-input"), "精修");

    await user.click(screen.getByTestId("lr-confirm"));
    await waitFor(() => expect(importMock).toHaveBeenCalledTimes(1));
    const [matches, albumId, subgroup] = importMock.mock.calls[0];
    expect(matches).toHaveLength(2);
    expect(matches[0]).toEqual({
      path: "D:\\LR导出\\DSC_0001_edit_v1.jpg",
      sourceAssetId: 11,
      basis: BASIS_FILENAME,
    });
    expect(albumId).toBe(7);
    expect(subgroup).toBe("精修");
  });

  it("结果摘要（imported/skipped/failed + 失败清单）→ 重试只重放失败子集；onImported 通知父层刷新", async () => {
    const user = userEvent.setup();
    const { onImported } = renderDialog();
    importMock
      .mockResolvedValueOnce({
        imported: 1,
        skipped: 0,
        failed: [{ path: "D:\\LR导出\\DSC_0002_edit_v1.jpg", error: "磁盘写入失败" }],
      })
      .mockResolvedValueOnce({ imported: 1, skipped: 0, failed: [] });
    await pickDirAndScan(user);
    await user.click(screen.getByTestId("lr-confirm"));

    const result = await screen.findByTestId("lr-result");
    expect(within(result).getByTestId("lr-result-imported")).toHaveTextContent("成功导入 1 张");
    expect(within(result).getByTestId("lr-result-failed")).toHaveTextContent("失败 1 张");
    expect(within(result).getByTestId("lr-result-failures")).toHaveTextContent("磁盘写入失败");
    await waitFor(() => expect(onImported).toHaveBeenCalledTimes(1));

    await user.click(screen.getByTestId("lr-result-retry"));
    await waitFor(() => expect(importMock).toHaveBeenCalledTimes(2));
    const [retryMatches] = importMock.mock.calls[1];
    expect(retryMatches.map((m) => m.path)).toEqual(["D:\\LR导出\\DSC_0002_edit_v1.jpg"]);
    await waitFor(() =>
      expect(within(screen.getByTestId("lr-result")).getByTestId("lr-result-failed")).toHaveTextContent("失败 0 张"),
    );
    // 重试成功不再重复通知
    expect(onImported).toHaveBeenCalledTimes(1);
  });

  it("全部待关联：确认禁用；命令失败（null）→ 通用错误提示", async () => {
    const user = userEvent.setup();
    scanMock.mockResolvedValue([row("D:\\LR导出\\x.jpg", 1, [])]);
    renderDialog();
    await pickDirAndScan(user);
    expect(screen.getByTestId("lr-confirm")).toBeDisabled();

    scanMock.mockResolvedValue(makeRows());
    importMock.mockResolvedValue(null);
    await user.click(screen.getByTestId("lr-rescan"));
    await screen.findAllByTestId("lr-row");
    await user.click(screen.getByTestId("lr-confirm"));
    expect(await screen.findByTestId("lr-import-error")).toHaveTextContent("导入失败");
  });
});
