import { describe, expect, it } from "vitest";
import { blockersText, oneLine } from "./sessionText";

describe("sessionText", () => {
  it("oneLine: пробелы и переводы строк схлопываются, длинный текст обрезается многоточием", () => {
    expect(oneLine(null)).toBe("");
    expect(oneLine("")).toBe("");
    expect(oneLine("  select\n\t 1  ")).toBe("select 1");
    expect(oneLine("a".repeat(120))).toBe("a".repeat(120));
    expect(oneLine("a".repeat(121))).toBe(`${"a".repeat(120)}…`);
    expect(oneLine(`${"b".repeat(50)}\n\n${"c".repeat(500)}`, 60)).toBe(`${"b".repeat(50)} ${"c".repeat(9)}…`);
  });

  it("oneLine не режет суррогатную пару посередине", () => {
    const s = oneLine("😀".repeat(10), 5);
    expect(s).toBe(`${"😀".repeat(5)}…`);
  });

  it("blockersText: 0 — подготовленная транзакция или завершившаяся сессия", () => {
    expect(blockersText([])).toBe("");
    expect(blockersText([4, 9])).toBe("4, 9");
    expect(blockersText([0, 9])).toBe("prepared transaction или завершившаяся сессия, 9");
  });
});
