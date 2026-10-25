import { describe, expect, it } from "vitest";
import { helpAnchorFor, helpChapterFor } from "./helpChapterFor";

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
});
