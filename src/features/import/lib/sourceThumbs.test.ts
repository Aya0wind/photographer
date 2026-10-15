import {expect, it} from "vitest";
import {sourceFileVersion} from "./sourceThumbs";

it("scopes unknown metadata to a listing entry while keeping repeat views cached", () => {
  const file = {mtime: "1970-01-01T00:00:00.000Z", size: 0};
  expect(sourceFileVersion(file)).toBe(sourceFileVersion(file));
  expect(sourceFileVersion({...file})).not.toBe(sourceFileVersion(file));
  expect(sourceFileVersion({mtime: "today", size: 42})).toBe("today:42");
});
