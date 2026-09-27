import { beforeEach, describe, expect, it, vi } from "vitest";

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter } from "react-router";

import i18n from "@/i18n";
import { albumList, smartViewCreate, type AiModelStatus } from "@/ipc/api";
import { useAiStore } from "@/stores/aiStore";
import {
  FilterPanel,
  buildChips,
  buildFilters,
  inputsFromFilters,
  EMPTY_INPUTS,
  type SearchInputs,
} from "./FilterPanel";

/**
 * 筛选面板 B1 维度：颜色标签（LR 五色点单选）/ 已拒绝（三态）→ buildFilters
 * 映射 filters.colorLabel / filters.rejected；chips 展示并可单独移除；
 * 智能视图保存行（有激活条件才出现；命名 → smart_view_create；重名错误行内提示）；
 * inputsFromFilters 反解（已存视图应用）。
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    albumList: vi.fn(),
    smartViewCreate: vi.fn(),
  };
});

const albumListMock = vi.mocked(albumList);
const createMock = vi.mocked(smartViewCreate);

function renderPanel(inputs: SearchInputs = EMPTY_INPUTS, onSaved?: () => void) {
  const onPatch = vi.fn();
  render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter>
        <FilterPanel inputs={inputs} onPatch={onPatch} onSmartViewSaved={onSaved} />
      </MemoryRouter>
    </I18nextProvider>,
  );
  return { onPatch };
}

beforeEach(() => {
  vi.clearAllMocks();
  useAiStore.setState({ models: [], modelsLoaded: false, indexStatus: null });
  albumListMock.mockResolvedValue([]);
  createMock.mockReset();
});

// --- 颜色标签维度 ---------------------------------------------------------------------

describe("筛选面板：颜色标签维度（B1）", () => {
  it("五色点 + 全部渲染为单选组；点红色 → onPatch({color:'red'})", async () => {
    const user = userEvent.setup();
    const { onPatch } = renderPanel();

    const group = screen.getByTestId("search-color");
    expect(group).toHaveAttribute("role", "radiogroup");
    for (const label of ["red", "yellow", "green", "blue", "purple"]) {
      expect(screen.getByTestId(`search-color-${label}`)).toHaveAttribute("aria-checked", "false");
    }

    await user.click(screen.getByTestId("search-color-red"));
    expect(onPatch).toHaveBeenCalledWith({ color: "red" });
  });

  it("已选红色：aria-checked=true；再点同色 = 取消（回 all）", async () => {
    const user = userEvent.setup();
    const { onPatch } = renderPanel({ ...EMPTY_INPUTS, color: "red" });

    expect(screen.getByTestId("search-color-red")).toHaveAttribute("aria-checked", "true");
    expect(screen.getByTestId("search-color-all")).toHaveAttribute("aria-checked", "false");
    await user.click(screen.getByTestId("search-color-red"));
    expect(onPatch).toHaveBeenCalledWith({ color: "all" });
  });

  it("buildFilters：color → filters.colorLabel；all 不传", () => {
    expect(buildFilters({ ...EMPTY_INPUTS, color: "green" }).colorLabel).toBe("green");
    expect(buildFilters(EMPTY_INPUTS).colorLabel).toBeUndefined();
  });

  it("chips：颜色 chip（label 含中文色名）可单独移除", () => {
    const inputs: SearchInputs = { ...EMPTY_INPUTS, color: "red" };
    const chips = buildChips(inputs, (key) => key);
    const chip = chips.find((c) => c.key === "color");
    expect(chip).toBeDefined();
    expect(chip?.label).toContain("search.color");
    expect(chip?.label).toContain("gallery.color.red");
    expect(chip?.patch.color).toBe("all");
  });
});

// --- 已拒绝三态 -----------------------------------------------------------------------

describe("筛选面板：已拒绝三态（B1）", () => {
  it("是/否/不限分段；点「是」→ onPatch({rejected:'yes'})", async () => {
    const user = userEvent.setup();
    const { onPatch } = renderPanel();

    expect(screen.getByTestId("search-rejected-all")).toHaveAttribute("aria-checked", "true");
    await user.click(screen.getByTestId("search-rejected-yes"));
    expect(onPatch).toHaveBeenCalledWith({ rejected: "yes" });
  });

  it("buildFilters：yes→true / no→false / all 不传", () => {
    expect(buildFilters({ ...EMPTY_INPUTS, rejected: "yes" }).rejected).toBe(true);
    expect(buildFilters({ ...EMPTY_INPUTS, rejected: "no" }).rejected).toBe(false);
    expect(buildFilters(EMPTY_INPUTS).rejected).toBeUndefined();
  });

  it("chips：已拒绝/未拒绝 chip 可单独移除", () => {
    const yes = buildChips({ ...EMPTY_INPUTS, rejected: "yes" }, (key) => key);
    expect(yes.find((c) => c.key === "rejected")?.label).toBe("search.rejected.yes");
    const no = buildChips({ ...EMPTY_INPUTS, rejected: "no" }, (key) => key);
    expect(no.find((c) => c.key === "rejected")?.patch.rejected).toBe("all");
  });
});

// --- 智能视图保存行 -------------------------------------------------------------------

describe("筛选面板：保存为视图（B1）", () => {
  it("默认态（无激活条件）不显示保存行；有激活条件才出现", () => {
    renderPanel();
    expect(screen.queryByTestId("smart-view-save-row")).not.toBeInTheDocument();

    renderPanel({ ...EMPTY_INPUTS, color: "blue" });
    expect(screen.getByTestId("smart-view-save-row")).toBeInTheDocument();
  });

  it("命名 → smart_view_create(name, JSON.stringify(buildFilters))；成功清空输入并回调", async () => {
    const user = userEvent.setup();
    createMock.mockResolvedValue({ ok: true, view: { id: 1, name: "蓝色横拍", filtersJson: "{}", createdAt: "2026-09-27" } });
    const onSaved = vi.fn();
    renderPanel({ ...EMPTY_INPUTS, color: "blue", orientation: "landscape" }, onSaved);

    await user.type(screen.getByTestId("smart-view-name-input"), "蓝色横拍");
    await user.click(screen.getByTestId("smart-view-save"));

    await waitFor(() => expect(createMock).toHaveBeenCalledOnce());
    const [name, filtersJson] = createMock.mock.calls[0];
    expect(name).toBe("蓝色横拍");
    expect(JSON.parse(filtersJson)).toMatchObject({ colorLabel: "blue", orientation: "landscape" });
    await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
    expect(screen.getByTestId("smart-view-name-input")).toHaveValue("");
    expect(screen.queryByTestId("smart-view-save-error")).not.toBeInTheDocument();
  });

  it("重名等后端业务错误：原始 Err 文案行内提示", async () => {
    const user = userEvent.setup();
    createMock.mockResolvedValue({ ok: false, error: "已存在同名视图" });
    renderPanel({ ...EMPTY_INPUTS, rejected: "yes" });

    await user.type(screen.getByTestId("smart-view-name-input"), "已拒绝");
    await user.click(screen.getByTestId("smart-view-save"));

    const error = await screen.findByTestId("smart-view-save-error");
    expect(error).toHaveTextContent("已存在同名视图");
    expect(screen.getByTestId("smart-view-name-input")).toHaveValue("已拒绝");
  });

  it("空名点保存：行内提示必填，不调 IPC", async () => {
    const user = userEvent.setup();
    renderPanel({ ...EMPTY_INPUTS, favoriteOnly: true });

    await user.click(screen.getByTestId("smart-view-save"));
    expect(screen.getByTestId("smart-view-save-error")).toHaveTextContent("请填写视图名称");
    expect(createMock).not.toHaveBeenCalled();
  });
});

// --- inputsFromFilters 反解（已存视图应用） ---------------------------------------------

describe("inputsFromFilters：filters → 面板输入（B1）", () => {
  it("关键字段反解：buildFilters(inputsFromFilters(f)) 语义等价（UI 可产出维度）", () => {
    const via = (inputs: SearchInputs) => buildFilters(inputsFromFilters(buildFilters(inputs)));
    const sample: SearchInputs = {
      ...EMPTY_INPUTS,
      favoriteOnly: true,
      kind: "raw",
      from: "2026-09-01",
      to: "2026-09-27",
      cameras: ["Canon EOS R5"],
      lenses: ["RF 50mm"],
      formats: ["NEF"],
      orientation: "landscape",
      flash: "on",
      gps: "no",
      focalMin: "24",
      focalMax: "70",
      isoMin: "100",
      isoMax: "1600",
      apertureMin: "2.8",
      shutterMax: "0.004",
      sizeMin: "1",
      sizeMax: "20",
      color: "purple",
      rejected: "no",
    };
    const round = via(sample);
    expect(round).toEqual(buildFilters(sample));
  });

  it("B1 新维度直映：colorLabel/rejected", () => {
    const inputs = inputsFromFilters({ colorLabel: "yellow", rejected: true });
    expect(inputs.color).toBe("yellow");
    expect(inputs.rejected).toBe("yes");
  });

  it("空 filters → 默认输入；ratingMin<5 不误判收藏", () => {
    const inputs = inputsFromFilters({});
    expect(inputs).toEqual(EMPTY_INPUTS);
    expect(inputsFromFilters({ ratingMin: 3 }).favoriteOnly).toBe(false);
  });
});

// --- AI 标签区（C 阶段：闭眼两档 + 疑似失焦） ---------------------------------------------

const AI_INDEX_BUSY = {
  thumb: { pending: 0, running: 0, done: 0, failed: 0, total: 0 },
  exif: { pending: 0, running: 0, done: 0, failed: 0, total: 0 },
  ai: { pending: 3, running: 1, done: 0, failed: 0, total: 4 },
};

function selectionModel(): AiModelStatus {
  return {
    id: "eyes",
    installed: true,
    bytesTotal: 10 * 1024 * 1024,
    downloadedBytes: 10 * 1024 * 1024,
    version: "v1.0",
    feature: "selection",
    state: "done",
  };
}

describe("筛选面板：AI 标签（C 阶段）", () => {
  it("未装选片辅助模型：闭眼两档置灰（不触发 onPatch）+ 提示与 ?tab=ai 跳转；失焦恒可用", async () => {
    const user = userEvent.setup();
    const { onPatch } = renderPanel();

    const closed = screen.getByTestId("search-ai-eyes-closed");
    const maybe = screen.getByTestId("search-ai-eyes-maybe");
    expect(closed).toBeDisabled();
    expect(maybe).toBeDisabled();
    const hint = screen.getByTestId("search-ai-model-hint");
    expect(hint).toHaveTextContent("需下载选片辅助包");
    expect(screen.getByTestId("search-ai-model-link")).toHaveAttribute("href", "/settings?tab=ai");

    await user.click(screen.getByTestId("search-ai-blur"));
    expect(onPatch).toHaveBeenCalledWith({ aiBlur: true });
  });

  it("已装模型：闭眼可用且两档互斥（closed→maybe）；再点同档清除", async () => {
    const user = userEvent.setup();
    useAiStore.setState({ models: [selectionModel()], modelsLoaded: true });
    const { onPatch } = renderPanel();

    expect(screen.getByTestId("search-ai-eyes-closed")).toBeEnabled();
    expect(screen.queryByTestId("search-ai-model-hint")).not.toBeInTheDocument();

    await user.click(screen.getByTestId("search-ai-eyes-closed"));
    expect(onPatch).toHaveBeenLastCalledWith({ aiEyes: "closed" });

    // 互斥：面板以 aiEyes="closed" 渲染时点 maybe → "maybe"
    cleanup();
    render(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter>
          <FilterPanel inputs={{ ...EMPTY_INPUTS, aiEyes: "closed" }} onPatch={onPatch} />
        </MemoryRouter>
      </I18nextProvider>,
    );
    await user.click(screen.getByTestId("search-ai-eyes-maybe"));
    expect(onPatch).toHaveBeenLastCalledWith({ aiEyes: "maybe" });
  });

  it("buildFilters：eyes/blur 映射 + 与其它维度组合负载；chips 可单独移除", () => {
    const combined = buildFilters({
      ...EMPTY_INPUTS,
      aiEyes: "closed",
      aiBlur: true,
      orientation: "landscape",
    });
    expect(combined).toEqual({ eyes: "closed", blur: "soft", orientation: "landscape" });
    expect(buildFilters(EMPTY_INPUTS).eyes).toBeUndefined();
    expect(buildFilters(EMPTY_INPUTS).blur).toBeUndefined();

    const chips = buildChips(
      { ...EMPTY_INPUTS, aiEyes: "maybe", aiBlur: true },
      (key) => key,
    );
    const eyesChip = chips.find((c) => c.key === "ai-eyes");
    const blurChip = chips.find((c) => c.key === "ai-blur");
    expect(eyesChip?.label).toBe("search.ai.eyesMaybe");
    expect(eyesChip?.patch.aiEyes).toBe("none");
    expect(blurChip?.patch.aiBlur).toBe(false);
  });

  it("inputsFromFilters 反解：eyes/blur", () => {
    expect(inputsFromFilters({ eyes: "closed", blur: "soft" })).toMatchObject({
      aiEyes: "closed",
      aiBlur: true,
    });
    expect(inputsFromFilters({}).aiEyes).toBe("none");
    expect(inputsFromFilters({}).aiBlur).toBe(false);
  });

  it("「分析中」提示：AI 通道有未完成项 → 结果可能不全提示", () => {
    useAiStore.setState({ models: [selectionModel()], modelsLoaded: true, indexStatus: AI_INDEX_BUSY });
    renderPanel();
    expect(screen.getByTestId("search-ai-running-hint")).toHaveTextContent("AI 分析进行中，结果可能不全");
  });
});
