import { describe, expect, it } from "vitest";

// Справка рендерит главы по CommonMark (react-markdown без remark-gfm, src/components/help/HelpPanel.tsx): таблица GFM
// видна сырыми строками «| … |». Поэтому в главах руководства таблиц нет — только списки.
const chapters = import.meta.glob<string>("../../docs/guide/*/*.md", { query: "?raw", import: "default", eager: true });

/** Строка-разделитель таблицы GFM: только `|`, `-`, `:` и пробелы, есть и `|`, и `-` (`|---|`, `| :-- |`, `---|---`). */
const isDelimiterRow = (line: string) => /^[\s|:-]+$/.test(line) && line.includes("|") && line.includes("-");

/** Номера строк (с 1), где начинается таблица GFM: строка с `|`, за ней разделитель; код в ``` и ~~~ не в счёт. */
function gfmTableStarts(text: string): number[] {
  const lines = text.split("\n");
  const starts: number[] = [];
  let fence = false;
  for (let i = 0; i < lines.length; i++) {
    if (/^\s*(```|~~~)/.test(lines[i])) {
      fence = !fence;
      continue;
    }
    if (!fence && lines[i].includes("|") && i + 1 < lines.length && isDelimiterRow(lines[i + 1])) starts.push(i + 1);
  }
  return starts;
}

describe("guide chapters", () => {
  it("the detector finds GFM tables and skips code and plain pipes", () => {
    expect(gfmTableStarts("Текст\n\n| A | B |\n|---|---|\n| 1 | 2 |\n")).toEqual([3]);
    expect(gfmTableStarts("| A |\n| :-- |\n")).toEqual([1]);
    expect(gfmTableStarts("A | B\n---|---\n")).toEqual([1]);
    expect(gfmTableStarts("```\n| A | B |\n|---|---|\n```\n")).toEqual([]);
    expect(gfmTableStarts("Команда `a | b` и линия\n\n---\n")).toEqual([]);
  });

  it("contain no GFM tables: the help renders CommonMark, where a table shows as raw «| … |» lines — use lists", () => {
    const paths = Object.keys(chapters);
    expect(paths.some((p) => p.includes("/ru/"))).toBe(true);
    expect(paths.some((p) => p.includes("/en/"))).toBe(true);
    const offenders = Object.entries(chapters).flatMap(([path, text]) =>
      gfmTableStarts(text).map((line) => `${path.replace("../../", "")}:${line}`),
    );
    expect(offenders, "таблицы GFM в справке не отображаются (CommonMark без remark-gfm) — перепишите их списками").toEqual([]);
  });
});
