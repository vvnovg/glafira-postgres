import { act, fireEvent, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useHelpContext } from "../state/helpContext";
import { useWorkspace } from "../state/workspace";
import { renderWithClient } from "../test/render";
import { Layout } from "./Layout";

vi.mock("./TabContent", () => ({ TabContent: () => <div /> }));

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
