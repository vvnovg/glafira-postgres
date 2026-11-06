import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api/client";
import { mcpStatus } from "../test/mcp";
import { ENV_COLORS } from "../lib/env";
import type { ConnectionProfile } from "../api/types";
import { useHelpContext } from "../state/helpContext";
import { useTabMarks } from "../state/tabMarks";
import { useWorkspace } from "../state/workspace";
import { renderWithClient } from "../test/render";
import { Layout } from "./Layout";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  const conn = {
    id: "c1",
    name: "prod-1",
    color: "#888888",
    environment: "prod",
    readOnly: true,
    kind: "server",
    hosts: [{ host: "localhost", port: 5432 }],
    targetServerType: "primary",
    patroni: null,
    pg: { username: "u", database: "postgres", sslMode: "prefer", tls: { caPath: null, clientCert: null }, connectTimeoutS: 10, databases: null },
    ssh: null,
  } as ConnectionProfile;
  return { ...actual, api: { ...actual.api, getMcpStatus: vi.fn(() => new Promise(() => {})), listConnections: vi.fn().mockResolvedValue([conn]) } };
});
vi.mock("./TabContent", () => ({ TabContent: () => <div /> }));

/** jsdom приводит цвет к rgb(...): сравниваем в одном виде. */
const hex = (c: string) => {
  const el = document.createElement("div");
  el.style.background = c;
  return el.style.background;
};

