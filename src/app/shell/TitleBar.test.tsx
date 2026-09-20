import { beforeEach, describe, expect, it, vi } from "vitest";

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter } from "react-router";

import i18n from "@/i18n";
import TitleBar from "./TitleBar";
import { getCurrentWindow } from "@tauri-apps/api/window";

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(),
}));

const getCurrentWindowMock = vi.mocked(getCurrentWindow);

interface FakeWindow {
  minimize: ReturnType<typeof vi.fn>;
  toggleMaximize: ReturnType<typeof vi.fn>;
  close: ReturnType<typeof vi.fn>;
  isMaximized: ReturnType<typeof vi.fn>;
  onResized: ReturnType<typeof vi.fn>;
}

/** 可控窗口实例：isMaximized/onResized 可按用例覆写 */
function fakeWindow(overrides: Partial<FakeWindow> = {}): FakeWindow {
  return {
    minimize: vi.fn().mockResolvedValue(undefined),
    toggleMaximize: vi.fn().mockResolvedValue(undefined),
    close: vi.fn().mockResolvedValue(undefined),
    isMaximized: vi.fn().mockResolvedValue(false),
    onResized: vi.fn().mockResolvedValue(() => {}),
    ...overrides,
  };
}

/** mock getCurrentWindow 返回可控实例（cast 到真实 Window 类型满足 mock 签名） */
function useWindow(win: FakeWindow): void {
  getCurrentWindowMock.mockImplementation(
    () => win as unknown as ReturnType<typeof getCurrentWindow>,
  );
}

function renderBar() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter>
        <TitleBar />
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  getCurrentWindowMock.mockReset();
  useWindow(fakeWindow());
});

describe("TitleBar 结构", () => {
  it("渲染拖拽背景层（data-tauri-drag-region）与应用标识", async () => {
    renderBar();

    const drag = screen.getByTestId("titlebar-drag-region");
    expect(drag).toHaveAttribute("data-tauri-drag-region");
    expect(screen.getByText("Photo Hub")).toBeInTheDocument();
    // 标识与控制钮的容器不拦截拖拽（透传到背景层）
    expect(drag.parentElement).not.toBeNull();
  });

  it("三个窗口控制钮带 aria-label/title；关闭钮红色 hover 样式", () => {
    renderBar();

    const min = screen.getByTestId("titlebar-minimize");
    expect(min).toHaveAttribute("aria-label", "最小化");
    expect(min).toHaveAttribute("title", "最小化");
    expect(screen.getByTestId("titlebar-maximize")).toHaveAttribute("aria-label", "最大化");

    const close = screen.getByTestId("titlebar-close");
    expect(close).toHaveAttribute("aria-label", "关闭");
    // Windows 惯例：关闭钮 hover 红底白字
    expect(close.className).toContain("hover:bg-[#C42B1C]");
    expect(close.className).toContain("hover:text-white");
    expect(min.className).not.toContain("hover:bg-[#C42B1C]");
  });
});

describe("窗口控制 API", () => {
  it("三钮分别调用 minimize / toggleMaximize / close", async () => {
    const win = fakeWindow();
    useWindow(win);
    renderBar();
    const user = userEvent.setup();

    await user.click(screen.getByTestId("titlebar-minimize"));
    await user.click(screen.getByTestId("titlebar-maximize"));
    await user.click(screen.getByTestId("titlebar-close"));

    expect(win.minimize).toHaveBeenCalledTimes(1);
    expect(win.toggleMaximize).toHaveBeenCalledTimes(1);
    expect(win.close).toHaveBeenCalledTimes(1);
  });

  it("mount 时查询 isMaximized：已最大化 → 还原图标与文案", async () => {
    useWindow(fakeWindow({ isMaximized: vi.fn().mockResolvedValue(true) }));
    renderBar();

    await waitFor(() => {
      expect(screen.getByTestId("titlebar-maximize")).toHaveAttribute("aria-label", "还原");
    });
  });

  it("resize 后重新查询最大化状态切换图标", async () => {
    let fire: ((...args: unknown[]) => void) | null = null;
    const isMaximized = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    const win = fakeWindow({
      isMaximized,
      onResized: vi.fn().mockImplementation((handler: (...args: unknown[]) => void) => {
        fire = handler;
        return Promise.resolve(() => {});
      }),
    });
    useWindow(win);
    renderBar();
    await waitFor(() => expect(isMaximized).toHaveBeenCalledTimes(1));
    expect(screen.getByTestId("titlebar-maximize")).toHaveAttribute("aria-label", "最大化");

    // 窗口 resize → 触发订阅回调 → isMaximized()=true → 切换为还原
    await waitFor(() => expect(fire).not.toBeNull());
    await act(async () => {
      (fire as (...args: unknown[]) => void)();
    });
    await waitFor(() => {
      expect(screen.getByTestId("titlebar-maximize")).toHaveAttribute("aria-label", "还原");
    });
  });
});

describe("动作位（actions）", () => {
  it("主壳传入的动作内容渲染在标题栏内（全局搜索框等）", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter>
          <TitleBar actions={<div data-testid="titlebar-extra">EXTRA</div>} />
        </MemoryRouter>
      </I18nextProvider>,
    );

    const bar = screen.getByTestId("titlebar");
    expect(within(bar).getByTestId("titlebar-extra")).toBeInTheDocument();
    // 顶部菜单栏已移除：无 menubar 结构
    expect(screen.queryByTestId("menubar")).not.toBeInTheDocument();
  });

  it("无 actions（选择器/向导）也可独立渲染", () => {
    renderBar();
    expect(screen.getByTestId("titlebar")).toBeInTheDocument();
    expect(screen.getByTestId("titlebar-close")).toBeInTheDocument();
  });
});

describe("jsdom 事件直发（不依赖 pointer-events 命中测试）", () => {
  it("双击拖拽层不触发前端 toggleMaximize（由 Tauri 注入脚本处理，避免双触发）", async () => {
    const win = fakeWindow();
    useWindow(win);
    renderBar();

    fireEvent.dblClick(screen.getByTestId("titlebar-drag-region"));

    // 前端不绑定双击最大化：调用次数保持 0（原生脚本 internal_toggle_maximize 才是通道）
    expect(win.toggleMaximize).not.toHaveBeenCalled();
  });
});
