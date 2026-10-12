import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { IndexStatus } from "@/ipc/api/types";
vi.mock("@/ipc/api", () => ({ aiModelsStatus: vi.fn(), indexStatus: vi.fn(), subscribeAppEvents: vi.fn() }));
import { aiModelsStatus, indexStatus } from "@/ipc/api";
import { useAiStore } from "./aiStore";
const statusMock=vi.mocked(indexStatus);
const snapshot=(done:number):IndexStatus => {
  const c={done,pending:100-done,running:0,failed:0,total:100};
  return { image:c,thumb:c,exif:c,ai:c };
};
beforeEach(()=>{vi.useFakeTimers(); useAiStore.getState().resetForTests();statusMock.mockReset();});
afterEach(()=>{useAiStore.getState().resetForTests();vi.useRealTimers();});

it("高频进度不会把已返回的快照全部作废，合并为一个后续刷新",async()=>{
  let resolve!:(s:IndexStatus)=>void;
  statusMock.mockReturnValueOnce(new Promise(r=>{resolve=r;})).mockResolvedValue(snapshot(100));
  const work=useAiStore.getState().refreshIndexStatus();
  for(let i=0;i<100;i++) useAiStore.getState().handleAppEvent({type:"indexTaskProgress",kind:"eyes",done:i,total:100});
  expect(statusMock).toHaveBeenCalledTimes(1);
  resolve(snapshot(10));
  await Promise.resolve();
  expect(useAiStore.getState().indexStatus?.image?.done).toBe(10);
  await vi.advanceTimersByTimeAsync(250);
  await work;
  expect(statusMock).toHaveBeenCalledTimes(2);
  expect(useAiStore.getState().indexStatus?.image?.done).toBe(100);
});

it("切库后旧请求不能覆盖新库进度",async()=>{
  let resolve!:(s:IndexStatus)=>void;
  statusMock.mockReturnValueOnce(new Promise(r=>{resolve=r;})).mockResolvedValueOnce(snapshot(3));
  const old=useAiStore.getState().refreshIndexStatus();
  useAiStore.getState().resetLibrarySession();
  await useAiStore.getState().refreshIndexStatus();
  resolve(snapshot(100)); await old;
  expect(useAiStore.getState().indexStatus?.image?.done).toBe(3);
});

it("一次查询失败不把有效进度清空成零",async()=>{
  statusMock.mockResolvedValueOnce(snapshot(30)).mockResolvedValueOnce(null);
  await useAiStore.getState().refreshIndexStatus();
  await useAiStore.getState().refreshIndexStatus();
  expect(useAiStore.getState().indexStatus?.image?.done).toBe(30);
});


it("下载结束时合并重拉，旧的缺模型快照不覆盖新状态",async()=>{
  let resolve!:(s:Awaited<ReturnType<typeof aiModelsStatus>>)=>void;
  const ready=[{id:"siglip2-text",state:"done"}] as Awaited<ReturnType<typeof aiModelsStatus>>;
  vi.mocked(aiModelsStatus).mockReset().mockReturnValueOnce(new Promise(r=>{resolve=r;})).mockResolvedValueOnce(ready);
  const pending=useAiStore.getState().refresh();
  useAiStore.getState().handleAppEvent({type:"aiModelDownloadFinished",id:"siglip2-text",ok:true});
  resolve([]); await pending;
  expect(aiModelsStatus).toHaveBeenCalledTimes(2);
  expect(useAiStore.getState().models).toEqual(ready);
});
it("临时连接失败保留已下载的模型状态",async()=>{
  const ready=[{id:"siglip2-text",state:"done"}] as Awaited<ReturnType<typeof aiModelsStatus>>;
  vi.mocked(aiModelsStatus).mockReset().mockResolvedValueOnce(ready).mockRejectedValueOnce("busy");
  await useAiStore.getState().refresh(); await useAiStore.getState().refresh();
  expect(useAiStore.getState().models).toEqual(ready);
});
