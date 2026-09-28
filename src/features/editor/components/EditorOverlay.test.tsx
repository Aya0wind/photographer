import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import type { AssetDto, EditRecipe } from "@/ipc/api";
import EditorOverlay from "./EditorOverlay";

/**
 * 编辑器浮层组件测试：Konva 画布以 stub 替身（jsdom 无 canvas），经 stub 暴露的
 * 回调驱动「放置文字/提交笔迹」等交互路径；几何/坐标换算见 lib/konvaMapping.test.ts。
 */

vi.mock("@/features/editor/components/EditorCanvas", () => ({
  default: (props: {
    tool: string;
    recipe: { rotateQuarter: number; textLayers: unknown[] };
    cropDraft: unknown;
    onPlaceText: (pos: { x: number; y: number }) => void;
    onStrokeCommit: (points: { x: number; y: number }[]) => void;
    onImageError: () => void;
  }) => (
    <div
      data-testid="editor-canvas-stub"
      data-tool={props.tool}
      data-rotate={props.recipe.rotateQuarter}
    >
      <button
        type="button"
        data-testid="stub-place-text"
        onClick={() => props.onPlaceText({ x: 0.25, y: 0.3 })}
      >
        place
      </button>
      <button
        type="button"
        data-testid="stub-image-error"
        onClick={() => props.onImageError()}
      >
        img-error
      </button>
      <button
        type="button"
        data-testid="stub-stroke"
        onClick={() => props.onStrokeCommit([{ x: 0.1, y: 0.1 }, { x: 0.3, y: 0.3 }])}
      >
        stroke
      </button>
      <span data-testid="stub-crop">{props.cropDraft !== null ? "draft" : "none"}</span>
      <span data-testid="stub-text-layers">{props.recipe.textLayers.length}</span>
    </div>
  ),
}));

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    editRecipeSave: vi.fn(),
    editRecipeDelete: vi.fn(),
    exportRun: vi.fn(),
    albumList: vi.fn(),
    albumSubgroups: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  isTauri: vi.fn(() => false),
  convertFileSrc: vi.fn(() => "asset://photo"),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

vi.mock("@/features/gallery/lib/thumbPipeline", () => ({
  useAssetThumbUrl: vi.fn(() => ({ url: null, status: "loading" })),
}));

import { useAssetThumbUrl } from "@/features/gallery/lib/thumbPipeline";

import {
  albumList,
  albumSubgroups,
  editRecipeDelete,
  editRecipeSave,
  exportRun,
} from "@/ipc/api";

const saveMock = vi.mocked(editRecipeSave);
const deleteMock = vi.mocked(editRecipeDelete);
const exportMock = vi.mocked(exportRun);
const albumListMock = vi.mocked(albumList);
const albumSubgroupsMock = vi.mocked(albumSubgroups);
const thumbHookMock = vi.mocked(useAssetThumbUrl);

const ASSET: AssetDto = {
  id: 1,
  path: "Y:\\照片\\2026\\DSC_1234.JPG",
  name: "DSC_1234.JPG",
  kind: "photo",
  capturedAt: "2026-09-18T10:00:00",
  camera: "Canon EOS R5",
  sizeBytes: 5 * 1024 * 1024,
  width: 4000,
  height: 3000,
};

function renderEditor(initial: { recipe: EditRecipe | null; updatedAt: string | null } = { recipe: null, updatedAt: null }) {
  const onClose = vi.fn();
  const onSaved = vi.fn();
  render(
    <I18nextProvider i18n={i18n}>
      <EditorOverlay asset={ASSET} initial={initial} onClose={onClose} onSaved={onSaved} />
    </I18nextProvider>,
  );
  return { onClose, onSaved };
}

