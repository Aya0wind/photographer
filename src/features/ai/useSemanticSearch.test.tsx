import { act, renderHook } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
vi.mock("@/ipc/api", () => ({ assetsByIds: vi.fn(), searchSemantic: vi.fn(), isIpcAvailable: vi.fn(() => true), aiModelsStatus: vi.fn(async () => []), indexStatus: vi.fn(), subscribeAppEvents: vi.fn() }));
import { assetsByIds, searchSemantic } from "@/ipc/api";
import { useSemanticSearch } from "./useSemanticSearch";
import { assetFixture } from "@/test/fixtures";
import { useAiStore } from "@/stores/aiStore";
beforeEach(() => { vi.mocked(searchSemantic).mockReset(); vi.mocked(assetsByIds).mockReset(); useAiStore.getState().resetForTests(); });
it("数据库和推理错误不误报模型未下载", async () => {
  vi.mocked(searchSemantic).mockRejectedValue("向量检索失败：索引文件不可访问");
  const { result } = renderHook(useSemanticSearch);
  await act(() => result.current.run("猫"));
  expect(result.current.status).toBe("error");
  expect(result.current.errorMessage).toContain("索引文件不可访问");
});
it("只有明确缺少模型才显示下载提示", async () => {
  vi.mocked(searchSemantic).mockRejectedValue("语义检索模型未下载（siglip2-text）");
  const { result } = renderHook(useSemanticSearch);
  await act(() => result.current.run("猫"));
  expect(result.current.status).toBe("modelNotReady");
});
it("结果详情读取失败不会一直停在搜索中", async () => {
  vi.mocked(searchSemantic).mockResolvedValue([{ assetId: 1, score: 0.8 }]);
  vi.mocked(assetsByIds).mockRejectedValue("照片读取失败");
  const { result } = renderHook(useSemanticSearch);
  await act(() => result.current.run("猫"));
  expect(result.current.status).toBe("error");
});
it("旧搜索晚返回不会覆盖新搜索结果", async () => {
  let resolve!: (v: {assetId:number;score:number}[]) => void;
  vi.mocked(searchSemantic).mockReturnValueOnce(new Promise(r => { resolve = r; })).mockResolvedValueOnce([{assetId:2,score:0.9}]);
  vi.mocked(assetsByIds).mockResolvedValue([assetFixture(2)]);
  const { result } = renderHook(useSemanticSearch);
  let old!: Promise<void>;
  act(() => { old = result.current.run("旧"); });
  await act(() => result.current.run("新"));
  await act(async () => { resolve([]); await old; });
  expect(result.current.status).toBe("ready");
  expect(result.current.assets.map(a => a.id)).toEqual([2]);
});
