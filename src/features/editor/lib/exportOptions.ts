import type { AssetDto, ExportOptions } from "@/ipc/api";

/**
 * 导出选项草稿（表单态字符串）→ 契约 ExportOptions 的构建与校验。
 * 校验规则：
 * - folder 模式必填 outputDir 与 fileName（非法字符 / \ : * ? " < > | 拒绝）；
 * - album 模式必填 albumId（文件名后端自动生成 {stem}_edit.jpg）；
 * - 长边留空 = 原尺寸（null）；非空必须为正整数。
 */

export interface ExportOptionsDraft {
  mode: "folder" | "album";
  outputDir: string;
  fileName: string;
  albumId: string;
  subgroup: string;
  /** 留空 = 原尺寸 */
  longEdge: string;
  removeGps: boolean;
  copyright: string;
  author: string;
  /** 逗号分隔输入 */
  keywords: string;
}

export type ExportDraftError =
  | "folderDirMissing"
  | "folderNameMissing"
  | "folderNameInvalid"
  | "albumMissing"
  | "longEdgeInvalid";

const FILE_NAME_FORBIDDEN = /[\\/:*?"<>|]/;

/** 文件名去扩展名取 stem（无点 = 全名） */
function stemOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}

/** 默认草稿：导出到目录、文件名 {stem}_edit.jpg、保留 GPS */
export function defaultExportDraft(asset: Pick<AssetDto, "name">): ExportOptionsDraft {
  return {
    mode: "folder",
    outputDir: "",
    fileName: `${stemOf(asset.name)}_edit.jpg`,
    albumId: "",
    subgroup: "",
    longEdge: "",
    removeGps: false,
    copyright: "",
    author: "",
    keywords: "",
  };
}

/** 解析长边输入："" → null；正整数字符串 → 数值；其余 → NaN（非法） */
export function parseLongEdge(raw: string): number | null {
  const trimmed = raw.trim();
  if (trimmed === "") return null;
  if (!/^\d+$/.test(trimmed)) return Number.NaN;
  const value = Number(trimmed);
  return Number.isInteger(value) && value >= 1 ? value : Number.NaN;
}

/** 关键词输入 → 去重关键词数组（中英文逗号分隔） */
export function parseKeywords(raw: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const part of raw.split(/[,，]/)) {
    const keyword = part.trim();
    if (keyword !== "" && !seen.has(keyword)) {
      seen.add(keyword);
      out.push(keyword);
    }
  }
  return out;
}

export type ExportDraftValidation =
  | { ok: true; options: ExportOptions }
  | { ok: false; errors: ExportDraftError[] };

/** 校验并构建契约 ExportOptions（quality 来自配方 output，1-100 夹取） */
export function validateExportDraft(
  draft: ExportOptionsDraft,
  quality: number,
): ExportDraftValidation {
  const errors: ExportDraftError[] = [];
  const longEdge = parseLongEdge(draft.longEdge);
  if (Number.isNaN(longEdge)) errors.push("longEdgeInvalid");

  let folder: ExportOptions["folder"];
  let album: ExportOptions["album"];
  if (draft.mode === "folder") {
    const outputDir = draft.outputDir.trim();
    const fileName = draft.fileName.trim();
    if (outputDir === "") errors.push("folderDirMissing");
    if (fileName === "") errors.push("folderNameMissing");
    else if (FILE_NAME_FORBIDDEN.test(fileName)) errors.push("folderNameInvalid");
    if (!errors.includes("folderDirMissing") && !errors.includes("folderNameMissing") && !errors.includes("folderNameInvalid")) {
      folder = { outputDir, fileName };
    }
  } else {
    const albumId = draft.albumId.trim();
    if (albumId === "") errors.push("albumMissing");
    else {
      const subgroup = draft.subgroup.trim();
      album = { albumId, subgroup: subgroup === "" ? null : subgroup };
    }
  }

  if (errors.length > 0) return { ok: false, errors };

  const options: ExportOptions = {
    mode: draft.mode,
    folder,
    album,
    longEdge,
    quality: Math.min(100, Math.max(1, Math.round(quality))),
    removeGps: draft.removeGps,
    copyright: draft.copyright.trim() || undefined,
    author: draft.author.trim() || undefined,
    keywords: parseKeywords(draft.keywords),
  };
  if (options.copyright === undefined) delete options.copyright;
  if (options.author === undefined) delete options.author;
  if (options.keywords !== undefined && options.keywords.length === 0) delete options.keywords;
  return { ok: true, options };
}
