import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { openExplainWithPrefill, useWorkspace } from "./workspace";
import { persistWorkspace, restoreWorkspace, useWorkspacePersistence } from "./workspacePersist";

const KEY = "glafira.workspace";
const saved = () => JSON.parse(localStorage.getItem(KEY) ?? "null");

beforeEach(() => {
  localStorage.clear();
  useWorkspace.setState({ tabs: [], activeId: null });
});

describe("сохранение вкладок (смоук P4c, R18)", () => {
  it("пишет открытые вкладки без одноразовых полей и активную; форма подключения не сохраняется", () => {
    const stop = persistWorkspace();
    try {
      const { open } = useWorkspace.getState();
      open({ kind: "diag", connectionId: "c", tool: "queries", member: "pg-1" });
      open({ kind: "query", connectionId: "c", member: "pg-1", database: "perf", queryId: "42", title: "select 1" });
      open({ kind: "connection-form", connectionId: null });
      open({ kind: "settings", section: "agent" });
      open({ kind: "explain", connectionId: "c", member: "pg-2", database: "perf", sql: "SELECT 1" });
      openExplainWithPrefill("c", { sql: "SELECT 2", database: "perf", proposalId: "p1", autoPlan: true });
      expect(saved()).toEqual({
        v: 1,
        tabs: [
          { kind: "diag", connectionId: "c", tool: "queries", member: "pg-1" },
          { kind: "query", connectionId: "c", member: "pg-1", database: "perf", queryId: "42", title: "select 1" },
          { kind: "settings", section: "agent" },
          // Консоль — без текста, базы, узла и подстановки: их держит черновик консоли.
          { kind: "explain", connectionId: "c" },
        ],
        active: "explain:c",
      });
      // Активна форма подключения — активной в сохранённом нет.
      useWorkspace.getState().activate("connection-form:new");
      expect(saved().active).toBeNull();
      // Закрытие вкладки пишется сразу.
      useWorkspace.getState().close("query:c:pg-1:perf:42");
      expect(saved().tabs.map((t: { kind: string }) => t.kind)).toEqual(["diag", "settings", "explain"]);
    } finally {
      stop();
    }
  });

  it("вкладки удалённого подключения уходят из сохранённого (closeForConnection)", () => {
    const stop = persistWorkspace();
    try {
      const { open } = useWorkspace.getState();
      open({ kind: "overview", connectionId: "a" });
      open({ kind: "explain", connectionId: "b" });
      open({ kind: "actions", connectionId: "b" });
      useWorkspace.getState().closeForConnection("b");
      expect(saved()).toEqual({ v: 1, tabs: [{ kind: "overview", connectionId: "a" }], active: "overview:a" });
    } finally {
      stop();
    }
  });

  it("восстанавливает вкладки, подключение которых есть; активная — если восстановлена", () => {
    localStorage.setItem(
      KEY,
      JSON.stringify({
        v: 1,
        tabs: [
          { kind: "overview", connectionId: "a", member: "pg-1" },
          { kind: "explain", connectionId: "gone" },
          { kind: "help", chapter: "queries", anchor: "console" },
          { kind: "diag", connectionId: "a", tool: "queries", member: null },
          { kind: "table", connectionId: "a", database: "perf", schema: "shop", name: "orders" },
          { kind: "view", connectionId: "a", database: "perf", schema: "shop", name: "v", matview: true },
          { kind: "function", connectionId: "a", database: "perf", oid: 16401, name: "f" },
          { kind: "sequence", connectionId: "a", database: "perf", schema: "shop", name: "s" },
          { kind: "about" },
        ],
        active: "help",
      }),
    );
    restoreWorkspace(new Set(["a"]));
    const s = useWorkspace.getState();
    expect(s.tabs.map((t) => t.id)).toEqual([
      "overview:a",
      "help",
      "diag:a:queries",
      "table:a:perf:shop.orders",
      "view:a:perf:shop.v",
      "function:a:perf:16401",
      "sequence:a:perf:shop.s",
      "about",
    ]);
    expect(s.tabs[0]).toEqual({ kind: "overview", connectionId: "a", member: "pg-1", id: "overview:a" });
    expect(s.tabs[1]).toEqual({ kind: "help", chapter: "queries", anchor: "console", id: "help" });
    expect(s.activeId).toBe("help");
  });

  it("активная вкладка удалённого подключения не восстанавливается — активна первая восстановленная", () => {
    localStorage.setItem(KEY, JSON.stringify({ v: 1, tabs: [{ kind: "about" }, { kind: "explain", connectionId: "gone" }], active: "explain:gone" }));
    restoreWorkspace(new Set());
    expect(useWorkspace.getState()).toMatchObject({ tabs: [{ kind: "about", id: "about" }], activeId: "about" });
  });

  it("уже открытые до восстановления вкладки остаются, активная — их", () => {
    localStorage.setItem(KEY, JSON.stringify({ v: 1, tabs: [{ kind: "about" }, { kind: "settings" }], active: "about" }));
    useWorkspace.getState().open({ kind: "settings", section: "mcp" });
    restoreWorkspace(new Set());
    const s = useWorkspace.getState();
    expect(s.tabs).toEqual([
      { kind: "about", id: "about" },
      { kind: "settings", section: "mcp", id: "settings" },
    ]);
    expect(s.activeId).toBe("settings");
    // Повтор ничего не удваивает.
    restoreWorkspace(new Set());
    expect(useWorkspace.getState().tabs).toHaveLength(2);
  });

  it.each([
    ["не JSON", "{oops"],
    ["другая версия", JSON.stringify({ v: 2, tabs: [{ kind: "about" }], active: "about" })],
    ["без версии", JSON.stringify({ tabs: [{ kind: "about" }], active: "about" })],
    ["вкладки — не массив", JSON.stringify({ v: 1, tabs: { kind: "about" }, active: "about" })],
    ["null", "null"],
  ])("повреждённое значение (%s) — пустая рабочая область, без ошибки", (_name, raw) => {
    localStorage.setItem(KEY, raw);
    expect(() => restoreWorkspace(new Set(["a"]))).not.toThrow();
    expect(useWorkspace.getState()).toMatchObject({ tabs: [], activeId: null });
  });

  it("испорченная запись вкладки пропускается, остальные восстанавливаются", () => {
    localStorage.setItem(
      KEY,
      JSON.stringify({
        v: 1,
        tabs: [
          { kind: "diag", connectionId: "a", tool: "noSuchTool" },
          { kind: "settings", section: "noSuchSection" },
          { kind: "query", connectionId: "a", member: "pg-1", database: "perf" },
          { kind: "function", connectionId: "a", database: "perf", oid: "16401", name: "f" },
          { kind: "connection-form", connectionId: "a" },
          { kind: "whatever" },
          "about",
          { kind: "actions", connectionId: "a", extra: "x" },
        ],
        active: 5,
      }),
    );
    restoreWorkspace(new Set(["a"]));
    expect(useWorkspace.getState()).toEqual(expect.objectContaining({ tabs: [{ kind: "actions", connectionId: "a", id: "actions:a" }], activeId: "actions:a" }));
  });

  it("хранилище недоступно — молча пусто, запись не падает", () => {
    const get = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    const set = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    const stop = persistWorkspace();
    try {
      expect(() => restoreWorkspace(new Set())).not.toThrow();
      expect(() => useWorkspace.getState().open({ kind: "about" })).not.toThrow();
    } finally {
      stop();
      get.mockRestore();
      set.mockRestore();
    }
  });
});

