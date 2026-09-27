import { beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import AddToAlbumDialog from "./AddToAlbumDialog";
import {
  albumAddAssets,
  albumClaimAssets,
  albumCreate,
  albumList,
  type AssetDto,
} from "@/ipc/api";
import { DEFAULT_SETTINGS, clone, useSettingsStore, type Library } from "@/stores/settingsStore";

/**
 * 「加入相册」选择弹窗（③ 全局入口）：已有相册单选 + 底部新建内联输入；
 * 确定后 album_add_assets，成功立即关闭；失败留在弹窗中提示。
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    albumList: vi.fn(),
    albumCreate: vi.fn(),
    albumAddAssets: vi.fn(),
    albumClaimAssets: vi.fn(),
  };
});

const albumListMock = vi.mocked(albumList);
const albumCreateMock = vi.mocked(albumCreate);
const addMock = vi.mocked(albumAddAssets);
const claimMock = vi.mocked(albumClaimAssets);

/** 带活动库（photoRoot=Y:\照片、收纳区 SmartPhoto）的库状态（claim 启发式基准） */
const LIB: Library = {
  id: "lib1",
  name: "主库",
  dbDir: "I:\\SmartPhoto\\主库",
  photoRoot: "Y:\\照片",
  dirTemplate: "{YYYY}/{MM-DD}/{原文件名}",
  importSubdir: "SmartPhoto",
  configured: true,
  streams: 4,
};

function seedLibrary(): void {
  useSettingsStore.setState({
    settings: { ...clone(DEFAULT_SETTINGS), activeLibraryId: "lib1", libraries: [LIB] },
    loaded: true,
    libraryChosen: true,
  });
}

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

function renderDialog(assets: AssetDto[]) {
  return render(
    <I18nextProvider i18n={i18n}>
      <AddToAlbumDialog assets={assets} onClose={() => {}} />
    </I18nextProvider>,
  );
}

/** 点选相册并确定（等待相册清单加载完成） */
async function pickAlbumAndConfirm(user: ReturnType<typeof userEvent.setup>, albumId: string) {
  const options = await screen.findAllByTestId("add-to-album-option");
  const option = options.find((o) => o.getAttribute("data-album-id") === albumId);
  if (!option) throw new Error(`album option ${albumId} not rendered`);
  await user.click(option);
  await user.click(screen.getByTestId("add-to-album-confirm"));
}

beforeEach(() => {
  vi.clearAllMocks();
  // 还原无活动库的默认态（非 claim 用例不受启发式影响；claim 用例自行 seed）
  useSettingsStore.setState({
    settings: clone(DEFAULT_SETTINGS),
    loaded: true,
    libraryChosen: false,
  });
  albumListMock.mockResolvedValue([
    { id: 3, name: "青海湖 2026", coverAssetId: null, itemCount: 12, createdAt: "2026-09-01" },
    { id: 4, name: "街拍", coverAssetId: null, itemCount: 5, createdAt: "2026-09-02" },
  ]);
  albumCreateMock.mockResolvedValue({
    ok: true,
    album: { id: 9, name: "新相册", coverAssetId: null, itemCount: 0, createdAt: "2026-09-03" },
  });
  addMock.mockResolvedValue(1);
});

describe("加入相册弹窗", () => {
  it("已有相册列表单选 + 张数徽标；确定调 album_add_assets", async () => {
    const user = userEvent.setup();
    renderDialog([makeAsset(1), makeAsset(2)]);

    const options = await screen.findAllByTestId("add-to-album-option");
    expect(options.map((o) => o.getAttribute("data-album-id"))).toEqual(["3", "4"]);
    expect(options[0]).toHaveTextContent("青海湖 2026");
    expect(options[0]).toHaveTextContent("12 张");

    await pickAlbumAndConfirm(user, "3");
    await waitFor(() => expect(addMock).toHaveBeenCalledWith(3, [1, 2]));
  });

  it("album_add_assets 失败（null）→ 加入失败文案", async () => {
    const user = userEvent.setup();
    addMock.mockResolvedValueOnce(null);
    renderDialog([makeAsset(1)]);

    await pickAlbumAndConfirm(user, "3");
    expect(await screen.findByTestId("add-to-album-error")).toHaveTextContent("加入失败");
  });

  it("底部新建相册：重名错误行内提示；创建成功自动选中并可确定", async () => {
    const user = userEvent.setup();
    albumCreateMock
      .mockResolvedValueOnce({ ok: false, error: "同名相册已存在" })
      .mockResolvedValueOnce({
        ok: true,
        album: { id: 9, name: "新相册", coverAssetId: null, itemCount: 0, createdAt: "2026-09-03" },
      });
    addMock.mockResolvedValue(1);
    renderDialog([makeAsset(7)]);

    // 重名：错误行内提示
    await user.type(await screen.findByTestId("add-to-album-new-name"), "新相册");
    await user.click(screen.getByTestId("add-to-album-new-create"));
    expect(await screen.findByTestId("add-to-album-new-error")).toHaveTextContent("同名相册已存在");

    // 成功：列表插入新相册并自动选中
    await user.clear(screen.getByTestId("add-to-album-new-name"));
    await user.type(screen.getByTestId("add-to-album-new-name"), "新相册");
    await user.click(screen.getByTestId("add-to-album-new-create"));
    await waitFor(() =>
      expect(
        screen
          .getAllByTestId("add-to-album-option")
          .some((o) => o.getAttribute("data-album-id") === "9" && o.getAttribute("data-selected") === "true"),
      ).toBe(true),
    );

    // 确定加入新建相册
    await user.click(screen.getByTestId("add-to-album-confirm"));
    await waitFor(() => expect(addMock).toHaveBeenCalledWith(9, [7]));
  });

  it("无相册（后端不可用）→ 空态文案，仍可新建", async () => {
    albumListMock.mockResolvedValue([]);
    const user = userEvent.setup();
    renderDialog([makeAsset(1)]);

    expect(await screen.findByTestId("add-to-album-empty")).toHaveTextContent("还没有相册");
    await user.type(screen.getByTestId("add-to-album-new-name"), "第一本");
    await user.click(screen.getByTestId("add-to-album-new-create"));
    await waitFor(() => expect(albumCreateMock).toHaveBeenCalledWith("第一本"));
  });

  it("加入成功：结果 toast（已加入（引用））展示后自动调 onClose（1.6s）", async () => {
    const onClose = vi.fn();
    addMock.mockResolvedValue(1);
    const user = userEvent.setup();
    render(
      <I18nextProvider i18n={i18n}>
        <AddToAlbumDialog assets={[makeAsset(1)]} onClose={onClose} />
      </I18nextProvider>,
    );

    await pickAlbumAndConfirm(user, "3");
    const toast = await screen.findByTestId("add-to-album-toast");
    expect(toast).toHaveTextContent("已加入（引用）1 张");
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1), { timeout: 3000 });
  });
});

