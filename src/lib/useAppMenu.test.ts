import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Opts = { id?: string; text?: string; accelerator?: string; action?: (id: string) => void; item?: unknown; items?: Item[] };
type Item = { kind: string; opts: Opts; closed: boolean };

// Мок меню Tauri: записывает созданные пункты и последнее меню приложения.
const h = vi.hoisted(() => ({
  menus: [] as { items: Item[]; closed: boolean }[],
  appMenu: null as null | { items: Item[] },
  helpForNSApp: [] as string[],
  windowsForNSApp: [] as string[],
  /** Хэндлы прежнего меню, которые вернул setAsAppMenu. */
  previous: [] as { closed: boolean }[],
  /** Как в Tauri: обработчики кликов по id пункта; Drop (close) пункта удаляет обработчик его id. */
  channels: new Map<string, (id: string) => void>(),
  closedWindows: 0,
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
    async setAsWindowsMenuForNSApp() {
      h.windowsForNSApp.push(this.opts.text ?? "");
    }
  }
  class Menu {
    state = { items: [] as Item[], closed: false };
    static async new(opts: Opts) {
      const m = new Menu();
      m.state.items = opts.items ?? [];
      h.menus.push(m.state);
      return m;
    }
    static async default() {
      throw new Error("Menu.default не используется: у его File и Window ⌘W закрывает окно");
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
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    close: async () => {
      h.closedWindows += 1;
    },
  }),
}));

import { openUrl } from "@tauri-apps/plugin-opener";
import { i18n } from "../i18n";
import { SUPPORT_URL } from "./links";
import { useDiagnosticsRequest } from "../state/diagnosticsRequest";
import { useWorkspace } from "../state/workspace";
import { useAppMenu } from "./useAppMenu";

const submenu = (base: string) => h.appMenu?.items.find((i) => i.kind === "submenu" && (i.opts.id ?? "").startsWith(`${base}-`));
const helpSubmenu = () => submenu("glafira-help");
const labels = (s = helpSubmenu()) => s?.opts.items?.map((i) => (i.kind === "predefined" ? "—" : i.opts.text));
/** Клик доставляется, как в Tauri: по id пункта через общую таблицу обработчиков. */
const click = (id: string, s = helpSubmenu()) => {
  const item = s?.opts.items?.find((i) => (i.opts.id ?? "").startsWith(`${id}-`));
  const itemId = item?.opts.id ?? "";
  act(() => h.channels.get(itemId)?.(itemId));
};
const accelerator = (base: string, id: string) => submenu(base)?.opts.items?.find((i) => (i.opts.id ?? "").startsWith(`${id}-`))?.opts.accelerator;
/** Все пункты меню, включая вложенные. */
const allItems = (items: Item[] = h.appMenu?.items ?? []): Item[] => items.flatMap((i) => [i, ...allItems(i.opts.items ?? [])]);

const realUA = navigator.userAgent;
const setUA = (ua: string) => Object.defineProperty(window.navigator, "userAgent", { value: ua, configurable: true });

beforeEach(() => {
  h.menus = [];
  h.appMenu = null;
  h.helpForNSApp = [];
  h.windowsForNSApp = [];
  h.previous = [];
  h.channels.clear();
  h.closedWindows = 0;
  vi.mocked(openUrl).mockClear();
  useWorkspace.setState({ tabs: [], activeId: null });
  useDiagnosticsRequest.setState({ pending: false });
});

