import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Opts = { id?: string; text?: string; accelerator?: string; action?: (id: string) => void; item?: unknown; items?: Item[] };
type Item = { kind: string; opts: Opts; closed: boolean };

// Мок меню Tauri: записывает созданные пункты и последнее меню приложения.
const h = vi.hoisted(() => ({
  menus: [] as { items: Item[]; removed: string[]; closed: boolean }[],
  appMenu: null as null | { items: Item[] },
  helpForNSApp: [] as string[],
  /** Хэндлы прежнего меню, которые вернул setAsAppMenu. */
  previous: [] as { closed: boolean }[],
  /** Как в Tauri: обработчики кликов по id пункта; Drop (close) пункта удаляет обработчик его id. */
  channels: new Map<string, (id: string) => void>(),
}));

vi.mock("@tauri-apps/api/menu", () => {
  class MenuItem {
    closed = false;
    async close() {
      this.closed = true;
      if (this.opts.id) h.channels.delete(this.opts.id);
    }
    kind = "item";
    constructor(public opts: Opts) {}
    static async new(opts: Opts) {
      if (opts.id && opts.action) h.channels.set(opts.id, opts.action);
      return new MenuItem(opts);
    }
  }
  class PredefinedMenuItem {
    closed = false;
    async close() {
      this.closed = true;
    }
    kind = "predefined";
    constructor(public opts: Opts) {}
    static async new(opts: Opts) {
      return new PredefinedMenuItem(opts);
    }
  }
  class Submenu {
    closed = false;
    async close() {
      this.closed = true;
    }
    kind = "submenu";
    constructor(public opts: Opts) {}
    static async new(opts: Opts) {
      return new Submenu(opts);
    }
    async setAsHelpMenuForNSApp() {
      h.helpForNSApp.push(this.opts.text ?? "");
    }
  }
  class Menu {
    state = {
      items: [new Submenu({ id: "__tauri_window_menu__", text: "Window" }), new Submenu({ id: "__tauri_help_menu__", text: "Help" })] as Item[],
      removed: [] as string[],
      closed: false,
    };
    static async default() {
      const m = new Menu();
      h.menus.push(m.state);
      return m;
    }
    async get(id: string) {
      return this.state.items.find((i) => i.opts.id === id) ?? null;
    }
    async remove(item: Item) {
      (this.state as { stock?: Item }).stock = item;
      this.state.items = this.state.items.filter((i) => i !== item);
      this.state.removed.push(item.opts.id ?? "");
    }
    async append(item: Item) {
      this.state.items.push(item);
    }
    async setAsAppMenu() {
      h.appMenu = this.state;
      const previous = {
        closed: false,
        async close() {
          previous.closed = true;
        },
      };
      h.previous.push(previous);
      return previous;
    }
    async close() {
      this.state.closed = true;
    }
  }
  return { Menu, Submenu, MenuItem, PredefinedMenuItem };
});
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

import { openUrl } from "@tauri-apps/plugin-opener";
import { i18n } from "../i18n";
import { useDiagnosticsRequest } from "../state/diagnosticsRequest";
import { useWorkspace } from "../state/workspace";
import { SUPPORT_URL } from "./links";
import { useAppMenu } from "./useAppMenu";

const helpSubmenu = () => h.appMenu?.items.find((i) => i.kind === "submenu" && (i.opts.id ?? "").startsWith("glafira-help"));
const labels = () => helpSubmenu()?.opts.items?.map((i) => (i.kind === "predefined" ? "—" : i.opts.text));
/** Клик доставляется, как в Tauri: по id пункта через общую таблицу обработчиков. */
const click = (id: string) => {
  const item = helpSubmenu()?.opts.items?.find((i) => (i.opts.id ?? "").startsWith(`${id}-`));
  const itemId = item?.opts.id ?? "";
  act(() => h.channels.get(itemId)?.(itemId));
};

const realUA = navigator.userAgent;
const setUA = (ua: string) => Object.defineProperty(window.navigator, "userAgent", { value: ua, configurable: true });

beforeEach(() => {
  h.menus = [];
  h.appMenu = null;
  h.helpForNSApp = [];
  h.previous = [];
  h.channels.clear();
  vi.mocked(openUrl).mockClear();
  useWorkspace.setState({ tabs: [], activeId: null });
  useDiagnosticsRequest.setState({ pending: false });
});

afterEach(async () => {
  setUA(realUA);
  await i18n.changeLanguage("ru");
});