// --- 归入 vs 引用语义分层（B1 追加包：相册物理目录化） -----------------------------------

describe("加入相册弹窗：归入（claim）分层", () => {
  it("无活动库（photoRoot 未知）：不显示归入按钮，维持「加入（引用）」", async () => {
    const user = userEvent.setup();
    renderDialog([makeAsset(1)]);
    const options = await screen.findAllByTestId("add-to-album-option");
    expect(screen.queryByTestId("add-to-album-claim")).not.toBeInTheDocument();
    expect(screen.queryByTestId("add-to-album-mode-hint")).not.toBeInTheDocument();
    await user.click(options[0]);
    await user.click(screen.getByTestId("add-to-album-confirm"));
    await waitFor(() => expect(addMock).toHaveBeenCalledWith(3, [1]));
  });

  it("全部未归册：默认动作突出「归入相册」→ album_claim_assets；toast「已归入（文件已移动）」", async () => {
    seedLibrary();
    claimMock.mockResolvedValue(2);
    const onClose = vi.fn();
    const user = userEvent.setup();
    const unclaimed = (id: number): AssetDto => ({
      ...makeAsset(id),
      path: `Y:\\照片\\SmartPhoto\\2026\\09-18\\IMG_${id}.JPG`,
    });
    render(
      <I18nextProvider i18n={i18n}>
        <AddToAlbumDialog assets={[unclaimed(1), unclaimed(2)]} onClose={onClose} />
      </I18nextProvider>,
    );

    const options = await screen.findAllByTestId("add-to-album-option");
    await user.click(options[0]);
    expect(screen.getByTestId("add-to-album-mode-hint")).toHaveTextContent("均在日期目录下未归册");
    const claimBtn = screen.getByTestId("add-to-album-claim");
    expect(claimBtn).toHaveTextContent("归入相册（移动文件）");

    await user.click(claimBtn);
    await waitFor(() => expect(claimMock).toHaveBeenCalledWith(3, [1, 2]));
    const toast = await screen.findByTestId("add-to-album-toast");
    expect(toast).toHaveTextContent("已归入（文件已移动）2 张");
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1), { timeout: 3000 });
  });

  it("混合选择：归入只作用于未归册子集；加入（引用）作用于全部；说明文案双行", async () => {
    seedLibrary();
    const user = userEvent.setup();
    const claimed = { ...makeAsset(1), path: "Y:\\照片\\SmartPhoto\\青海湖 2026\\2026\\09-18\\IMG_1.JPG" };
    const unclaimed = { ...makeAsset(2), path: "Y:\\照片\\SmartPhoto\\2026\\09-18\\IMG_2.JPG" };
    renderDialog([claimed, unclaimed]);

    const options = await screen.findAllByTestId("add-to-album-option");
    await user.click(options[0]);
    expect(screen.getByTestId("add-to-album-mode-hint")).toHaveTextContent("1 张未归册");
    expect(screen.getByTestId("add-to-album-claim")).toHaveTextContent("归入相册（移动 1 张未归册）");
    expect(screen.getByTestId("add-to-album-confirm")).toHaveTextContent("加入（引用）");

    await user.click(screen.getByTestId("add-to-album-claim"));
    await waitFor(() => expect(claimMock).toHaveBeenCalledWith(3, [2]));
  });

  it("归入失败（已在别册主目录）：透传后端 Err + 改用引用提示，弹窗不关", async () => {
    seedLibrary();
    claimMock.mockRejectedValue(new Error("已在其他相册主目录"));
    const onClose = vi.fn();
    const user = userEvent.setup();
    const unclaimed = { ...makeAsset(1), path: "Y:\\照片\\SmartPhoto\\2026\\09-18\\IMG_1.JPG" };
    render(
      <I18nextProvider i18n={i18n}>
        <AddToAlbumDialog assets={[unclaimed]} onClose={onClose} />
      </I18nextProvider>,
    );

    await screen.findAllByTestId("add-to-album-option");
    await user.click(screen.getAllByTestId("add-to-album-option")[0]);
    await user.click(screen.getByTestId("add-to-album-claim"));

    const error = await screen.findByTestId("add-to-album-claim-error");
    expect(error).toHaveTextContent("已在其他相册主目录");
    expect(error).toHaveTextContent("可改用「加入（引用）」");
    expect(screen.getByTestId("add-to-album-dialog")).toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
  });
});
