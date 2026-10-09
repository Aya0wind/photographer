/** 动画关闭态契约（settings.appearance.animations=false）：
 *  查看器立即卸载不延迟、路由内容直通渲染、上下文菜单无入场缩放残留。 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import { DEFAULT_SETTINGS, useSettingsStore } from "@/stores/settingsStore";
import ViewerOverlay from "@/features/gallery/components/ViewerOverlay";
import { AssetContextMenu } from "@/features/gallery/components/ContextMenu";
import type { AssetDto } from "@/ipc/api";
import type { AssetGroup } from "@/features/gallery/lib/assetGroups";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetThumbGet: vi.fn().mockResolvedValue({ status: "unavailable" }),
    assetDetail: vi.fn().mockResolvedValue(null),
    assetRatingSet: vi.fn().mockResolvedValue(undefined),
    assetFlagSet: vi.fn().mockResolvedValue(undefined),
  };
});

vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: vi.fn().mockResolvedValue(undefined),
}));

function setAnimations(enabled: boolean): void {
  useSettingsStore.setState({
    settings: { ...DEFAULT_SETTINGS, appearance: { animations: enabled } },
    loaded: true,
  });
}

const ASSET = {
  id: 1,
  name: "DSC_0001.JPG",
  kind: "photo",
} as unknown as AssetDto;

const GROUP = { key: "g", assets: [ASSET, { ...ASSET, id: 2, name: "DSC_0002.JPG" }] } as unknown as AssetGroup;

function renderViewer(onClose: () => void): void {
  render(
    <I18nextProvider i18n={i18n}>
      <ViewerOverlay asset={ASSET} group={GROUP} index={0} onNavigate={vi.fn()} onClose={onClose} />
    </I18nextProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  setAnimations(true);
});

describe("动画关闭态（no-motion 契约）", () => {
  it("查看器关闭即时回调（无 150ms 退场延迟）", () => {
    setAnimations(false);
    const onClose = vi.fn();
    renderViewer(onClose);
    expect(screen.getByTestId("viewer")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("viewer-close"));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("上下文菜单关闭态直出（无入场缩放的初始样式）", async () => {
    setAnimations(false);
    render(
      <I18nextProvider i18n={i18n}>
        <AssetContextMenu at={{ x: 10, y: 10 }} assets={[ASSET]} onClose={vi.fn()} />
      </I18nextProvider>,
    );
    const menu = await screen.findByRole("menu");
    // initial=false：mount 即终态，无 scale/opacity 过渡内联痕迹
    expect(menu.style.transform).not.toContain("scale(0.96");
    expect(menu.style.opacity).not.toBe("0");
  });

  it("开启态查看器关闭走退场延迟（对照：动画开=延迟回调）", async () => {
    setAnimations(true);
    const onClose = vi.fn();
    renderViewer(onClose);
    fireEvent.click(screen.getByTestId("viewer-close"));
    // 退场动画期间未回调；150ms 后回调
    expect(onClose).not.toHaveBeenCalledTimes(2);
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1), { timeout: 1000 });
  });
});
