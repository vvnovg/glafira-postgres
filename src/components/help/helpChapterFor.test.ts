import { describe, expect, it } from "vitest";
import { helpChapterFor } from "./helpChapterFor";

describe("helpChapterFor", () => {
  it("для null, настроек и подключений — «Начало работы»", () => {
    expect(helpChapterFor(null)).toBe("getting-started");
    expect(helpChapterFor({ kind: "settings" })).toBe("getting-started");
    expect(helpChapterFor({ kind: "connection-form", connectionId: null })).toBe("getting-started");
  });

  it("для «О программе» — «Обновления и диагностика»", () => {
    expect(helpChapterFor({ kind: "about" })).toBe("updates");
  });
});
