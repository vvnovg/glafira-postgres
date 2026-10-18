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
    expect(tabId({ kind: "settings" })).toBe("settings");
    expect(tabId({ kind: "license" })).toBe("license");
    expect(tabId({ kind: "about" })).toBe("about");
    expect(tabId({ kind: "help", chapter: "getting-started" })).toBe("help");
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
});