describe("useWorkspacePersistence", () => {
  it("восстанавливает после загрузки списка подключений и только после этого начинает писать", () => {
    localStorage.setItem(KEY, JSON.stringify({ v: 1, tabs: [{ kind: "overview", connectionId: "a" }, { kind: "overview", connectionId: "b" }], active: "overview:b" }));
    const { rerender } = renderHook(({ ids }: { ids: { id: string }[] | undefined }) => useWorkspacePersistence(ids), { initialProps: { ids: undefined as { id: string }[] | undefined } });
    // Список ещё читается: ничего не восстановлено, а открытая вкладка сохранённое не затирает.
    act(() => useWorkspace.getState().open({ kind: "about" }));
    expect(saved().tabs).toHaveLength(2);
    rerender({ ids: [{ id: "a" }] });
    expect(useWorkspace.getState().tabs.map((t) => t.id)).toEqual(["overview:a", "about"]);
    // Сразу после восстановления сохранённое приведено к рабочей области (вкладки удалённого «b» ушли).
    expect(saved()).toEqual({ v: 1, tabs: [{ kind: "overview", connectionId: "a" }, { kind: "about" }], active: "about" });
    act(() => useWorkspace.getState().close("about"));
    expect(saved().tabs).toEqual([{ kind: "overview", connectionId: "a" }]);
    // Новый ответ списка подключений повторно не восстанавливает.
    localStorage.setItem(KEY, JSON.stringify({ v: 1, tabs: [{ kind: "settings" }], active: "settings" }));
    rerender({ ids: [{ id: "a" }, { id: "b" }] });
    expect(useWorkspace.getState().tabs.map((t) => t.id)).toEqual(["overview:a"]);
  });
});