beforeEach(() => {
  saveMock.mockReset();
  deleteMock.mockReset();
  exportMock.mockReset();
  albumListMock.mockReset();
  thumbHookMock.mockReset().mockReturnValue({ url: null, status: "loading" });
  saveMock.mockImplementation(async (_id: number, recipe: EditRecipe) => ({
    recipe,
    updatedAt: "2026-09-28T00:00:00Z",
  }));
  deleteMock.mockResolvedValue(undefined);
  exportMock.mockResolvedValue({
    ok: true,
    task: { id: 7, assetId: 1, mode: "album", status: "queued", result: null, error: null },
  });
  albumSubgroupsMock.mockResolvedValue([]);
  albumListMock.mockResolvedValue([
    { id: 3, name: "婚礼", coverAssetId: null, itemCount: 2, createdAt: "2026-09-01T00:00:00Z" },
  ]);
});

describe("EditorOverlay 骨架与工具切换", () => {
  it("默认浏览工具；撤销/重做初始禁用；已保存配方显示角标", () => {
    const recipe: EditRecipe = {
      version: 1,
      rotateQuarter: 1,
      crop: null,
      textLayers: [],
      brushStrokes: [],
      output: { longEdge: null, quality: 90 },
    };
    renderEditor({ recipe, updatedAt: "x" });
    expect(screen.getByTestId("editor-canvas-stub").dataset.tool).toBe("view");
    expect(screen.getByTestId("editor-undo")).toBeDisabled();
    expect(screen.getByTestId("editor-redo")).toBeDisabled();
    expect(screen.getByTestId("editor-edited-badge")).toHaveTextContent("已编辑");
  });

  it("切换裁剪工具进入草稿态；文字工具可放置", async () => {
    const user = userEvent.setup();
    renderEditor();
    await user.click(screen.getByTestId("editor-tool-crop"));
    expect(screen.getByTestId("editor-canvas-stub").dataset.tool).toBe("crop");
    expect(screen.getByTestId("stub-crop")).toHaveTextContent("draft");
    expect(screen.getByTestId("editor-crop-options")).toBeInTheDocument();

    await user.click(screen.getByTestId("editor-tool-text"));
    expect(screen.getByTestId("editor-canvas-stub").dataset.tool).toBe("text");
    expect(screen.getByTestId("stub-crop")).toHaveTextContent("none");
    expect(screen.getByTestId("editor-text-options")).toBeInTheDocument();
  });
});

describe("EditorOverlay 编辑操作与撤销", () => {
  it("放置文字 → 输入内容 → 撤销移除", async () => {
    const user = userEvent.setup();
    renderEditor();
    await user.click(screen.getByTestId("editor-tool-text"));
    await user.click(screen.getByTestId("stub-place-text"));
    const input = screen.getByTestId("editor-text-input");
    expect(input).toBeInTheDocument();
    await user.type(input, "你好");
    expect(screen.getByTestId("stub-text-layers")).toHaveTextContent("1");

    await user.click(screen.getByTestId("editor-undo"));
    expect(screen.getByTestId("stub-text-layers")).toHaveTextContent("0");
    expect(screen.queryByTestId("editor-text-input")).not.toBeInTheDocument();
  });

  it("画笔一笔进撤销栈", async () => {
    const user = userEvent.setup();
    renderEditor();
    await user.click(screen.getByTestId("editor-tool-brush"));
    await user.click(screen.getByTestId("stub-stroke"));
    await user.click(screen.getByTestId("editor-undo"));
    expect(screen.getByTestId("editor-undo")).toBeDisabled();
  });

  it("旋转 90° 步进 + 撤销复位", async () => {
    const user = userEvent.setup();
    renderEditor();
    await user.click(screen.getByTestId("editor-rotate-cw"));
    expect(screen.getByTestId("editor-canvas-stub").dataset.rotate).toBe("1");
    await user.click(screen.getByTestId("editor-rotate-cw"));
    expect(screen.getByTestId("editor-canvas-stub").dataset.rotate).toBe("2");
    await user.click(screen.getByTestId("editor-undo"));
    expect(screen.getByTestId("editor-canvas-stub").dataset.rotate).toBe("1");
    await user.click(screen.getByTestId("editor-rotate-ccw"));
    expect(screen.getByTestId("editor-canvas-stub").dataset.rotate).toBe("0");
  });
});

