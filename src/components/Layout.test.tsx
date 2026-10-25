import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { ENV_COLORS } from "../lib/env";
import type { ConnectionProfile } from "../api/types";
import { useHelpContext } from "../state/helpContext";
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
  return { ...actual, api: { ...actual.api, listConnections: vi.fn().mockResolvedValue([conn]) } };
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
    useHelpContext.setState({ byTab: {} });
    useWorkspace.setState({
      tabs: [
        { kind: "settings", id: "settings" },
        { kind: "about", id: "about" },
      ],
      activeId: "settings",
    });
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
