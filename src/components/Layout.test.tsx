import { act, fireEvent, screen, within } from "@testing-library/react";
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

  it("форма подключения: в заголовке имя, а не id; без подключения — прочерк-заглушка", async () => {
    useWorkspace.setState({
      tabs: [
        { kind: "connection-form", connectionId: "c1", id: "connection-form:c1" },
        { kind: "connection-form", connectionId: "gone", id: "connection-form:gone" },
        { kind: "connection-form", connectionId: null, id: "connection-form:new" },
      ],
      activeId: "connection-form:new",
    });
    const { container } = renderWithClient(<Layout />);
    const list = screen.getByRole("tablist");
    expect(await within(list).findByRole("tab", { name: "Подключение: prod-1" })).toBeInTheDocument();
    expect(within(list).getByRole("tab", { name: "Подключение: ?" })).toBeInTheDocument();
    expect(within(list).getByRole("tab", { name: "Новое подключение" })).toBeInTheDocument();
    // Активна вкладка без подключения: полоса прозрачна.
    expect(container.querySelector<HTMLElement>(".env-stripe")!.style.background).toBe("transparent");
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

  it("opens the guide from the sidebar", () => {
    renderWithClient(<Layout />);
    fireEvent.click(screen.getByRole("button", { name: "Справка" }));
    expect(useWorkspace.getState().activeId).toBe("help");
  });
});
