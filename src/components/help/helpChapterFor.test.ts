import { describe, expect, it } from "vitest";
import { helpChapterFor } from "./helpChapterFor";

describe("helpChapterFor", () => {
  it("для null и для настроек — «Начало работы»", () => {
    expect(helpChapterFor(null)).toBe("getting-started");
    expect(helpChapterFor({ kind: "settings" })).toBe("getting-started");
  });
});
