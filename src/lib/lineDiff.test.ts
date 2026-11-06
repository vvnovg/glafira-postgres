import { describe, expect, it } from "vitest";
import { LCS_CELLS, lineDiff, sideBySide } from "./lineDiff";

/** Сравнение в виде строк «±текст»: так ожидания читаются как обычный diff. */
const marks = (a: string, b: string) => lineDiff(a, b).map((d) => `${d.kind === "same" ? " " : d.kind === "del" ? "-" : "+"}${d.text}`);

describe("lineDiff", () => {
  it("одинаковые тексты — все строки общие; CRLF и LF равны", () => {
    expect(marks("a\nb", "a\r\nb")).toEqual([" a", " b"]);
  });

  it("замена строки: удаление перед добавлением", () => {
    expect(marks("SELECT *\nFROM t\nWHERE a = 1", "SELECT *\nFROM t\nWHERE a = 2")).toEqual([" SELECT *", " FROM t", "-WHERE a = 1", "+WHERE a = 2"]);
  });

  it("вставка и удаление", () => {
    expect(marks("a\nc", "a\nb\nc")).toEqual([" a", "+b", " c"]);
    expect(marks("a\nb\nc", "a\nc")).toEqual([" a", "-b", " c"]);
  });

  it("перестановка находит наибольшую общую часть", () => {
    expect(marks("a\nb\nc\nd", "b\nc\nd\na")).toEqual(["-a", " b", " c", " d", "+a"]);
  });

  it("пустой текст — ни одной строки", () => {
    expect(marks("", "x")).toEqual(["+x"]);
    expect(marks("x", "")).toEqual(["-x"]);
    expect(marks("", "")).toEqual([]);
  });

  it("слишком большая середина сравнивается без таблицы: общие начало и конец, середина удалена и добавлена целиком", () => {
    const n = Math.ceil(Math.sqrt(LCS_CELLS)) + 1;
    const a = ["head", ...Array.from({ length: n }, (_, i) => `a${i}`), "tail"].join("\n");
    const b = ["head", ...Array.from({ length: n }, (_, i) => `b${i}`), "tail"].join("\n");
    const d = lineDiff(a, b);
    expect(d[0]).toEqual({ kind: "same", text: "head" });
    expect(d[d.length - 1]).toEqual({ kind: "same", text: "tail" });
    expect(d.slice(1, n + 1).every((x) => x.kind === "del")).toBe(true);
    expect(d.slice(n + 1, 2 * n + 1).every((x) => x.kind === "add")).toBe(true);
    expect(d).toHaveLength(2 * n + 2);
  });
});

describe("sideBySide", () => {
  it("общие строки — в обеих колонках; удаления и добавления блока — парами, остаток — с пустой стороной; номера у каждой стороны свои", () => {
    expect(sideBySide(lineDiff("a\nb1\nb2\nc", "a\nB\nc\nd"))).toEqual([
      { left: { kind: "same", text: "a", n: 1 }, right: { kind: "same", text: "a", n: 1 } },
      { left: { kind: "del", text: "b1", n: 2 }, right: { kind: "add", text: "B", n: 2 } },
      { left: { kind: "del", text: "b2", n: 3 }, right: null },
      { left: { kind: "same", text: "c", n: 4 }, right: { kind: "same", text: "c", n: 3 } },
      { left: null, right: { kind: "add", text: "d", n: 4 } },
    ]);
  });
});
