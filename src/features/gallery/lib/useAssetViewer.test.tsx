import { describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, useLocation } from "react-router";

vi.mock("./viewMark", () => ({ markAssetViewed: vi.fn() }));
vi.mock("@/ipc/api", async (importOriginal) => ({
  ...await importOriginal<typeof import("@/ipc/api")>(),
  assetsByIds: vi.fn(),
}));

import { useAssetViewer } from "./useAssetViewer";
import type { AssetDto } from "@/ipc/api";
import { assetsByIds } from "@/ipc/api";
import type { AssetGroup } from "./assetGroups";

const asset = (id: number): AssetDto => ({ id, name: `${id}.jpg`, path: `${id}.jpg`, kind: "photo", capturedAt: null, camera: null, sizeBytes: 0 });
const groups: AssetGroup[] = [
  { key: "2026-09-27", date: "2026-09-27", assets: [asset(1), asset(2)] },
  { key: "2026-09-26", date: "2026-09-26", assets: [asset(3)] },
];

function Harness() {
  const { viewer, openAsset, navigateTo } = useAssetViewer(groups);
  return <>
    <button onClick={() => openAsset(groups[0].assets[1])}>open</button>
    <button onClick={() => navigateTo((viewer?.index ?? 0) + 1)}>next</button>
    <span data-testid="viewer-position">{viewer ? `${viewer.asset.id}:${viewer.index}:${viewer.group.assets.length}` : "closed"}</span>
  </>;
}

const pairJpg = { ...asset(1), pairId: 1 };
const pairRaw = { ...asset(2), name: "1.NEF", kind: "raw" as const, pairId: 1 };
function PairHarness({ loaded = true }: { loaded?: boolean }) {
  const { viewer, openAsset, navigateTo, selectVersion } = useAssetViewer(
    [{ key: "pair", date: null, assets: [pairJpg, asset(3)] }],
    loaded ? [pairJpg, pairRaw, asset(3)] : [],
  );
  return <>
    <button onClick={() => openAsset(pairJpg)}>open</button>
    <button onClick={() => void selectVersion(2)}>raw</button>
    <button onClick={() => void selectVersion(1)}>jpg</button>
    <button onClick={() => navigateTo((viewer?.index ?? 0) + 1)}>next</button>
    <span data-testid="viewer-position">{viewer ? `${viewer.asset.id}:${viewer.index}:${viewer.group.assets.length}` : "closed"}</span>
  </>;
}

describe("useAssetViewer", () => {
  it("未知日期预览打开、翻页和关闭都保留语义查询及格式参数",()=>{
    function SemanticHarness(){
      const state=useAssetViewer([{key:"unknown",date:null,assets:[asset(99),asset(100)]}]);
      const location=useLocation();
      return <>
        <button onClick={()=>state.openAsset(asset(99))}>open</button>
        <button onClick={()=>state.navigateTo(1)}>next</button>
        <button onClick={state.closeViewer}>close</button>
        <span data-testid="url">{location.search}</span>
        <span data-testid="active">{state.viewer?.asset.id??"closed"}</span>
      </>;
    }
    render(<MemoryRouter initialEntries={["/gallery?mode=semantic&q=cat&format=NEF"]}><SemanticHarness/></MemoryRouter>);
    fireEvent.click(screen.getByText("open"));
    expect(screen.getByTestId("active")).toHaveTextContent("99");
    fireEvent.click(screen.getByText("next"));
    expect(screen.getByTestId("active")).toHaveTextContent("100");
    fireEvent.click(screen.getByText("close"));
    const params=new URLSearchParams(screen.getByTestId("url").textContent!);
    expect(params.get("asset")).toBeNull();
    expect(params.get("mode")).toBe("semantic");
    expect(params.get("q")).toBe("cat");
    expect(params.get("format")).toBe("NEF");
  });
  it("RAW 已打开后被分页合并到 JPG 卡，预览仍保留在同一组", () => {
    render(<MemoryRouter initialEntries={["/?asset=2"]}><PairHarness /></MemoryRouter>);
    expect(screen.getByTestId("viewer-position")).toHaveTextContent("2:0:2");
    fireEvent.click(screen.getByText("next"));
    expect(screen.getByTestId("viewer-position")).toHaveTextContent("3:1:2");
  });
  it("RAW/JPG 切换保持同一照片位置和胶片条数量，下一张跳过伙伴文件", async () => {
    render(<MemoryRouter><PairHarness /></MemoryRouter>);
    fireEvent.click(screen.getByText("open"));
    fireEvent.click(screen.getByText("raw"));
    await waitFor(() => expect(screen.getByTestId("viewer-position")).toHaveTextContent("2:0:2"));
    fireEvent.click(screen.getByText("jpg"));
    await waitFor(() => expect(screen.getByTestId("viewer-position")).toHaveTextContent("1:0:2"));
    fireEvent.click(screen.getByText("raw"));
    fireEvent.click(screen.getByText("next"));
    expect(screen.getByTestId("viewer-position")).toHaveTextContent("3:1:2");
  });

  it("跨页取回版本时不扩大列表，迟到的请求不能覆盖已翻到的照片", async () => {
    let resolve!: (assets: AssetDto[]) => void;
    vi.mocked(assetsByIds).mockReturnValue(new Promise((done) => { resolve = done; }));
    render(<MemoryRouter><PairHarness loaded={false} /></MemoryRouter>);
    fireEvent.click(screen.getByText("open"));
    fireEvent.click(screen.getByText("raw"));
    expect(assetsByIds).toHaveBeenCalledWith([2]);
    fireEvent.click(screen.getByText("next"));
    await act(async () => { resolve([pairRaw]); });
    expect(screen.getByTestId("viewer-position")).toHaveTextContent("3:1:2");
  });
  it("navigates across date groups in the current result set", () => {
    render(<MemoryRouter><Harness /></MemoryRouter>);
    fireEvent.click(screen.getByRole("button", { name: "open" }));
    expect(screen.getByTestId("viewer-position")).toHaveTextContent("2:1:3");
    fireEvent.click(screen.getByRole("button", { name: "next" }));
    expect(screen.getByTestId("viewer-position")).toHaveTextContent("3:2:3");
  });
});