describe("useAppMenu", () => {
  it("после смены языка пункты нового меню по-прежнему срабатывают", async () => {
    setUA("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
    renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()?.opts.text).toBe("Справка"));
    await act(() => i18n.changeLanguage("en"));
    await waitFor(() => expect(helpSubmenu()?.opts.text).toBe("Help"));
    await waitFor(() => expect(h.menus[0].closed).toBe(true));
    click("guide");
    expect(useWorkspace.getState().tabs.map((t) => t.kind)).toEqual(["help"]);
  });

  it("на macOS — меню приложения с подменю «Справка» на русском", async () => {
    setUA("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
    renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()).toBeDefined());
    expect(helpSubmenu()?.opts.text).toBe("Справка");
    expect(labels()).toEqual(["Руководство Glafira", "—", "Поддержать проект", "О программе", "Собрать диагностику…"]);
    // Стандартное пустое Help из меню по умолчанию заменено нашим; остальные подменю на месте.
    expect(h.appMenu?.items.some((i) => i.opts.id === "__tauri_help_menu__")).toBe(false);
    expect(h.appMenu?.items.some((i) => i.opts.id === "__tauri_window_menu__")).toBe(true);
    expect(helpSubmenu()?.opts.items?.[0].opts.accelerator).toBe("CmdOrCtrl+Shift+/");
    // Не меню справки NSApp — без системного поиска macOS.
    expect(h.helpForNSApp).toEqual([]);
  });

  it("после смены языка меню перестроено с английскими подписями", async () => {
    setUA("Macintosh");
    renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()?.opts.text).toBe("Справка"));
    await act(() => i18n.changeLanguage("en"));
    await waitFor(() => expect(helpSubmenu()?.opts.text).toBe("Help"));
    expect(labels()).toEqual(["Glafira Guide", "—", "Support the project", "About Glafira", "Collect Diagnostics…"]);
    // Прежнее меню освобождено.
    expect(h.menus[0].closed).toBe(true);
  });

  it("пункты открывают справку, «О программе» и сбор диагностики", async () => {
    setUA("Macintosh");
    renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()).toBeDefined());

    click("guide");
    expect(useWorkspace.getState().tabs).toEqual([{ id: "help", kind: "help" }]);

    click("about");
    expect(useWorkspace.getState().activeId).toBe("about");
    expect(useDiagnosticsRequest.getState().pending).toBe(false);

    // «Поддержать проект» открывает браузер и не создаёт вкладок.
    useWorkspace.setState({ tabs: [], activeId: null });
    click("support");
    expect(openUrl).toHaveBeenCalledWith(SUPPORT_URL);
    expect(useWorkspace.getState().tabs).toEqual([]);

    click("diagnostics");
    expect(useWorkspace.getState().activeId).toBe("about");
    expect(useDiagnosticsRequest.getState().pending).toBe(true);
  });

  it("хэндлы освобождаются: стандартный Help, прежнее меню из setAsAppMenu, пункты заменённого меню", async () => {
    setUA("Macintosh");
    const stockHelp = () => h.menus[0] && (h.menus[0] as unknown as { stock?: Item }).stock;
    renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()?.opts.text).toBe("Справка"));
    expect(h.menus[0].removed).toEqual(["__tauri_help_menu__"]);
    expect(stockHelp()?.closed).toBe(true);
    expect(h.previous.map((p) => p.closed)).toEqual([true]);
    const first = helpSubmenu()!;
    const firstItems = first.opts.items!;
    expect([first, ...firstItems].every((i) => !i.closed)).toBe(true);

    await act(() => i18n.changeLanguage("en"));
    await waitFor(() => expect(helpSubmenu()?.opts.text).toBe("Help"));
    await waitFor(() => expect(first.closed).toBe(true));
    expect(firstItems.every((i) => i.closed)).toBe(true);
    expect(h.previous.map((p) => p.closed)).toEqual([true, true]);
    // Пункты действующего меню не закрыты: иначе Tauri снимет обработчики нажатий.
    const live = helpSubmenu()!;
    expect([live, ...live.opts.items!].every((i) => !i.closed)).toBe(true);
    expect(h.menus[1].closed).toBe(false);
  });

  it("при размонтировании меню и его пункты освобождаются", async () => {
    setUA("Macintosh");
    const { unmount } = renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()).toBeDefined());
    const live = helpSubmenu()!;
    unmount();
    await waitFor(() => expect(h.menus[0].closed).toBe(true));
    expect([live, ...live.opts.items!].every((i) => i.closed)).toBe(true);
  });

  it("вне macOS меню не строится", async () => {
    setUA("Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
    renderHook(() => useAppMenu());
    await act(() => i18n.changeLanguage("en"));
    expect(h.menus).toEqual([]);
    expect(h.appMenu).toBeNull();
  });

  it("после размонтирования смена языка меню не трогает", async () => {
    setUA("Macintosh");
    const { unmount } = renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()).toBeDefined());
    unmount();
    const built = h.menus.length;
    await act(() => i18n.changeLanguage("en"));
    expect(h.menus.length).toBe(built);
  });
});
