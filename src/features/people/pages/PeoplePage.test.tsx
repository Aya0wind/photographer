import { assetFixture } from "@/test/fixtures";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import PeoplePage from "./PeoplePage";
import { resetThumbPipelineForTests } from "@/features/gallery/lib/thumbPipeline";
import {
  assetThumbGet,
  peopleAssets,
  peopleList,
  personDelete,
  personRename,
  type AssetDto,
  type PersonCluster,
} from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    peopleList: vi.fn(),
    peopleAssets: vi.fn(),
    personRename: vi.fn(),
    personDelete: vi.fn(),
    assetThumbGet: vi.fn(),
  };
});
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

import { convertFileSrc } from "@tauri-apps/api/core";

const peopleListMock = vi.mocked(peopleList);
const peopleAssetsMock = vi.mocked(peopleAssets);
const personRenameMock = vi.mocked(personRename);
const personDeleteMock = vi.mocked(personDelete);
const thumbMock = vi.mocked(assetThumbGet);
const convertMock = vi.mocked(convertFileSrc);

const PEOPLE: PersonCluster[] = [
  { clusterId: 0, name: "张三", faceCount: 12, coverAssetId: 101 },
  { clusterId: 2, name: null, faceCount: 5, coverAssetId: 102 },
  { clusterId: 3, name: "李四", faceCount: 1, coverAssetId: 103 },
];

function makeAsset(id: number): AssetDto {
  return assetFixture(id, {
    capturedAt: "2026-09-01T10:00:00",
  });
}

function renderPeople() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/people"]}>
        <Routes>
          <Route path="/people" element={<PeoplePage />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeAll(() => {
  // jsdom 无布局：虚拟网格视口为空会一行都不渲染
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 800 });
});

beforeEach(() => {
  peopleListMock.mockReset().mockResolvedValue([]);
  peopleAssetsMock.mockReset().mockResolvedValue([]);
  personRenameMock.mockReset().mockResolvedValue(true);
  personDeleteMock.mockReset().mockResolvedValue(true);
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  convertMock.mockReset().mockReturnValue("");
  resetThumbPipelineForTests();
});

// --- 列表渲染 -----------------------------------------------------------------------

describe("人物页列表", () => {
  it("进页面拉取 peopleList；渲染封面/姓名或「人物 N」/faceCount 徽标/总数", async () => {
    peopleListMock.mockResolvedValue(PEOPLE);
    thumbMock.mockResolvedValue({ status: "ready", path: "D:\\cache\\101.jpg" });
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderPeople();

    const cards = await screen.findAllByTestId("people-card");
    expect(cards).toHaveLength(3);
    expect(peopleListMock).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("people-count")).toHaveTextContent("3 位人物");

    // 姓名：命名优先；未命名 = 「人物 {clusterId+1}」
    expect(within(cards[0]).getByTestId("people-card-name")).toHaveTextContent("张三");
    expect(within(cards[1]).getByTestId("people-card-name")).toHaveTextContent("人物 3");
    expect(within(cards[2]).getByTestId("people-card-name")).toHaveTextContent("李四");

    // faceCount 徽标
    expect(within(cards[0]).getByTestId("people-card-count")).toHaveTextContent("12 张");
    expect(within(cards[1]).getByTestId("people-card-count")).toHaveTextContent("5 张");

    // 封面 = coverAssetId 走 asset_thumb_get 管线
    await waitFor(() => expect(thumbMock).toHaveBeenCalledWith(101, 240));
    const coverImg = await within(cards[0]).findByTestId("people-card-cover-img");
    expect(coverImg).toHaveAttribute("src", "asset://D:\\cache\\101.jpg");
  });

  it("封面缩略图缺失 → 占位（不报错）", async () => {
    peopleListMock.mockResolvedValue(PEOPLE);
    renderPeople();

    const cards = await screen.findAllByTestId("people-card");
    await waitFor(() => expect(thumbMock).toHaveBeenCalledWith(103, 240));
    expect(within(cards[2]).queryByTestId("people-card-cover-img")).not.toBeInTheDocument();
  });
});

// --- 空态兜底 -----------------------------------------------------------------------

describe("人物页空态", () => {
  it("后端未就绪（空清单）→ 说明 + 占位网格", async () => {
    renderPeople();

    expect(await screen.findByTestId("people-placeholder-grid")).toBeInTheDocument();
    expect(screen.getAllByTestId("people-placeholder-card").length).toBeGreaterThanOrEqual(6);
    expect(screen.getByTestId("people-page")).toHaveTextContent("人脸聚类将在索引完成后自动生成");
    expect(screen.queryByTestId("people-card")).not.toBeInTheDocument();
  });

  it("peopleList 拒绝（传输异常）同样回空态", async () => {
    peopleListMock.mockRejectedValue("ipc dead");
    renderPeople();

    expect(await screen.findByTestId("people-placeholder-grid")).toBeInTheDocument();
  });
});

// --- 单人物照片视图 -------------------------------------------------------------------

