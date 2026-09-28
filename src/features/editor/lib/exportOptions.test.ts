import { describe, expect, it } from "vitest";

import { defaultExportDraft, parseKeywords, parseLongEdge, validateExportDraft, type ExportOptionsDraft } from "./exportOptions";

function draft(overrides: Partial<ExportOptionsDraft> = {}): ExportOptionsDraft {
  return {
    ...defaultExportDraft({ name: "DSC_1234.NEF" }),
    outputDir: "D:\\交付",
    ...overrides,
  };
}

describe("defaultExportDraft", () => {
  it("默认导出到目录、文件名 {stem}_edit.jpg、保留 GPS", () => {
    const d = defaultExportDraft({ name: "DSC_1234.NEF" });
    expect(d.mode).toBe("folder");
    expect(d.fileName).toBe("DSC_1234_edit.jpg");
    expect(d.removeGps).toBe(false);
    expect(d.longEdge).toBe("");
    // 无扩展名 / 多点文件名
    expect(defaultExportDraft({ name: "photo" }).fileName).toBe("photo_edit.jpg");
    expect(defaultExportDraft({ name: "a.b.c.jpg" }).fileName).toBe("a.b.c_edit.jpg");
  });
});

describe("parseLongEdge", () => {
  it("空 → null（原尺寸）", () => {
    expect(parseLongEdge("")).toBeNull();
    expect(parseLongEdge("   ")).toBeNull();
  });
  it("正整数字符串 → 数值", () => {
    expect(parseLongEdge("2560")).toBe(2560);
    expect(parseLongEdge(" 1920 ")).toBe(1920);
  });
  it("非法值 → NaN", () => {
    expect(parseLongEdge("0")).toBeNaN();
    expect(parseLongEdge("-100")).toBeNaN();
    expect(parseLongEdge("abc")).toBeNaN();
    expect(parseLongEdge("2560.5")).toBeNaN();
    expect(parseLongEdge("1e3")).toBeNaN();
  });
});

describe("parseKeywords", () => {
  it("中英文逗号分隔、去空白、去重", () => {
    expect(parseKeywords("婚礼, 逆光，婚礼, portrait ")).toEqual(["婚礼", "逆光", "portrait"]);
    expect(parseKeywords("")).toEqual([]);
    expect(parseKeywords(" , ,，")).toEqual([]);
  });
});

describe("validateExportDraft", () => {
  it("folder 模式合法草稿 → 契约 ExportOptions", () => {
    const result = validateExportDraft(
      draft({ longEdge: "2560", copyright: "© 2026 Studio", author: " 摄影师 ", keywords: "婚礼, 逆光" }),
      85,
    );
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.options.mode).toBe("folder");
    expect(result.options.folder).toEqual({ outputDir: "D:\\交付", fileName: "DSC_1234_edit.jpg" });
    expect(result.options.album).toBeUndefined();
    expect(result.options.longEdge).toBe(2560);
    expect(result.options.quality).toBe(85);
    expect(result.options.removeGps).toBe(false);
    expect(result.options.copyright).toBe("© 2026 Studio");
    expect(result.options.author).toBe("摄影师");
    expect(result.options.keywords).toEqual(["婚礼", "逆光"]);
  });

  it("长边留空 → longEdge null；quality 夹取 1-100", () => {
    const result = validateExportDraft(draft({ longEdge: "" }), 250);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.options.longEdge).toBeNull();
    expect(result.options.quality).toBe(100);
  });

  it("folder 缺目录 → folderDirMissing", () => {
    const result = validateExportDraft(draft({ outputDir: "" }), 90);
    expect(result).toEqual({ ok: false, errors: ["folderDirMissing"] });
  });

  it("folder 缺文件名 → folderNameMissing", () => {
    const result = validateExportDraft(draft({ fileName: "  " }), 90);
    expect(result).toEqual({ ok: false, errors: ["folderNameMissing"] });
  });

  it("folder 文件名非法字符 → folderNameInvalid", () => {
    for (const bad of ['a/b.jpg', 'a\\b.jpg', 'a:b', 'a*b', 'a?b', 'a"b', "a<b", "a>b", "a|b"]) {
      const result = validateExportDraft(draft({ fileName: bad }), 90);
      expect(result).toEqual({ ok: false, errors: ["folderNameInvalid"] });
    }
  });

  it("album 模式缺相册 → albumMissing；合法时 subgroup 空串 → null", () => {
    const missing = validateExportDraft(draft({ mode: "album", albumId: "" }), 90);
    expect(missing).toEqual({ ok: false, errors: ["albumMissing"] });

    const ok = validateExportDraft(draft({ mode: "album", albumId: " 3 ", subgroup: "  " }), 90);
    expect(ok.ok).toBe(true);
    if (!ok.ok) return;
    expect(ok.options.album).toEqual({ albumId: "3", subgroup: null });
    expect(ok.options.folder).toBeUndefined();
  });

  it("album 子分组保留；长边非法叠加报错", () => {
    const result = validateExportDraft(
      draft({ mode: "album", albumId: "3", subgroup: "精修", longEdge: "abc" }),
      90,
    );
    expect(result).toEqual({ ok: false, errors: ["longEdgeInvalid"] });

    const ok = validateExportDraft(
      draft({ mode: "album", albumId: "3", subgroup: "精修" }),
      90,
    );
    expect(ok.ok).toBe(true);
    if (!ok.ok) return;
    expect(ok.options.album).toEqual({ albumId: "3", subgroup: "精修" });
  });

  it("空元数据字段不进契约负载", () => {
    const result = validateExportDraft(draft({ copyright: "", author: "", keywords: "" }), 90);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect("copyright" in result.options).toBe(false);
    expect("author" in result.options).toBe(false);
    expect("keywords" in result.options).toBe(false);
  });
});
