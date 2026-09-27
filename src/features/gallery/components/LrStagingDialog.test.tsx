import { beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import LrStagingDialog from "./LrStagingDialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { lrStagingCreate, revealInExplorer, type AssetDto } from "@/ipc/api";

/**
 * 生成 LR 暂存夹弹窗（B1 追加包）：默认名 `MMDD-选中数`；lr_staging_create；
 * 结果面板 = 目录路径 + 「硬链接 N 张 / 复制 M 张」 + 打开文件夹（reveal_in_explorer，
 * 失败回退逐个 opener）；失败行内提示；Esc/取消关闭。
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    lrStagingCreate: vi.fn(),
    revealInExplorer: vi.fn(),
  };
});

vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: vi.fn(),
}));

const stagingMock = vi.mocked(lrStagingCreate);
const revealBatchMock = vi.mocked(revealInExplorer);
const revealMock = vi.mocked(revealItemInDir);

function makeAsset(id: number): AssetDto {
  return {
    id,
    path: `Y:\\照片\\IMG_${id}.JPG`,
    name: `IMG_${id}.JPG`,
    kind: "photo",
    capturedAt: "2026-09-18T10:00:00",
    camera: null,
    sizeBytes: 1,
  };
}

/** 本地今天 "MMDD"（与组件同口径） */
function expectedPrefix(): string {
  const now = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(now.getMonth() + 1)}${pad(now.getDate())}`;
}

function setup(assets: AssetDto[]) {
  const onClose = vi.fn();
  render(
    <I18nextProvider i18n={i18n}>
      <LrStagingDialog assets={assets} onClose={onClose} />
    </I18nextProvider>,
  );
  return { onClose };
}

beforeEach(() => {
  stagingMock.mockReset();
  revealBatchMock.mockReset().mockResolvedValue(1);
  revealMock.mockReset().mockResolvedValue(undefined);
});

describe("生成 LR 暂存夹弹窗", () => {
  it("默认名 = MMDD-选中数；确认调 lr_staging_create(assetIds, name)", async () => {
    const user = userEvent.setup();
    stagingMock.mockResolvedValue({ dir: "D:\LR\stage", created: 3, hardlinked: 3, copied: 0 });
    setup([makeAsset(1), makeAsset(2), makeAsset(3)]);

    const input = screen.getByTestId("lr-staging-name");
    expect(input).toHaveValue(`${expectedPrefix()}-3`);

    await user.clear(input);
    await user.type(input, "婚礼0927");
    await user.click(screen.getByTestId("lr-staging-confirm"));

    await waitFor(() => expect(stagingMock).toHaveBeenCalledWith([1, 2, 3], "婚礼0927"));
  });

  it("生成失败（命令不可用）→ 行内错误提示，弹窗不关", async () => {
    const user = userEvent.setup();
    stagingMock.mockResolvedValue(null);
    setup([makeAsset(1)]);

    await user.click(screen.getByTestId("lr-staging-confirm"));
    expect(await screen.findByTestId("lr-staging-error")).toHaveTextContent("生成失败");
    expect(screen.getByTestId("lr-staging-dialog")).toBeInTheDocument();
  });

  it("成功：结果面板显示目录路径 + 硬链接/复制计数；「打开文件夹」走 reveal_in_explorer；完成关闭", async () => {
    const user = userEvent.setup();
    const { onClose } = { onClose: setup([makeAsset(1), makeAsset(2)]).onClose };
    stagingMock.mockResolvedValue({ dir: "I:\\SmartPhoto\\主库\\LR\\0927-2", created: 2, hardlinked: 2, copied: 0 });

    await user.click(screen.getByTestId("lr-staging-confirm"));

    const result = await screen.findByTestId("lr-staging-result");
    expect(within(result).getByTestId("lr-staging-path")).toHaveTextContent("LR\\0927-2");
    expect(within(result).getByTestId("lr-staging-counts")).toHaveTextContent("硬链接 2 张 / 复制 0 张");

    await user.click(screen.getByTestId("lr-staging-open"));
    await waitFor(() => expect(revealBatchMock).toHaveBeenCalledWith(["I:\\SmartPhoto\\主库\\LR\\0927-2"]));

    await user.click(screen.getByTestId("lr-staging-done"));
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("跨盘回退复制也能打开；打开失败回退逐个 opener", async () => {
    const user = userEvent.setup();
    setup([makeAsset(1)]);
    stagingMock.mockResolvedValue({ dir: "D:\\LR\\stage", created: 1, hardlinked: 0, copied: 1 });
    revealBatchMock.mockRejectedValue(new Error("batch failed"));

    await user.click(screen.getByTestId("lr-staging-confirm"));
    await user.click(await screen.findByTestId("lr-staging-open"));
    await waitFor(() => expect(revealMock).toHaveBeenCalledWith("D:\\LR\\stage"));
  });
});
