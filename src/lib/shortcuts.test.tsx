import { act, render, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({ closedWindows: 0 }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    close: async () => {
      h.closedWindows += 1;
    },
  }),
}));

import { useWorkspace } from "../state/workspace";
import { resolveShortcut, useShortcuts } from "./shortcuts";

const realUA = navigator.userAgent;
const setUA = (ua: string) => Object.defineProperty(window.navigator, "userAgent", { value: ua, configurable: true });
const press = (key: string, init: KeyboardEventInit = {}) => {
  const e = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  act(() => {
    window.dispatchEvent(e);
  });
  return e;
};

beforeEach(() => {
  h.closedWindows = 0;
  useWorkspace.setState({ tabs: [], activeId: null });
});
afterEach(() => setUA(realUA));

describe("resolveShortcut", () => {
  const ev = (key: string, init: KeyboardEventInit = {}) => new KeyboardEvent("keydown", { key, ...init });

  it("на Windows и Linux — Ctrl: N, W, ⇧W, R, F, ⇧F, запятая, Tab, цифры", () => {
    const names = (mac: boolean) =>
      ["n", "w", "r", "f", ",", "1", "9", "Tab", "Enter"].map((k) => (resolveShortcut(ev(k, { ctrlKey: !mac, metaKey: mac }), mac) ? k : null));
    expect(names(false)).toEqual(["n", "w", "r", "f", ",", "1", "9", "Tab", "Enter"]);
    expect(resolveShortcut(ev("w", { ctrlKey: true, shiftKey: true }), false)).not.toBeNull();
    expect(resolveShortcut(ev("f", { ctrlKey: true, shiftKey: true }), false)).not.toBeNull();
    // Без модификатора и с Alt — не наше.
    expect(resolveShortcut(ev("w"), false)).toBeNull();
    expect(resolveShortcut(ev("w", { ctrlKey: true, altKey: true }), false)).toBeNull();
    expect(resolveShortcut(ev("0", { ctrlKey: true }), false)).toBeNull();
  });

  it("на macOS — ⌘ вместо Ctrl, а ⌃Tab переключает вкладки", () => {
    expect(resolveShortcut(ev("w", { metaKey: true }), true)).not.toBeNull();
    expect(resolveShortcut(ev("w", { ctrlKey: true }), true)).toBeNull();
    expect(resolveShortcut(ev("Tab", { ctrlKey: true }), true)).not.toBeNull();
  });
});

describe("useShortcuts", () => {
  it("вне macOS: Ctrl+N открывает форму, Ctrl+1 и Ctrl+Tab переключают, Ctrl+W закрывает, Ctrl+Shift+W закрывает окно", () => {
    setUA("Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
    renderHook(() => useShortcuts());
    expect(press("n", { ctrlKey: true }).defaultPrevented).toBe(true);
    expect(useWorkspace.getState().tabs.map((t) => t.kind)).toEqual(["connection-form"]);
    act(() => useWorkspace.getState().open({ kind: "settings" }));
    expect(useWorkspace.getState().activeId).toBe("settings");
    press("1", { ctrlKey: true });
    expect(useWorkspace.getState().activeId).toBe("connection-form:new");
    press("Tab", { ctrlKey: true });
    expect(useWorkspace.getState().activeId).toBe("settings");
    press("Tab", { ctrlKey: true, shiftKey: true });
    expect(useWorkspace.getState().activeId).toBe("connection-form:new");
    press("w", { ctrlKey: true });
    expect(useWorkspace.getState().tabs.map((t) => t.kind)).toEqual(["settings"]);
    press("w", { ctrlKey: true, shiftKey: true });
    expect(h.closedWindows).toBe(1);
    press(",", { ctrlKey: true });
    expect(useWorkspace.getState().activeId).toBe("settings");
  });

  it("Ctrl+R нажимает «Обновить» видимой вкладки, Ctrl+F ставит фокус в её фильтр, иначе в фильтр дерева", () => {
    setUA("Mozilla/5.0 (X11; Linux x86_64)");
    const refresh = vi.fn();
    render(
      <div>
        <input className="tree-filter" aria-label="дерево" />
        <section className="tab-content" hidden>
          <button type="button" data-action="refresh" onClick={() => refresh("hidden")}>
            скрытая
          </button>
        </section>
        <section className="tab-content">
          <button type="button" data-action="refresh" onClick={() => refresh("visible")}>
            видимая
          </button>
          <input data-action="filter" aria-label="фильтр" />
        </section>
      </div>,
    );
    renderHook(() => useShortcuts());
    press("r", { ctrlKey: true });
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(refresh).toHaveBeenCalledWith("visible");
    press("f", { ctrlKey: true });
    expect(document.activeElement).toHaveAttribute("aria-label", "фильтр");
    press("f", { ctrlKey: true, shiftKey: true });
    expect(document.activeElement).toHaveAttribute("aria-label", "дерево");
  });

  it("Ctrl+Enter нажимает видимую кнопку запуска; кнопки внутри скрытого раздела не считаются", () => {
    setUA("Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
    const run = vi.fn();
    const refresh = vi.fn();
    render(
      <section className="tab-content">
        <div hidden>
          <button type="button" data-action="run" onClick={() => run("hidden")}>
            скрытый режим
          </button>
          <button type="button" data-action="refresh" onClick={() => refresh("hidden")}>
            скрытый раздел
          </button>
        </div>
        <button type="button" data-action="run" onClick={() => run("visible")}>
          План
        </button>
        <button type="button" data-action="refresh" onClick={() => refresh("visible")}>
          Обновить
        </button>
      </section>,
    );
    renderHook(() => useShortcuts());
    expect(press("Enter", { ctrlKey: true }).defaultPrevented).toBe(true);
    expect(run).toHaveBeenCalledTimes(1);
    expect(run).toHaveBeenCalledWith("visible");
    press("r", { ctrlKey: true });
    expect(refresh).toHaveBeenCalledWith("visible");
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("Ctrl+, переключает на открытые «Настройки», не сбрасывая выбранный раздел", () => {
    setUA("Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
    renderHook(() => useShortcuts());
    act(() => {
      useWorkspace.getState().open({ kind: "settings", section: "audit" });
      useWorkspace.getState().open({ kind: "about" });
    });
    press(",", { ctrlKey: true });
    const { tabs, activeId } = useWorkspace.getState();
    expect(activeId).toBe("settings");
    expect(tabs.find((x) => x.id === "settings")).toMatchObject({ kind: "settings", section: "audit" });
  });

  it("на macOS хук не трогает сочетания из меню приложения, но ⌘1…⌘9 обрабатывает", () => {
    setUA("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
    renderHook(() => useShortcuts());
    act(() => {
      useWorkspace.getState().open({ kind: "settings" });
      useWorkspace.getState().open({ kind: "about" });
    });
    // ⌘W в меню — здесь без изменений и без preventDefault.
    expect(press("w", { metaKey: true }).defaultPrevented).toBe(false);
    expect(useWorkspace.getState().tabs.length).toBe(2);
    expect(press("1", { metaKey: true }).defaultPrevented).toBe(true);
    expect(useWorkspace.getState().activeId).toBe("settings");
  });

  it("уже обработанное событие пропускается", () => {
    setUA("Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
    renderHook(() => useShortcuts());
    const e = new KeyboardEvent("keydown", { key: "n", ctrlKey: true, bubbles: true, cancelable: true });
    e.preventDefault();
    act(() => {
      window.dispatchEvent(e);
    });
    expect(useWorkspace.getState().tabs).toEqual([]);
  });
});
