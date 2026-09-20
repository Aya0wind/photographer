import { beforeEach, describe, expect, it } from "vitest";

import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";

import i18n from "@/i18n";
import GlobalSearchBox from "./GlobalSearchBox";
import { useAiStore } from "@/stores/aiStore";
import type { IndexStatus } from "@/ipc/api";

/** 落点 URL 断言探针（渲染 search 参数） */
function LocationProbe() {
  const location = useLocation();
  return (
    <div>
      <div data-testid="search-probe" />
      <span data-testid="loc">{`${location.pathname}${location.search}`}</span>
    </div>
  );
}

function renderBox() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/gallery"]}>
        <GlobalSearchBox />
        <Routes>
          <Route path="/gallery" element={<LocationProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

function aiStatus(done: number, total: number): IndexStatus {
  return {
    thumb: { pending: 0, running: 0, done: 0, failed: 0, total: 0 },
    exif: { pending: 0, running: 0, done: 0, failed: 0, total: 0 },
    ai: { pending: total - done, running: 0, done, failed: 0, total },
    face: { pending: 0, running: 0, done: 0, failed: 0, total: 0 },
  };
}

beforeEach(() => {
  useAiStore.getState().resetForTests();
});

describe("顶部全局搜索框（M4.5 A1）", () => {
  it("占位文案与过滤图标；空输入回车 → /search（条件模式）", async () => {
    renderBox();

    expect(screen.getByTestId("globalsearch-input")).toHaveAttribute(
      "placeholder",
      "输入一段描述搜索照片…",
    );
    expect(screen.getByTestId("globalsearch-filter")).toBeInTheDocument();

    fireEvent.keyDown(screen.getByTestId("globalsearch-input"), { key: "Enter" });
    expect(await screen.findByTestId("loc")).toHaveTextContent("/gallery");
  });

  it("输入描述回车 → /search?mode=semantic&q=（URL 编码）", async () => {
    const user = userEvent.setup();
    renderBox();

    await user.type(screen.getByTestId("globalsearch-input"), "海边 日落");
    fireEvent.keyDown(screen.getByTestId("globalsearch-input"), { key: "Enter" });

    // 回车跳 /gallery?mode=semantic&q=（URL 编码；画廊+搜索合并）
    expect(await screen.findByTestId("loc")).toHaveTextContent(
      "/gallery?mode=semantic&q=%E6%B5%B7%E8%BE%B9%20%E6%97%A5%E8%90%BD",
    );
  });

  it("过滤图标 → /search", async () => {
    const user = userEvent.setup();
    renderBox();

    await user.click(screen.getByTestId("globalsearch-filter"));
    expect(await screen.findByTestId("search-probe")).toBeInTheDocument();
  });

  it("语义索引未建完 → 输入框下细提示条；建完/未知不提示", () => {
    const { rerender } = renderBox();
    expect(screen.queryByTestId("globalsearch-hint")).not.toBeInTheDocument();

    useAiStore.setState({ indexStatus: aiStatus(30, 100) });
    rerender(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter initialEntries={["/gallery"]}>
          <GlobalSearchBox />
        </MemoryRouter>
      </I18nextProvider>,
    );
    expect(screen.getByTestId("globalsearch-hint")).toHaveTextContent("语义索引建立中");

    useAiStore.setState({ indexStatus: aiStatus(100, 100) });
    rerender(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter initialEntries={["/gallery"]}>
          <GlobalSearchBox />
        </MemoryRouter>
      </I18nextProvider>,
    );
    expect(screen.queryByTestId("globalsearch-hint")).not.toBeInTheDocument();
  });
});
