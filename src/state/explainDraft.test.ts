import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DRAFT_SAVE_DELAY_MS, flushExplainDraft, forgetExplainDraft, readExplainDraft, saveExplainDraft } from "./explainDraft";

const key = (id: string) => `glafira.explain.draft:${id}`;
const stored = (id: string) => JSON.parse(localStorage.getItem(key(id)) ?? "null");

beforeEach(() => {
  localStorage.clear();
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("черновик консоли EXPLAIN (смоук P4c, R18)", () => {
  it("пишется с задержкой ~500 мс: только текст, база и узел; последняя правка выигрывает", () => {
    saveExplainDraft("d1", { sql: "SELECT 1", database: "perf", member: "pg-2" });
    vi.advanceTimersByTime(DRAFT_SAVE_DELAY_MS - 1);
    expect(localStorage.getItem(key("d1"))).toBeNull();
    saveExplainDraft("d1", { sql: "SELECT 12", database: "perf", member: "pg-2" });
    vi.advanceTimersByTime(DRAFT_SAVE_DELAY_MS - 1);
    expect(localStorage.getItem(key("d1"))).toBeNull();
    vi.advanceTimersByTime(1);
    expect(stored("d1")).toEqual({ v: 1, sql: "SELECT 12", database: "perf", member: "pg-2" });
    expect(readExplainDraft("d1")).toEqual({ sql: "SELECT 12", database: "perf", member: "pg-2" });
    expect(DRAFT_SAVE_DELAY_MS).toBe(500);
  });

  it("flush пишет сразу; пустой черновик (нет текста, базы, узла) удаляется", () => {
    saveExplainDraft("d2", { sql: "SELECT 2", database: null, member: null });
    flushExplainDraft("d2");
    expect(stored("d2")).toEqual({ v: 1, sql: "SELECT 2", database: null, member: null });
    saveExplainDraft("d2", { sql: "", database: null, member: null });
    flushExplainDraft("d2");
    expect(localStorage.getItem(key("d2"))).toBeNull();
    // Без ожидающей записи flush ничего не делает.
    flushExplainDraft("d2");
    expect(localStorage.getItem(key("d2"))).toBeNull();
  });

  it("у каждого подключения свой черновик", () => {
    saveExplainDraft("d3", { sql: "SELECT 3", database: null, member: null });
    saveExplainDraft("d4", { sql: "SELECT 4", database: null, member: null });
    vi.advanceTimersByTime(DRAFT_SAVE_DELAY_MS);
    expect(readExplainDraft("d3")?.sql).toBe("SELECT 3");
    expect(readExplainDraft("d4")?.sql).toBe("SELECT 4");
  });

  it("удаление подключения стирает черновик и отменяет ожидающую запись; поздняя запись его не возвращает", () => {
    saveExplainDraft("d5", { sql: "SELECT 5", database: null, member: null });
    flushExplainDraft("d5");
    saveExplainDraft("d5", { sql: "SELECT 55", database: null, member: null });
    forgetExplainDraft("d5");
    expect(localStorage.getItem(key("d5"))).toBeNull();
    vi.advanceTimersByTime(DRAFT_SAVE_DELAY_MS);
    expect(localStorage.getItem(key("d5"))).toBeNull();
    // Консоль удалённого подключения закрывается после стирания: её последняя запись пропадает.
    saveExplainDraft("d5", { sql: "SELECT 555", database: null, member: null });
    flushExplainDraft("d5");
    vi.advanceTimersByTime(DRAFT_SAVE_DELAY_MS);
    expect(localStorage.getItem(key("d5"))).toBeNull();
  });

  it.each([
    ["не JSON", "{oops"],
    ["другая версия", JSON.stringify({ v: 2, sql: "x", database: null, member: null })],
    ["текст не строка", JSON.stringify({ v: 1, sql: 1, database: null, member: null })],
    ["база не строка", JSON.stringify({ v: 1, sql: "x", database: 5, member: null })],
    ["null", "null"],
  ])("повреждённое значение (%s) — черновика нет, без ошибки", (_name, raw) => {
    localStorage.setItem(key("d6"), raw);
    expect(readExplainDraft("d6")).toBeNull();
  });

  it("недоступное хранилище — черновика нет, запись не падает", () => {
    const get = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    const set = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    try {
      expect(readExplainDraft("d7")).toBeNull();
      saveExplainDraft("d7", { sql: "x", database: null, member: null });
      expect(() => flushExplainDraft("d7")).not.toThrow();
    } finally {
      get.mockRestore();
      set.mockRestore();
    }
  });

  it("закрытие окна (pagehide) дописывает ожидающие черновики", () => {
    saveExplainDraft("d8", { sql: "SELECT 8", database: null, member: null });
    window.dispatchEvent(new Event("pagehide"));
    expect(readExplainDraft("d8")?.sql).toBe("SELECT 8");
  });
});
