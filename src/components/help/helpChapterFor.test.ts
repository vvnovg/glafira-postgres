import { describe, expect, it } from "vitest";
import type { DiagTool } from "../../api/types";
import { loadGuide } from "../../guide/chapters";
import { parseChapter } from "../../guide/parse";
import type { TabSpec } from "../../state/workspace";
import { helpAnchorFor, helpChapterFor } from "./helpChapterFor";

/** Все инструменты диагностики: тип `Record` не даст добавить инструмент, не внеся его сюда (а значит, и в проверку якорей). */
const DIAG_TOOLS: Record<DiagTool, true> = { pulse: true, sessions: true, idleTx: true, waits: true, locks: true, horizon: true, replication: true, slots: true, failover: true, health: true, queries: true };

describe("helpChapterFor", () => {
  it("для null, настроек и обзора — «Начало работы»", () => {
    expect(helpChapterFor(null)).toBe("getting-started");
    expect(helpChapterFor({ kind: "settings" })).toBe("getting-started");
    expect(helpChapterFor({ kind: "overview", connectionId: "c1" })).toBe("getting-started");
    expect(helpChapterFor({ kind: "help" })).toBe("getting-started");
  });

  it("для формы подключения — «Подключения»", () => {
    expect(helpChapterFor({ kind: "connection-form", connectionId: null })).toBe("connections");
    expect(helpChapterFor({ kind: "connection-form", connectionId: "c1" })).toBe("connections");
  });

  it("для вкладок таблицы, представления, функции и последовательности — «Дерево и объекты»", () => {
    expect(helpChapterFor({ kind: "table", connectionId: "c1", database: "orders", schema: "shop", name: "orders" })).toBe("objects");
    expect(helpChapterFor({ kind: "view", connectionId: "c1", database: "orders", schema: "shop", name: "v", matview: false })).toBe("objects");
    expect(helpChapterFor({ kind: "view", connectionId: "c1", database: "orders", schema: "shop", name: "mv", matview: true })).toBe("objects");
    expect(helpChapterFor({ kind: "function", connectionId: "c1", database: "orders", oid: 16384, name: "shop.touch" })).toBe("objects");
    expect(helpChapterFor({ kind: "sequence", connectionId: "c1", database: "orders", schema: "shop", name: "orders_id_seq" })).toBe("objects");
  });

  it("для «О программе» — «Обновления и диагностика»", () => {
    expect(helpChapterFor({ kind: "about" })).toBe("updates");
  });

  it("вкладка инструмента — глава «Диагностика» с якорем инструмента", () => {
    expect(helpChapterFor({ kind: "diag", connectionId: "c", tool: "locks" })).toBe("diagnostics");
    expect(helpAnchorFor({ kind: "diag", connectionId: "c", tool: "locks" })).toBe("locks");
    expect(helpAnchorFor({ kind: "diag", connectionId: "c", tool: "idleTx" })).toBe("sessions");
    expect(helpAnchorFor({ kind: "overview", connectionId: "c" })).toBeUndefined();
    expect(helpAnchorFor(null)).toBeUndefined();
  });

  it("инструмент «Запросы», карточка запроса и консоль EXPLAIN — глава «Запросы» с якорями top, card и console", () => {
    const tool = { kind: "diag", connectionId: "c", tool: "queries" } as const;
    const query = { kind: "query", connectionId: "c", member: "pg-1", database: "perf", queryId: "1" } as const;
    const explain = { kind: "explain", connectionId: "c" } as const;
    expect(helpChapterFor(tool)).toBe("queries");
    expect(helpChapterFor(query)).toBe("queries");
    expect(helpChapterFor(explain)).toBe("queries");
    expect(helpAnchorFor(tool)).toBe("top");
    expect(helpAnchorFor(query)).toBe("card");
    expect(helpAnchorFor(explain)).toBe("console");
  });

  it("вкладка «Журнал действий» — глава «Диагностика», якорь journal", () => {
    expect(helpChapterFor({ kind: "actions", connectionId: "c" })).toBe("diagnostics");
    expect(helpAnchorFor({ kind: "actions", connectionId: "c" })).toBe("journal");
  });

  it("F1 каждого инструмента и вкладок запроса, консоли и журнала ведёт в раздел {#id} своей главы на обоих языках (справка при отсутствии якоря молча прокручивает наверх)", async () => {
    const tabs: TabSpec[] = [
      ...(Object.keys(DIAG_TOOLS) as DiagTool[]).map((tool): TabSpec => ({ kind: "diag", connectionId: "c", tool })),
      { kind: "query", connectionId: "c", member: "pg-1", database: "perf", queryId: "1" },
      { kind: "explain", connectionId: "c" },
      { kind: "actions", connectionId: "c" },
    ];
    for (const lang of ["en", "ru"]) {
      const guide = await loadGuide(lang);
      for (const tab of tabs) {
        const chapter = helpChapterFor(tab);
        const anchor = helpAnchorFor(tab);
        const label = tab.kind === "diag" ? `${tab.kind}:${tab.tool}` : tab.kind;
        const anchors = parseChapter(chapter, guide[chapter], (key) => key).sections.map((x) => x.id);
        expect(anchor, `${lang}: ${label}`).toBeDefined();
        expect(anchors, `${lang}: якорь ${anchor} вкладки ${label} в главе ${chapter}`).toContain(anchor);
      }
    }
  });

  it("разделы действий — {#actions}, {#patroni-actions} и {#journal} — есть в главе «Диагностика» на обоих языках, а F1 журнала ведёт в {#journal}", async () => {
    const journal = helpAnchorFor({ kind: "actions", connectionId: "c" });
    expect(journal).toBe("journal");
    for (const lang of ["en", "ru"]) {
      const chapter = (await loadGuide(lang)).diagnostics;
      const anchors = parseChapter("diagnostics", chapter, (key) => key).sections.map((s) => s.id);
      for (const anchor of ["actions", "patroni-actions", journal]) {
        expect(anchors, `${lang}: якорь ${anchor}`).toContain(anchor);
      }
    }
  });
});
