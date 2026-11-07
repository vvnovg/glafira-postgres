import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import Markdown from "react-markdown";
import { describe, expect, it } from "vitest";
import { remarkTables } from "./remarkTables";

// Справка рисует таблицы GFM плагином remarkTables (src/components/help/HelpPanel.tsx). Тест следит, чтобы каждая таблица
// глав руководства разбиралась таблицей, а не уходила абзацем с сырыми «| … |», и чтобы ячейки строк не терялись: лишние
// ячейки строки GFM молча отбрасывает.
const chapters = import.meta.glob<string>("../../docs/guide/*/*.md", { query: "?raw", import: "default", eager: true });

/** Строка-разделитель таблицы GFM: только `|`, `-`, `:` и пробелы, есть и `|`, и `-` (`|---|`, `| :-- |`, `---|---`). */
const isDelimiterRow = (line: string) => /^[\s|:-]+$/.test(line) && line.includes("|") && line.includes("-");

/** Число ячеек строки таблицы: `|` по краям не в счёт, `\|` — не разделитель. */
const cellCount = (line: string) => line.trim().replace(/^\|/, "").replace(/(?<!\\)\|$/, "").split(/(?<!\\)\|/).length;

/** Таблицы GFM текста: номер первой строки (с 1) и строки таблицы; код в ``` и ~~~ не в счёт. */
function gfmTables(text: string): { start: number; rows: string[] }[] {
  const lines = text.split("\n");
  const tables: { start: number; rows: string[] }[] = [];
  let fence = false;
  for (let i = 0; i < lines.length; i++) {
    if (/^\s*(```|~~~)/.test(lines[i])) {
      fence = !fence;
      continue;
    }
    if (fence || !lines[i].includes("|") || i + 1 >= lines.length || !isDelimiterRow(lines[i + 1])) continue;
    const rows = [lines[i], lines[i + 1]];
    let j = i + 2;
    while (j < lines.length && lines[j].trim() !== "" && lines[j].includes("|")) rows.push(lines[j++]);
    tables.push({ start: i + 1, rows });
    i = j - 1;
  }
  return tables;
}

const render = (markdown: string) =>
  renderToStaticMarkup(createElement(Markdown, { skipHtml: true, remarkPlugins: [remarkTables] }, markdown));

describe("guide chapters", () => {
  it("the detector finds GFM tables and skips code and plain pipes", () => {
    expect(gfmTables("Текст\n\n| A | B |\n|---|---|\n| 1 | 2 |\n").map((t) => t.start)).toEqual([3]);
    expect(gfmTables("| A |\n| :-- |\n")).toEqual([{ start: 1, rows: ["| A |", "| :-- |"] }]);
    expect(gfmTables("A | B\n---|---\n").map((t) => t.start)).toEqual([1]);
    expect(gfmTables("```\n| A | B |\n|---|---|\n```\n")).toEqual([]);
    expect(gfmTables("Команда `a | b` и линия\n\n---\n")).toEqual([]);
    expect(cellCount("| a | b \\| c | d |")).toBe(3);
    expect(cellCount("a | b")).toBe(2);
  });

  it("render their GFM tables as tables, with as many cells in every row as in the header", () => {
    const paths = Object.keys(chapters);
    expect(paths.some((p) => p.includes("/ru/"))).toBe(true);
    expect(paths.some((p) => p.includes("/en/"))).toBe(true);
    let total = 0;
    for (const [path, text] of Object.entries(chapters)) {
      const name = path.replace("../../", "");
      const tables = gfmTables(text);
      total += tables.length;
      const html = render(text);
      expect((html.match(/<table>/g) ?? []).length, `${name}: таблицы GFM должны отрисоваться таблицами`).toBe(
        tables.length,
      );
      expect(html, `${name}: сырой разделитель таблицы в тексте`).not.toMatch(/\|\s*:?-{3}/);
      for (const t of tables) {
        const header = cellCount(t.rows[0]);
        const bad = t.rows.filter((row) => cellCount(row) !== header);
        expect(bad, `${name}:${t.start}: у строк таблицы должно быть ${header} ячеек`).toEqual([]);
      }
    }
    // Таблицы в справке есть («Локальные модели»): тест проверяет их, а не пустоту.
    expect(total).toBeGreaterThan(0);
  });
});