afterEach(async () => {
  // Сначала размонтировать хук: иначе смена языка ниже запустит сборку меню, которая закончится уже в следующем тесте.
  cleanup();
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

  it("на macOS — полное меню приложения на русском: Glafira, Файл, Правка, Вид, Окно, Справка", async () => {
    setUA("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
    renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()).toBeDefined());
    expect(h.appMenu?.items.map((i) => i.opts.text)).toEqual(["Glafira", "Файл", "Правка", "Вид", "Окно", "Справка"]);
    expect(labels()).toEqual(["Руководство Glafira", "—", "Поддержать проект", "О программе", "Собрать диагностику…"]);
    expect(labels(submenu("file"))).toEqual(["Новое подключение", "—", "Закрыть вкладку", "Закрыть окно"]);
    expect(labels(submenu("view"))).toEqual(["Обновить", "Запустить", "Найти", "Фильтр подключений", "—", "—"]);
    expect(helpSubmenu()?.opts.items?.[0].opts.accelerator).toBe("CmdOrCtrl+Shift+/");
    // Подменю «Окно» — список окон macOS; меню справки NSApp не назначается (иначе системный поиск).
    expect(h.windowsForNSApp).toEqual(["Окно"]);
    expect(h.helpForNSApp).toEqual([]);
  });

  it("сочетания клавиш (спец. U1 §8): ⌘N, ⌘W, ⇧⌘W, ⌘R, ⌘⏎, ⌘F, ⇧⌘F, ⌃Tab, ⌘,", async () => {
    setUA("Macintosh");
    renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()).toBeDefined());
    expect(accelerator("file", "new-connection")).toBe("CmdOrCtrl+N");
    expect(accelerator("file", "close-tab")).toBe("CmdOrCtrl+W");
    expect(accelerator("file", "close-window")).toBe("CmdOrCtrl+Shift+W");
    expect(accelerator("view", "refresh")).toBe("CmdOrCtrl+R");
    expect(accelerator("view", "run")).toBe("CmdOrCtrl+Enter");
    expect(accelerator("view", "find")).toBe("CmdOrCtrl+F");
    expect(accelerator("view", "filter-connections")).toBe("CmdOrCtrl+Shift+F");
    expect(accelerator("window", "next-tab")).toBe("Ctrl+Tab");
    expect(accelerator("window", "prev-tab")).toBe("Ctrl+Shift+Tab");
    expect(accelerator("app", "settings")).toBe("CmdOrCtrl+Comma");
    // Стандартного «Закрыть окно» с ⌘W нет: ⌘W закрывает вкладку.
    expect(allItems().some((i) => i.kind === "predefined" && i.opts.item === "CloseWindow")).toBe(false);
  });

  it("пункты «Файл», «Вид» и «Окно» действуют: вкладки, настройки, закрытие окна", async () => {
    setUA("Macintosh");
    renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()).toBeDefined());

    click("new-connection", submenu("file"));
    expect(useWorkspace.getState().tabs.map((t) => t.kind)).toEqual(["connection-form"]);
    click("settings", submenu("app"));
    expect(useWorkspace.getState().tabs.map((t) => t.kind)).toEqual(["connection-form", "settings"]);
    expect(useWorkspace.getState().activeId).toBe("settings");
    click("prev-tab", submenu("window"));
    expect(useWorkspace.getState().activeId).toBe("connection-form:new");
    click("next-tab", submenu("window"));
    expect(useWorkspace.getState().activeId).toBe("settings");
    click("close-tab", submenu("file"));
    expect(useWorkspace.getState().tabs.map((t) => t.kind)).toEqual(["connection-form"]);
    click("close-window", submenu("file"));
    expect(h.closedWindows).toBe(1);
  });

  it("после смены языка меню перестроено с английскими подписями", async () => {
    setUA("Macintosh");
    renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()?.opts.text).toBe("Справка"));
    await act(() => i18n.changeLanguage("en"));
    await waitFor(() => expect(helpSubmenu()?.opts.text).toBe("Help"));
    expect(labels()).toEqual(["Glafira Guide", "—", "Support the project", "About Glafira", "Collect Diagnostics…"]);
    expect(h.appMenu?.items.map((i) => i.opts.text)).toEqual(["Glafira", "File", "Edit", "View", "Window", "Help"]);
    // Прежнее меню освобождено.
    expect(h.menus[0].closed).toBe(true);
  });

  it("«Руководство» открывает раздел текущего инструмента диагностики, как F1", async () => {
    setUA("Macintosh");
    renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()).toBeDefined());
    useWorkspace.setState({ tabs: [{ id: "diag:c:locks", kind: "diag", connectionId: "c", tool: "locks" }], activeId: "diag:c:locks" });
    click("guide");
    expect(useWorkspace.getState().tabs.find((t) => t.kind === "help")).toMatchObject({ chapter: "diagnostics", anchor: "locks" });
  });

  it("«Запустить» нажимает главную кнопку запуска видимой вкладки", async () => {
    setUA("Macintosh");
    renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()).toBeDefined());
    const section = document.createElement("section");
    section.className = "tab-content";
    const run = document.createElement("button");
    run.dataset.action = "run";
    const clicked = vi.fn();
    run.addEventListener("click", clicked);
    section.append(run);
    document.body.append(section);
    click("run", submenu("view"));
    expect(clicked).toHaveBeenCalledTimes(1);
    section.remove();
  });

  it("пункты открывают справку, «О программе» и сбор диагностики", async () => {
    setUA("Macintosh");
    renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()).toBeDefined());

    // Без открытых вкладок — глава по умолчанию, как у F1.
    click("guide");
    expect(useWorkspace.getState().tabs).toEqual([{ id: "help", kind: "help", chapter: "getting-started" }]);

    click("about");
    expect(useWorkspace.getState().activeId).toBe("about");
    expect(useDiagnosticsRequest.getState().pending).toBe(false);

    click("support");
    expect(openUrl).toHaveBeenCalledWith(SUPPORT_URL);

    useWorkspace.setState({ tabs: [], activeId: null });
    click("diagnostics");
    expect(useWorkspace.getState().activeId).toBe("about");
    expect(useDiagnosticsRequest.getState().pending).toBe(true);
  });

  it("хэндлы освобождаются: прежнее меню из setAsAppMenu, пункты заменённого меню", async () => {
    setUA("Macintosh");
    renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()?.opts.text).toBe("Справка"));
    expect(h.previous.map((p) => p.closed)).toEqual([true]);
    const firstItems = allItems();
    expect(firstItems.length).toBeGreaterThan(20);
    expect(firstItems.every((i) => !i.closed)).toBe(true);

    await act(() => i18n.changeLanguage("en"));
    await waitFor(() => expect(helpSubmenu()?.opts.text).toBe("Help"));
    await waitFor(() => expect(firstItems.every((i) => i.closed)).toBe(true));
    expect(h.previous.map((p) => p.closed)).toEqual([true, true]);
    // Пункты действующего меню не закрыты: иначе Tauri снимет обработчики нажатий.
    expect(allItems().every((i) => !i.closed)).toBe(true);
    expect(h.menus[1].closed).toBe(false);
  });

  it("при размонтировании меню и его пункты освобождаются", async () => {
    setUA("Macintosh");
    const { unmount } = renderHook(() => useAppMenu());
    await waitFor(() => expect(helpSubmenu()).toBeDefined());
    const live = allItems();
    unmount();
    await waitFor(() => expect(h.menus[0].closed).toBe(true));
    expect(live.every((i) => i.closed)).toBe(true);
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