describe("EditorOverlay 保存/重置", () => {
  it("保存配方：空占位层被剔除后落库，onSaved 回传", async () => {
    const user = userEvent.setup();
    const { onSaved } = renderEditor();
    // 放置一个文字层但不输入内容（空层不入库）
    await user.click(screen.getByTestId("editor-tool-text"));
    await user.click(screen.getByTestId("stub-place-text"));
    await user.click(screen.getByTestId("editor-save"));
    await waitFor(() => expect(saveMock).toHaveBeenCalledTimes(1));
    const expected: EditRecipe = {
      version: 1,
      rotateQuarter: 0,
      crop: null,
      textLayers: [],
      brushStrokes: [],
      output: { longEdge: null, quality: 90 },
    };
    expect(saveMock).toHaveBeenCalledWith(1, expected);
    await waitFor(() =>
      expect(screen.getByTestId("editor-toast").dataset.kind).toBe("save-ok"),
    );
    expect(onSaved).toHaveBeenCalledWith(expected);
  });

  it("重置：确认后 editRecipeDelete + 配方复位", async () => {
    const user = userEvent.setup();
    renderEditor({
      recipe: {
        version: 1,
        rotateQuarter: 2,
        crop: null,
        textLayers: [],
        brushStrokes: [],
        output: { longEdge: null, quality: 90 },
      },
      updatedAt: "x",
    });
    await user.click(screen.getByTestId("editor-rotate-cw")); // 制造本地改动（quarter 2→3）
    await user.click(screen.getByTestId("editor-reset"));
    expect(screen.getByTestId("editor-confirm-dialog")).toBeInTheDocument();
    await user.click(screen.getByTestId("editor-confirm-ok"));
    await waitFor(() => expect(deleteMock).toHaveBeenCalledWith(1));
    expect(screen.getByTestId("editor-canvas-stub").dataset.rotate).toBe("0");
    expect(screen.getByTestId("editor-undo")).toBeDisabled();
  });
});

describe("EditorOverlay 导出对话框", () => {
  it("folder 缺目录 → 确认禁用并提示；album 缺相册同理；选相册后可导出", async () => {
    const user = userEvent.setup();
    renderEditor();
    await user.click(screen.getByTestId("editor-export"));

    const dialog = screen.getByTestId("export-dialog");
    expect(dialog).toBeInTheDocument();
    // 默认 folder 模式：目录为空 → 错误 + 确认禁用
    const confirm = screen.getByTestId("export-dialog-confirm");
    expect(confirm).toBeDisabled();
    expect(screen.getByTestId("export-dialog-errors")).toHaveTextContent("请选择输出目录");

    // 切 album 模式：未选相册
    await user.click(screen.getByTestId("editor-output-mode-album"));
    expect(confirm).toBeDisabled();
    expect(screen.getByTestId("export-dialog-errors")).toHaveTextContent("请选择要加入的相册");

    // 选相册 → 确认可点
    await user.selectOptions(screen.getByTestId("editor-output-album"), "3");
    expect(confirm).toBeEnabled();
    await user.click(confirm);

    await waitFor(() => expect(exportMock).toHaveBeenCalledTimes(1));
    const [assetId, , options] = exportMock.mock.calls[0];
    expect(assetId).toBe(1);
    expect(options.mode).toBe("album");
    expect(options.album).toEqual({ albumId: "3", subgroup: null });
    expect(options.quality).toBe(90);
    expect(options.removeGps).toBe(false);
    expect("copyright" in options).toBe(false);
    await waitFor(() =>
      expect(screen.getByTestId("editor-toast").dataset.kind).toBe("export-running"),
    );
  });

  it("folder 模式目录必填（只读输入经系统选择器选取）；文件名默认 {stem}_edit.jpg", async () => {
    const user = userEvent.setup();
    renderEditor();
    await user.click(screen.getByTestId("editor-export"));
    expect(screen.getByTestId("editor-output-dir")).toHaveAttribute("readonly");
    expect(screen.getByTestId("editor-output-filename")).toHaveValue("DSC_1234_edit.jpg");
    expect(screen.getByTestId("export-dialog-confirm")).toBeDisabled();
    expect(screen.getByTestId("export-dialog-errors")).toHaveTextContent("请选择输出目录");
    // 摘要含元数据写入目标说明（§8：区分写入目标）
    expect(screen.getByTestId("export-dialog-summary")).toHaveTextContent("不会修改原片");
  });
});