describe("人物照片视图", () => {
  it("点击卡片 → peopleAssets(clusterId, 500) + AssetGrid；打开查看器；返回列表", async () => {
    const user = userEvent.setup();
    peopleListMock.mockResolvedValue(PEOPLE);
    peopleAssetsMock.mockResolvedValue([makeAsset(1), makeAsset(2)]);
    renderPeople();

    const cards = await screen.findAllByTestId("people-card");
    await user.click(within(cards[0]).getByTestId("people-card-name"));

    expect(await screen.findByTestId("people-assets-view")).toBeInTheDocument();
    expect(peopleAssetsMock).toHaveBeenCalledWith(0, 500);
    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles).toHaveLength(2);
    expect(screen.getByTestId("people-assets-title")).toHaveTextContent("张三");
    expect(screen.getByTestId("people-assets-count")).toHaveTextContent("12 张");

    // 复用查看器（?asset= searchParams）
    await user.click(tiles[0]);
    expect(await screen.findByTestId("viewer")).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("viewer")).not.toBeInTheDocument());

    // 返回人物列表
    await user.click(screen.getByTestId("people-back"));
    expect((await screen.findAllByTestId("people-card")).length).toBeGreaterThanOrEqual(1);
  });

  it("该人物暂无照片 → 空态文案", async () => {
    const user = userEvent.setup();
    peopleListMock.mockResolvedValue(PEOPLE);
    peopleAssetsMock.mockResolvedValue([]);
    renderPeople();

    const cards = await screen.findAllByTestId("people-card");
    await user.click(within(cards[2]).getByTestId("people-card-name"));

    expect(await screen.findByTestId("people-assets-empty")).toHaveTextContent("该人物暂无照片");
  });
});

// --- 重命名 -------------------------------------------------------------------------

describe("人物重命名", () => {
  it("inline 编辑：Enter 提交 personRename(clusterId, 新名) → 成功后重拉清单", async () => {
    const user = userEvent.setup();
    peopleListMock.mockResolvedValue(PEOPLE);
    renderPeople();
    const cards = await screen.findAllByTestId("people-card");

    await user.click(within(cards[0]).getByTestId("people-card-rename"));
    const input = screen.getByTestId("people-rename-input");
    expect(input).toHaveValue("张三");
    await user.clear(input);
    await user.type(input, "老王{enter}");

    await waitFor(() => expect(personRenameMock).toHaveBeenCalledWith(0, "老王"));
    await waitFor(() => expect(peopleListMock).toHaveBeenCalledTimes(2));
    expect(screen.queryByTestId("people-rename-input")).not.toBeInTheDocument();
  });

  it("Escape 取消：不调用 personRename", async () => {
    const user = userEvent.setup();
    peopleListMock.mockResolvedValue(PEOPLE);
    renderPeople();
    const cards = await screen.findAllByTestId("people-card");

    await user.click(within(cards[0]).getByTestId("people-card-rename"));
    await user.type(screen.getByTestId("people-rename-input"), "{escape}");

    expect(personRenameMock).not.toHaveBeenCalled();
    expect(screen.queryByTestId("people-rename-input")).not.toBeInTheDocument();
  });

  it("清空名字提交 = 取消（不调用）", async () => {
    const user = userEvent.setup();
    peopleListMock.mockResolvedValue(PEOPLE);
    renderPeople();
    const cards = await screen.findAllByTestId("people-card");

    await user.click(within(cards[0]).getByTestId("people-card-rename"));
    const input = screen.getByTestId("people-rename-input");
    await user.clear(input);
    await user.type(input, "{enter}");

    expect(personRenameMock).not.toHaveBeenCalled();
  });
});

// --- 删除（红色两步确认） -------------------------------------------------------------

describe("人物删除", () => {
  it("两步确认：取消不动；确认调 personDelete(cluster_id) 并重拉清单", async () => {
    const user = userEvent.setup();
    peopleListMock.mockResolvedValue(PEOPLE);
    renderPeople();
    const cards = await screen.findAllByTestId("people-card");

    await user.click(within(cards[1]).getByTestId("people-card-delete"));
    const confirm = screen.getByTestId("people-delete-confirm");
    // 红色确认钮
    expect(within(confirm).getByTestId("people-delete-confirm-yes").className).toContain("red");
    expect(confirm).toHaveTextContent("照片不受影响");

    // 取消路径
    await user.click(within(confirm).getByTestId("people-delete-confirm-no"));
    expect(screen.queryByTestId("people-delete-confirm")).not.toBeInTheDocument();
    expect(personDeleteMock).not.toHaveBeenCalled();

    // 确认路径
    await user.click(within(cards[1]).getByTestId("people-card-delete"));
    await user.click(screen.getByTestId("people-delete-confirm-yes"));
    await waitFor(() => expect(personDeleteMock).toHaveBeenCalledWith(2));
    await waitFor(() => expect(peopleListMock).toHaveBeenCalledTimes(2));
  });

  it("确认浮层出现期间卡片其余操作被覆盖（覆盖层渲染）", async () => {
    const user = userEvent.setup();
    peopleListMock.mockResolvedValue(PEOPLE);
    renderPeople();
    const cards = await screen.findAllByTestId("people-card");

    await user.click(within(cards[0]).getByTestId("people-card-delete"));
    expect(screen.getByTestId("people-delete-confirm")).toBeInTheDocument();
    // 重命名按钮仍在文档中（被覆盖层视觉遮挡），但浮层可关闭
    await user.click(screen.getByTestId("people-delete-confirm-no"));
    expect(screen.queryByTestId("people-delete-confirm")).not.toBeInTheDocument();
  });
});