describe("Layout tabs", () => {
  beforeEach(() => {
    useTabMarks.setState({ marks: {} });
    useHelpContext.setState({ byTab: {} });
    useWorkspace.setState({
      tabs: [
        { kind: "settings", id: "settings" },
        { kind: "about", id: "about" },
      ],
      activeId: "settings",
    });
  });

  it("диалог подтверждения MCP-клиента — поверх любой вкладки; подписка на mcp://state", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValueOnce(mcpStatus({ pending: [{ sessionId: "s1", clientName: "qwen-code", clientVersion: "0.26.7" }] }));
    renderWithClient(<Layout />);
    expect(await screen.findByRole("dialog", { name: "qwen-code 0.26.7 хочет подключиться к Glafira" })).toBeInTheDocument();
    expect(listen).toHaveBeenCalledWith("mcp://state", expect.any(Function));
  });

  it("shows tab titles with accessible tab roles", () => {
    renderWithClient(<Layout />);
    const list = screen.getByRole("tablist");
    const tabs = within(list).getAllByRole("tab");
    expect(tabs.map((t) => t.textContent)).toEqual(["Настройки", "О программе"]);
    expect(tabs[0].getAttribute("aria-selected")).toBe("true");
    expect(tabs[1].getAttribute("aria-selected")).toBe("false");
  });

  it("обзор подключения: заголовок с именем, значок окружения и замок во вкладке, полоса окружения", async () => {
    useWorkspace.setState({
      tabs: [
        { kind: "settings", id: "settings" },
        { kind: "overview", connectionId: "c1", id: "overview:c1" },
      ],
      activeId: "overview:c1",
    });
    const { container } = renderWithClient(<Layout />);
    const list = screen.getByRole("tablist");
    expect(await within(list).findByRole("tab", { name: "Обзор: prod-1" })).toBeInTheDocument();
    expect(within(list).getAllByText("PROD")).toHaveLength(1);
    expect(within(list).getAllByRole("img", { name: "Только чтение" })).toHaveLength(1);
    // Вкладка без подключения (настройки) значка не получает.
    expect(within(within(list).getByRole("tab", { name: "Настройки" }).parentElement!).queryByText("PROD")).toBeNull();
    const stripe = container.querySelector<HTMLElement>(".env-stripe")!;
    expect(stripe.style.background).toBe(hex(ENV_COLORS.prod));
  });

  it("вкладка инструмента диагностики: заголовок «инструмент — подключение», значок окружения", async () => {
    useWorkspace.setState({
      tabs: [
        { kind: "diag", connectionId: "c1", tool: "idleTx", id: "diag:c1:idleTx" },
        { kind: "diag", connectionId: "c1", tool: "locks", member: "pg-2", id: "diag:c1:locks" },
      ],
      activeId: "diag:c1:idleTx",
    });
    renderWithClient(<Layout />);
    const list = screen.getByRole("tablist");
    expect(await within(list).findByRole("tab", { name: "Idle in transaction — prod-1" })).toBeInTheDocument();
    expect(within(list).getByRole("tab", { name: "Блокировки — prod-1" })).toBeInTheDocument();
    expect(within(list).getAllByText("PROD")).toHaveLength(2);
  });

  it("вкладка журнала действий: заголовок «Журнал действий — подключение», значок окружения; F1 открывает якорь journal главы «Диагностика»", async () => {
    useWorkspace.setState({ tabs: [{ kind: "actions", connectionId: "c1", id: "actions:c1" }], activeId: "actions:c1" });
    renderWithClient(<Layout />);
    const list = screen.getByRole("tablist");
    expect(await within(list).findByRole("tab", { name: "Журнал действий — prod-1" })).toBeInTheDocument();
    expect(within(list).getAllByText("PROD")).toHaveLength(1);
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "F1", bubbles: true, cancelable: true }));
    });
    expect(useWorkspace.getState().tabs.find((t) => t.id === "help")).toMatchObject({ kind: "help", chapter: "diagnostics", anchor: "journal" });
  });

  it("карточка запроса — начало текста (без текста queryId) и подключение; консоль EXPLAIN — подключение", async () => {
    useWorkspace.setState({
      tabs: [
        { kind: "query", connectionId: "c1", member: "pg-1", database: "perf", queryId: "1", title: "select * from orders", id: "query:c1:pg-1:perf:1" },
        { kind: "query", connectionId: "c1", member: "pg-1", database: "perf", queryId: "2", id: "query:c1:pg-1:perf:2" },
        { kind: "explain", connectionId: "c1", id: "explain:c1" },
      ],
      activeId: "explain:c1",
    });
    renderWithClient(<Layout />);
    const list = screen.getByRole("tablist");
    expect(await within(list).findByRole("tab", { name: "select * from orders — prod-1" })).toBeInTheDocument();
    expect(within(list).getByRole("tab", { name: "2 — prod-1" })).toBeInTheDocument();
    expect(within(list).getByRole("tab", { name: "EXPLAIN — prod-1" })).toBeInTheDocument();
  });

  it("форма подключения: в заголовке имя, а не id; без подключения — прочерк-заглушка", async () => {
    useWorkspace.setState({
      tabs: [
        { kind: "connection-form", connectionId: "c1", id: "connection-form:c1" },
        { kind: "connection-form", connectionId: "gone", id: "connection-form:gone" },
        { kind: "connection-form", connectionId: null, id: "connection-form:new" },
        { kind: "connection-form", connectionId: null, copyOf: "c1", id: "connection-form:copy:c1" },
      ],
      activeId: "connection-form:new",
    });
    const { container } = renderWithClient(<Layout />);
    const list = screen.getByRole("tablist");
    expect(await within(list).findByRole("tab", { name: "Подключение: prod-1" })).toBeInTheDocument();
    expect(within(list).getByRole("tab", { name: "Подключение: ?" })).toBeInTheDocument();
    expect(within(list).getByRole("tab", { name: "Новое подключение" })).toBeInTheDocument();
    expect(within(list).getByRole("tab", { name: "Копия: prod-1" })).toBeInTheDocument();
    // Активна вкладка без подключения: полоса прозрачна.
    expect(container.querySelector<HTMLElement>(".env-stripe")!.style.background).toBe("transparent");
  });

  it("вкладки объектов: таблица, представление и последовательность — схема.имя, функция — имя; у всех значок окружения", async () => {
    useWorkspace.setState({
      tabs: [
        { kind: "table", connectionId: "c1", database: "orders", schema: "shop", name: "orders", id: "table:c1:orders:shop.orders" },
        { kind: "view", connectionId: "c1", database: "orders", schema: "shop", name: "v_orders", matview: false, id: "view:c1:orders:shop.v_orders" },
        { kind: "function", connectionId: "c1", database: "orders", oid: 16401, name: "order_count", id: "function:c1:orders:16401" },
        { kind: "sequence", connectionId: "c1", database: "orders", schema: "shop", name: "orders_id_seq", id: "sequence:c1:orders:shop.orders_id_seq" },
      ],
      activeId: "function:c1:orders:16401",
    });
    renderWithClient(<Layout />);
    const list = screen.getByRole("tablist");
    expect(await within(list).findByRole("tab", { name: "shop.orders" })).toBeInTheDocument();
    expect(within(list).getByRole("tab", { name: "shop.v_orders" })).toBeInTheDocument();
    expect(within(list).getByRole("tab", { name: "order_count" })).toBeInTheDocument();
    expect(within(list).getByRole("tab", { name: "shop.orders_id_seq" })).toBeInTheDocument();
    // Значки приходят вместе со списком подключений, позже заголовков.
    await waitFor(() => expect(within(list).getAllByText("PROD")).toHaveLength(4));
  });

  it("точка у вкладки консоли — в имени самой вкладки, пока у неё есть непоказанное предложение LLM", () => {
    useWorkspace.setState({ tabs: [{ kind: "explain", connectionId: "c1", id: "explain:c1" }, { kind: "settings", id: "settings" }], activeId: "settings" });
    useTabMarks.setState({ marks: { "explain:c1": true } });
    renderWithClient(<Layout />);
    expect(screen.getByRole("img", { name: "Новое предложение LLM" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /Новое предложение LLM/ })).toBeInTheDocument();
    act(() => useTabMarks.getState().setMark("explain:c1", false));
    expect(screen.queryByRole("img", { name: "Новое предложение LLM" })).toBeNull();
  });

  it("подписывается на события наблюдения", () => {
    renderWithClient(<Layout />);
    expect(vi.mocked(listen).mock.calls.map((c) => c[0])).toEqual(expect.arrayContaining(["connection://state", "topology://changed"]));
  });

  it("activates a tab via its button", () => {
    renderWithClient(<Layout />);
    const tabs = within(screen.getByRole("tablist")).getAllByRole("tab");
    fireEvent.click(tabs[1]);
    expect(tabs[1].getAttribute("aria-selected")).toBe("true");
    expect(tabs[0].getAttribute("aria-selected")).toBe("false");
  });

  it("closes a tab with its close button", () => {
    renderWithClient(<Layout />);
    fireEvent.click(screen.getAllByRole("button", { name: "Закрыть вкладку" })[0]);
    expect(useWorkspace.getState().tabs.map((t) => t.id)).toEqual(["about"]);
    expect(useWorkspace.getState().activeId).toBe("about");
  });

  it("F1 opens the guide chapter of the active screen", () => {
    renderWithClient(<Layout />);
    const ev = new KeyboardEvent("keydown", { key: "F1", bubbles: true, cancelable: true });
    act(() => {
      window.dispatchEvent(ev);
    });
    expect(ev.defaultPrevented).toBe(true);
    const { tabs, activeId } = useWorkspace.getState();
    expect(activeId).toBe("help");
    expect(tabs.find((t) => t.id === "help")).toMatchObject({ kind: "help", chapter: "getting-started" });
    expect(within(screen.getByRole("tablist")).getByRole("tab", { name: "Справка" })).toBeInTheDocument();
  });

  it("F1 на вкладке инструмента диагностики открывает его раздел главы «Диагностика»", () => {
    useWorkspace.setState({
      tabs: [
        { kind: "diag", connectionId: "c1", tool: "locks", id: "diag:c1:locks" },
        { kind: "diag", connectionId: "c1", tool: "idleTx", id: "diag:c1:idleTx" },
      ],
      activeId: "diag:c1:locks",
    });
    renderWithClient(<Layout />);
    const press = () =>
      act(() => {
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "F1", bubbles: true, cancelable: true }));
      });
    press();
    expect(useWorkspace.getState().tabs.find((t) => t.id === "help")).toMatchObject({ kind: "help", chapter: "diagnostics", anchor: "locks" });
    // «Idle in transaction» описан в «Сессиях».
    act(() => useWorkspace.getState().activate("diag:c1:idleTx"));
    press();
    expect(useWorkspace.getState().tabs.find((t) => t.id === "help")).toMatchObject({ chapter: "diagnostics", anchor: "sessions" });
    // На самой справке F1 только возвращает её на экран: открытая глава не сбрасывается.
    act(() => useWorkspace.getState().activate("diag:c1:locks"));
    act(() => useWorkspace.getState().activate("help"));
    press();
    expect(useWorkspace.getState().activeId).toBe("help");
    expect(useWorkspace.getState().tabs.find((t) => t.id === "help")).toMatchObject({ chapter: "diagnostics", anchor: "sessions" });
  });

  it("F1: глава, заданная экраном, главнее таблицы, и якорь не передаётся", () => {
    useWorkspace.setState({ tabs: [{ kind: "diag", connectionId: "c1", tool: "locks", id: "diag:c1:locks" }], activeId: "diag:c1:locks" });
    useHelpContext.setState({ byTab: { "diag:c1:locks": "updates" } });
    renderWithClient(<Layout />);
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "F1", bubbles: true, cancelable: true }));
    });
    const help = useWorkspace.getState().tabs.find((t) => t.id === "help");
    expect(help).toMatchObject({ kind: "help", chapter: "updates" });
    expect(help && "anchor" in help ? help.anchor : undefined).toBeUndefined();
  });

  it("opens the guide from the sidebar", () => {
    renderWithClient(<Layout />);
    fireEvent.click(screen.getByRole("button", { name: "Справка" }));
    expect(useWorkspace.getState().activeId).toBe("help");
  });
});