describe("EditorOverlay 关闭确认", () => {
  it("未保存改动关闭需确认：继续编辑留在编辑器，丢弃后 onClose", async () => {
    const user = userEvent.setup();
    const { onClose } = renderEditor();
    // 无改动：直接关闭
    await user.click(screen.getByTestId("editor-close"));
    expect(onClose).toHaveBeenCalledTimes(1);

    // 制造未保存改动（放置 + 输入文字）
    await user.click(screen.getByTestId("editor-tool-text"));
    await user.click(screen.getByTestId("stub-place-text"));
    await user.type(screen.getByTestId("editor-text-input"), "标题");
    expect(screen.getByTestId("editor-dirty")).toBeInTheDocument();

    await user.click(screen.getByTestId("editor-close"));
    expect(screen.getByTestId("editor-confirm-dialog")).toBeInTheDocument();
    await user.click(screen.getByTestId("editor-confirm-cancel"));
    expect(onClose).toHaveBeenCalledTimes(1); // 仍是第一次的直接关闭

    await user.click(screen.getByTestId("editor-close"));
    await user.click(screen.getByTestId("editor-confirm-ok"));
    expect(onClose).toHaveBeenCalledTimes(2);
  });
});

// --- 源缺失（missing 终态：源文件被第三方移动/删除） --------------------------------------

describe("EditorOverlay 源缺失（missing）", () => {
  it("thumb 结算 missing：与 load-failed 同位置显示缺失文案，画布替换、保存/导出禁用 + title 提示", () => {
    thumbHookMock.mockReturnValue({ url: null, status: "missing" });
    renderEditor();

    const missing = screen.getByTestId("editor-load-missing");
    expect(missing).toHaveTextContent("源文件已被移动或删除，无法编辑");
    // 画布被替换；不再出现无限 loading spinner（此前真机「编辑器无限转圈」根因）
    expect(screen.queryByTestId("editor-canvas-stub")).not.toBeInTheDocument();
    expect(screen.queryByTestId("editor-loading")).not.toBeInTheDocument();

    const save = screen.getByTestId("editor-save");
    expect(save).toBeDisabled();
    expect(save).toHaveAttribute("title", "源文件已被移动或删除，无法编辑");
    const exportBtn = screen.getByTestId("editor-export");
    expect(exportBtn).toBeDisabled();
    expect(exportBtn).toHaveAttribute("title", "源文件已被移动或删除，无法编辑");
  });

  it("missing 有历史缓存 url：同样进入缺失态（编辑/导出作用于源文件，缓存图不可编辑）", () => {
    thumbHookMock.mockReturnValue({ url: "asset://cache.jpg", status: "missing" });
    renderEditor();

    expect(screen.getByTestId("editor-load-missing")).toBeInTheDocument();
    expect(screen.getByTestId("editor-save")).toBeDisabled();
  });

  it("failed（不可解码）态不受影响：仍显示 load-failed 文案", () => {
    thumbHookMock.mockReturnValue({ url: null, status: "failed" });
    renderEditor();

    // photo 的 failed 判定要求原图先加载失败降档到 thumb 档（真机链路同序）
    fireEvent.click(screen.getByTestId("stub-image-error"));
    expect(screen.getByTestId("editor-load-failed")).toHaveTextContent("无法加载预览大图");
    expect(screen.queryByTestId("editor-load-missing")).not.toBeInTheDocument();
  });
});
