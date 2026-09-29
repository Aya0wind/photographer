import type { AssetDto } from "@/ipc/api";

/** 完整资产 DTO；测试用覆盖值表达日期、格式、标记等场景差异。 */
export function assetFixture(id: number, overrides: Partial<AssetDto> = {}): AssetDto {
  return {
    id,
    path: `Y:\\照片\\IMG_${id}.JPG`,
    name: `IMG_${id}.JPG`,
    kind: "photo",
    capturedAt: null,
    camera: null,
    sizeBytes: 1,
    ...overrides,
  };
}
