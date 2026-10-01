import { describe, expect, it } from "vitest";
import { directoryComparisonKey, directoryPreview, trimDirectoryEnd } from "./filesystemPaths";

describe("filesystem path display contracts", () => {
  it("preserves POSIX case, Unicode and literal backslashes", () => {
    expect(directoryComparisonKey("/Volumes/Card/A/")).toBe("/Volumes/Card/A");
    expect(directoryComparisonKey("/Volumes/Card/A")).not.toBe(directoryComparisonKey("/Volumes/Card/a"));
    expect(directoryComparisonKey("/Users/me/照片\\")).toBe("/Users/me/照片\\");
  });
  it("keeps Windows comparisons case-insensitive with either separator", () => {
    expect(directoryComparisonKey("D:/Photos/")).toBe(directoryComparisonKey("d:\\photos"));
    expect(directoryComparisonKey("\\\\server\\share\\")).toBe("\\\\server\\share");
  });
  it("does not erase a root or turn a Windows drive root into a relative path", () => {
    expect(trimDirectoryEnd("/")).toBe("/");
    expect(trimDirectoryEnd("C:\\")).toBe("C:\\");
    expect(trimDirectoryEnd("C:/")).toBe("C:\\");
    expect(trimDirectoryEnd("")).toBe("");
  });
  it("renders each destination independently, including root and backslash filenames", () => {
    expect(directoryPreview("/", "2026", "09", "相册")).toBe("/2026/09/相册/");
    expect(directoryPreview("/Volumes/照片\\", "2026")).toBe("/Volumes/照片\\/2026/");
    expect(directoryPreview("D:\\", "2026", "Album")).toBe("D:\\2026\\Album\\");
    expect(directoryPreview("D:/backup/", "2026")).toBe("D:\\backup\\2026\\");
  });
});
