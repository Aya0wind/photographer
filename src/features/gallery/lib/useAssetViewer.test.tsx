import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";

vi.mock("./viewMark", () => ({ markAssetViewed: vi.fn() }));

import { useAssetViewer } from "./useAssetViewer";
import type { AssetDto } from "@/ipc/api";
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

describe("useAssetViewer", () => {
  it("navigates across date groups in the current result set", () => {
    render(<MemoryRouter><Harness /></MemoryRouter>);
    fireEvent.click(screen.getByRole("button", { name: "open" }));
    expect(screen.getByTestId("viewer-position")).toHaveTextContent("2:1:3");
    fireEvent.click(screen.getByRole("button", { name: "next" }));
    expect(screen.getByTestId("viewer-position")).toHaveTextContent("3:2:3");
  });
});