describe("Layout: переполнение строки вкладок", () => {
  /** Строка 0…250 px, вкладки по 100 px подряд: третья и дальше не видны целиком. */
  let rects: { mockRestore: () => void } | null = null;
  const layoutRects = () =>
    (rects = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const rect = (left: number, width: number) => ({ left, right: left + width, width, top: 0, bottom: 20, height: 20, x: left, y: 0, toJSON: () => ({}) }) as DOMRect;
      if (this.getAttribute("role") === "tablist") return rect(0, 250);
      const id = this.dataset.tabId;
      if (id) return rect(useWorkspace.getState().tabs.findIndex((t) => t.id === id) * 100, 100);
      return rect(0, 0);
    }));

  beforeEach(() => {
    useHelpContext.setState({ byTab: {} });
    useWorkspace.setState({
      tabs: [
        { kind: "settings", id: "settings" },
        { kind: "about", id: "about" },
        { kind: "help", id: "help", chapter: "getting-started" },
      ],
      activeId: "settings",
    });
  });

  afterEach(() => {
    rects?.mockRestore();
    rects = null;
    delete (Element.prototype as { scrollIntoView?: unknown }).scrollIntoView;
  });

  /** Прокрутка строки вкладок: смещение учитывается в положении вкладок, записи scrollLeft копятся. */
  const scrollableStrip = () => {
    const writes: number[] = [];
    let scrollLeft = 0;
    rects = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const rect = (left: number, width: number) => ({ left, right: left + width, width, top: 0, bottom: 20, height: 20, x: left, y: 0, toJSON: () => ({}) }) as DOMRect;
      if (this.getAttribute("role") === "tablist") {
        if (!Object.getOwnPropertyDescriptor(this, "scrollLeft")) {
          Object.defineProperty(this, "scrollLeft", {
            configurable: true,
            get: () => scrollLeft,
            set: (v: number) => {
              scrollLeft = v;
              writes.push(v);
            },
          });
        }
        return rect(0, 250);
      }
      const id = this.dataset.tabId;
      if (id) return rect(useWorkspace.getState().tabs.findIndex((t) => t.id === id) * 100 - scrollLeft, 100);
      return rect(0, 0);
    });
    return { writes, get scrollLeft() { return scrollLeft; } };
  };

  it("активная вкладка прокручивается в видимую часть: при выборе, при открытии новой и при изменении размера окна", () => {
    const strip = scrollableStrip();
    renderWithClient(<Layout />);
    act(() => useWorkspace.getState().activate("help"));
    // Третья вкладка (200…300) в строке 0…250: прокрутка на 50 + отступ 8.
    expect(strip.scrollLeft).toBe(58);
    act(() => useWorkspace.getState().activate("settings"));
    expect(strip.scrollLeft).toBe(0);
    act(() => useWorkspace.getState().open({ kind: "about" }));
    expect(strip.scrollLeft).toBe(0 + 0);
    act(() => useWorkspace.setState({ tabs: [...useWorkspace.getState().tabs, { kind: "settings", id: "s2" } as never], activeId: "s2" }));
    expect(strip.scrollLeft).toBe(300 + 100 - 250 + 8);
    // Пользователь прокрутил строку к началу; окно сузили — активная снова в видимой части.
    strip.writes.length = 0;
    const nav = screen.getByRole("tablist");
    nav.scrollLeft = 0;
    act(() => {
      window.dispatchEvent(new Event("resize"));
    });
    expect(strip.scrollLeft).toBe(158);
  });

  it("скрытые вкладки — в меню за кнопкой у строки; выбор пункта делает вкладку активной, Esc закрывает меню", async () => {
    layoutRects();
    const user = userEvent.setup();
    renderWithClient(<Layout />);
    const more = await screen.findByRole("button", { name: "Скрытые вкладки: 1" });
    expect(more).toHaveAttribute("aria-haspopup", "menu");
    await user.click(more);
    const menu = screen.getByRole("menu", { name: "Скрытые вкладки: 1" });
    expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual(["Справка"]);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(more).toHaveFocus();
    await user.keyboard("{Enter}");
    await user.click(within(screen.getByRole("menu")).getByRole("menuitem", { name: "Справка" }));
    expect(useWorkspace.getState().activeId).toBe("help");
  });

  it("все вкладки видны — кнопки меню нет", () => {
    renderWithClient(<Layout />);
    expect(screen.queryByRole("button", { name: /Скрытые вкладки/ })).toBeNull();
  });
});
