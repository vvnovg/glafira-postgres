import { describe, expect, it } from "vitest";
import type { DiagTool } from "../../api/types";
import { loadGuide } from "../../guide/chapters";
import { parseChapter } from "../../guide/parse";
import { helpAnchorFor, helpChapterFor } from "./helpChapterFor";

/** Все инструменты диагностики: тип `Record` не даст добавить инструмент, не внеся его сюда (а значит, и в проверку якорей). */
const DIAG_TOOLS: Record<DiagTool, true> = { pulse: true, sessions: true, idleTx: true, waits: true, locks: true, horizon: true, replication: true, slots: true, failover: true, health: true };

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

  it("вкладка «Журнал действий» — глава «Диагностика», якорь journal", () => {
    expect(helpChapterFor({ kind: "actions", connectionId: "c" })).toBe("diagnostics");
    expect(helpAnchorFor({ kind: "actions", connectionId: "c" })).toBe("journal");
  });

  it("якорь F1 каждого инструмента — раздел {#id} главы «Диагностика» на обоих языках (справка при отсутствии якоря молча прокручивает наверх)", async () => {
    for (const lang of ["en", "ru"]) {
      const chapter = (await loadGuide(lang)).diagnostics;
      const anchors = parseChapter("diagnostics", chapter, (key) => key).sections.map((s) => s.id);
      for (const tool of Object.keys(DIAG_TOOLS) as DiagTool[]) {
        const anchor = helpAnchorFor({ kind: "diag", connectionId: "c", tool });
        expect(anchor, `${lang}: ${tool}`).toBeDefined();
        expect(anchors, `${lang}: якорь ${anchor} инструмента ${tool}`).toContain(anchor);
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
