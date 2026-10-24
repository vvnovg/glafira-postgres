import { beforeEach, describe, expect, it } from "vitest";
import { tabId, useWorkspace } from "./workspace";

beforeEach(() => {
  useWorkspace.setState({ tabs: [], activeId: null });
});

describe("workspace", () => {
  it("повторное открытие активирует существующую вкладку", () => {
    const { open } = useWorkspace.getState();
    open({ kind: "settings" });
    open({ kind: "connection-form", connectionId: "c" });
    open({ kind: "settings" });
    const s = useWorkspace.getState();
    expect(s.tabs).toHaveLength(2);
    expect(s.activeId).toBe("settings");
  });

  it("закрытие активной вкладки активирует соседнюю", () => {
    const { open, close, activate } = useWorkspace.getState();
    open({ kind: "connection-form", connectionId: "a" });
    open({ kind: "connection-form", connectionId: "b" });
    open({ kind: "connection-form", connectionId: "c" });
    activate("connection-form:b");
    close("connection-form:b");
    expect(useWorkspace.getState().activeId).toBe("connection-form:c");
    close("connection-form:c");
    expect(useWorkspace.getState().activeId).toBe("connection-form:a");
    close("connection-form:a");
    expect(useWorkspace.getState().activeId).toBeNull();
  });

  it("id вкладок", () => {
    expect(tabId({ kind: "connection-form", connectionId: null })).toBe("connection-form:new");
    expect(tabId({ kind: "connection-form", connectionId: "c" })).toBe("connection-form:c");
    // Копия — своя вкладка на каждое исходное подключение, не занимает вкладку «Новое подключение».
    expect(tabId({ kind: "connection-form", connectionId: null, copyOf: "c" })).toBe("connection-form:copy:c");
    expect(tabId({ kind: "settings" })).toBe("settings");
    expect(tabId({ kind: "about" })).toBe("about");
    expect(tabId({ kind: "help", chapter: "getting-started" })).toBe("help");
    expect(tabId({ kind: "table", connectionId: "c", database: "orders", schema: "shop", name: "events" })).toBe("table:c:orders:shop.events");
    expect(tabId({ kind: "view", connectionId: "c", database: "orders", schema: "shop", name: "v", matview: true })).toBe("view:c:orders:shop.v");
    expect(tabId({ kind: "function", connectionId: "c", database: "orders", oid: 16401, name: "order_count" })).toBe("function:c:orders:16401");
    expect(tabId({ kind: "sequence", connectionId: "c", database: "orders", schema: "shop", name: "s" })).toBe("sequence:c:orders:shop.s");
    // Инструмент диагностики — одна вкладка на подключение и инструмент; узел в id не входит.
    expect(tabId({ kind: "diag", connectionId: "c", tool: "pulse" })).toBe("diag:c:pulse");
    expect(tabId({ kind: "diag", connectionId: "c", tool: "idleTx", member: "pg-2" })).toBe("diag:c:idleTx");
  });

  it("closeForConnection закрывает все вкладки подключения", () => {
    const { open, activate, closeForConnection } = useWorkspace.getState();
    open({ kind: "connection-form", connectionId: "a" });
    open({ kind: "settings" });
    open({ kind: "connection-form", connectionId: "b" });
    open({ kind: "connection-form", connectionId: "c" });
    activate("connection-form:b");
    closeForConnection("b");
    let s = useWorkspace.getState();
    expect(s.tabs.map((t) => t.id)).toEqual(["connection-form:a", "settings", "connection-form:c"]);
    expect(s.activeId).toBe("connection-form:c");
    activate("connection-form:a");
    closeForConnection("c");
    s = useWorkspace.getState();
    expect(s.activeId).toBe("connection-form:a");
    closeForConnection("a");
    expect(useWorkspace.getState().tabs.map((t) => t.id)).toEqual(["settings"]);
    expect(useWorkspace.getState().activeId).toBe("settings");
  });

  it("вкладки объектов закрываются вместе с подключением, а повторное открытие объекта активирует ту же вкладку", () => {
    const { open, closeForConnection } = useWorkspace.getState();
    open({ kind: "settings" });
    open({ kind: "table", connectionId: "a", database: "orders", schema: "shop", name: "events" });
    open({ kind: "function", connectionId: "a", database: "orders", oid: 7, name: "f" });
    open({ kind: "table", connectionId: "b", database: "orders", schema: "shop", name: "events" });
    open({ kind: "table", connectionId: "a", database: "orders", schema: "shop", name: "events" });
    expect(useWorkspace.getState().tabs).toHaveLength(4);
    expect(useWorkspace.getState().activeId).toBe("table:a:orders:shop.events");
    closeForConnection("a");
    expect(useWorkspace.getState().tabs.map((t) => t.id)).toEqual(["settings", "table:b:orders:shop.events"]);
  });

  it("повторное open обновляет spec открытой вкладки", () => {
    const { open } = useWorkspace.getState();
    open({ kind: "help" });
    open({ kind: "settings" });
    open({ kind: "help", chapter: "getting-started", anchor: "passwords" });
    const s = useWorkspace.getState();
    expect(s.tabs).toHaveLength(2);
    expect(s.activeId).toBe("help");
    expect(s.tabs[0]).toMatchObject({ kind: "help", chapter: "getting-started", anchor: "passwords" });
  });

  it("вкладка обзора одна на подключение и получает выбранный узел", () => {
    useWorkspace.setState({ tabs: [], activeId: null });
    useWorkspace.getState().open({ kind: "overview", connectionId: "c" });
    useWorkspace.getState().open({ kind: "overview", connectionId: "c", member: "pg-2" });
    const { tabs, activeId } = useWorkspace.getState();
    expect(tabs).toHaveLength(1);
    expect(activeId).toBe("overview:c");
    expect(tabs[0]).toMatchObject({ member: "pg-2" });
  });

  it("closeForConnection закрывает и вкладку обзора", () => {
    const { open, closeForConnection } = useWorkspace.getState();
    open({ kind: "overview", connectionId: "a" });
    open({ kind: "overview", connectionId: "b" });
    closeForConnection("a");
    expect(useWorkspace.getState().tabs.map((t) => t.id)).toEqual(["overview:b"]);
  });

  it("вкладка инструмента одна на подключение и инструмент: другой узел меняет member у существующей", () => {
    const { open } = useWorkspace.getState();
    open({ kind: "diag", connectionId: "c", tool: "locks" });
    open({ kind: "settings" });
    open({ kind: "diag", connectionId: "c", tool: "locks", member: "pg-2" });
    const { tabs, activeId } = useWorkspace.getState();
    expect(tabs.map((t) => t.id)).toEqual(["diag:c:locks", "settings"]);
    expect(activeId).toBe("diag:c:locks");
    expect(tabs[0]).toMatchObject({ kind: "diag", tool: "locks", member: "pg-2" });
  });

  it("closeForConnection закрывает и вкладки инструментов", () => {
    const { open, closeForConnection } = useWorkspace.getState();
    open({ kind: "diag", connectionId: "a", tool: "pulse" });
    open({ kind: "diag", connectionId: "b", tool: "pulse" });
    closeForConnection("a");
    expect(useWorkspace.getState().tabs.map((t) => t.id)).toEqual(["diag:b:pulse"]);
  });
});
