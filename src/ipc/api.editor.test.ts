import { beforeEach, describe, expect, it, vi } from "vitest";

import { invoke } from "@tauri-apps/api/core";

import {
  editRecipeDelete,
  editRecipeGet,
  editRecipeSave,
  exportRun,
  resetIpcAvailable,
  type EditRecipe,
  type ExportOptions,
} from "./api";

/** 阶段 D 编辑/导出契约封装：命令名 snake_case、payload camelCase（与后端 lane 共同遵守） */

const invokeMock = vi.mocked(invoke);

const RECIPE: EditRecipe = {
  version: 1,
  rotateQuarter: 1,
  crop: { x: 0.1, y: 0.2, w: 0.5, h: 0.4 },
  textLayers: [{ id: "t1", x: 0.25, y: 0.3, text: "你好\n世界", sizeRel: 0.06, color: "#FFFFFF" }],
  brushStrokes: [{ id: "s1", color: "#FF5252", widthRel: 0.008, points: [{ x: 0.1, y: 0.1 }, { x: 0.2, y: 0.2 }] }],
  output: { longEdge: 2560, quality: 90 },
};

const OPTIONS: ExportOptions = {
  mode: "album",
  album: { albumId: "3", subgroup: "精修" },
  longEdge: 2560,
  quality: 90,
  removeGps: true,
  keywords: ["婚礼", "逆光"],
};

beforeEach(() => {
  invokeMock.mockReset();
  resetIpcAvailable();
});

describe("editRecipeGet", () => {
  it("命令名/参数：edit_recipe_get + { assetId }（字符串 id，后端 parse_asset_id 契约）", async () => {
    invokeMock.mockResolvedValue({ recipe: RECIPE, updatedAt: "2026-09-28T10:00:00Z" });
    const state = await editRecipeGet(42);
    expect(invokeMock).toHaveBeenCalledWith("edit_recipe_get", { assetId: "42" });
    expect(state.updatedAt).toBe("2026-09-28T10:00:00Z");
    expect(state.recipe).toEqual(RECIPE);
  });

  it("无配方（recipe null）→ { recipe: null, updatedAt: null }", async () => {
    invokeMock.mockResolvedValue({ recipe: null, updatedAt: null });
    expect(await editRecipeGet(1)).toEqual({ recipe: null, updatedAt: null });
  });

  it("命令失败静默降级 null 态（不抛出）", async () => {
    invokeMock.mockRejectedValue("command edit_recipe_get not found");
    await expect(editRecipeGet(1)).resolves.toEqual({ recipe: null, updatedAt: null });
  });

  it("脏数据容错：version!=1 / 缺字段 → recipe null", async () => {
    invokeMock.mockResolvedValue({ recipe: { ...RECIPE, version: 2 }, updatedAt: "x" });
    expect((await editRecipeGet(1)).recipe).toBeNull();
    invokeMock.mockResolvedValue({ recipe: { rotateQuarter: 7 } });
    expect((await editRecipeGet(1)).recipe).toBeNull();
  });
});

describe("editRecipeSave / editRecipeDelete", () => {
  it("edit_recipe_save：payload 逐字段等于配方 JSON（assetId 字符串 + snake_case 命令）", async () => {
    invokeMock.mockResolvedValue({ recipe: RECIPE, updatedAt: "2026-09-28T10:00:00Z" });
    await editRecipeSave(42, RECIPE);
    expect(invokeMock).toHaveBeenCalledWith("edit_recipe_save", { assetId: "42", recipe: RECIPE });
  });

  it("edit_recipe_save 失败透传（不吞错）", async () => {
    invokeMock.mockRejectedValue("数据库不可写");
    await expect(editRecipeSave(1, RECIPE)).rejects.toBe("数据库不可写");
  });

  it("edit_recipe_delete：命令名与参数", async () => {
    invokeMock.mockResolvedValue(undefined);
    await editRecipeDelete(42);
    expect(invokeMock).toHaveBeenCalledWith("edit_recipe_delete", { assetId: "42" });
  });
});

describe("exportRun", () => {
  it("export_run：payload { assetId(字符串), recipe, options }，返回 ExportTaskDto", async () => {
    invokeMock.mockResolvedValue({
      id: 7,
      assetId: 42,
      mode: "album",
      status: "queued",
      result: null,
      error: null,
    });
    const result = await exportRun(42, RECIPE, OPTIONS);
    expect(invokeMock).toHaveBeenCalledWith("export_run", {
      assetId: "42",
      recipe: RECIPE,
      options: OPTIONS,
    });
    expect(result).toEqual({
      ok: true,
      task: { id: 7, assetId: 42, mode: "album", status: "queued", result: null, error: null },
    });
  });

  it("id 缺失/非法 → { ok: false, error: null }", async () => {
    invokeMock.mockResolvedValue(undefined);
    expect(await exportRun(1, RECIPE, OPTIONS)).toEqual({ ok: false, error: null });
    invokeMock.mockResolvedValue({ id: "x" });
    expect(await exportRun(1, RECIPE, OPTIONS)).toEqual({ ok: false, error: null });
  });

  it("业务错误透传原始 Err 文案", async () => {
    invokeMock.mockRejectedValue("输出目录不可写");
    const result = await exportRun(1, RECIPE, OPTIONS);
    expect(result).toEqual({ ok: false, error: "输出目录不可写" });
  });

  it("invoke 不可用类错误 → error=null（UI 显示通用文案）", async () => {
    invokeMock.mockRejectedValue("__TAURI_INTERNALS__ invoke is not available");
    const result = await exportRun(1, RECIPE, OPTIONS);
    expect(result).toEqual({ ok: false, error: null });
  });

  it("folder 模式 options 形状：folder 子对象、album 缺省", async () => {
    invokeMock.mockResolvedValue({ id: 1, assetId: 1, mode: "folder", status: "queued", result: null, error: null });
    const folderOptions: ExportOptions = {
      mode: "folder",
      folder: { outputDir: "D:\\交付", fileName: "DSC_1234_edit.jpg" },
      quality: 95,
    };
    await exportRun(1, RECIPE, folderOptions);
    expect(invokeMock).toHaveBeenCalledWith("export_run", {
      assetId: "1",
      recipe: RECIPE,
      options: folderOptions,
    });
  });
});
