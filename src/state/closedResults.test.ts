import { beforeEach, describe, expect, it, vi } from "vitest";

const KEY = "glafira.llm.closedResults";

/** Свежий модуль: начальное состояние читается из localStorage при загрузке. */
async function fresh() {
  vi.resetModules();
  return (await import("./closedResults")).useClosedResults;
}

describe("closedResults", () => {
  beforeEach(() => localStorage.clear());

  it("закрытый итог помнится и после перезапуска (localStorage); повтор не дублирует", async () => {
    const store = await fresh();
    expect(store.getState().closed).toEqual([]);
    store.getState().close("r3a9f1c");
    store.getState().close("r3a9f1c");
    store.getState().close("r0a5277");
    expect(store.getState().closed).toEqual(["r3a9f1c", "r0a5277"]);
    expect(JSON.parse(localStorage.getItem(KEY) ?? "null")).toEqual({ v: 1, ids: ["r3a9f1c", "r0a5277"] });
    expect((await fresh()).getState().closed).toEqual(["r3a9f1c", "r0a5277"]);
  });

  it("хранит не больше 200 последних; испорченное значение — как пустое", async () => {
    let store = await fresh();
    for (let i = 0; i < 205; i++) store.getState().close(`r${i}`);
    expect(store.getState().closed).toHaveLength(200);
    expect(store.getState().closed[0]).toBe("r5");
    localStorage.setItem(KEY, "{broken");
    store = await fresh();
    expect(store.getState().closed).toEqual([]);
    localStorage.setItem(KEY, JSON.stringify({ v: 2, ids: ["r1"] }));
    expect((await fresh()).getState().closed).toEqual([]);
  });

  it("хранилище недоступно — закрытие живёт до перезапуска", async () => {
    const store = await fresh();
    const set = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("quota");
    });
    try {
      store.getState().close("r3a9f1c");
      expect(store.getState().closed).toEqual(["r3a9f1c"]);
    } finally {
      set.mockRestore();
    }
  });
});
