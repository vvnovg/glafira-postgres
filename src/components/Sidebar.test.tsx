import { act, fireEvent, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { newProfile } from "./connections/profile";
import { renderWithClient } from "../test/render";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return {
    ...orig,
    api: {
      getMcpStatus: vi.fn(() => new Promise(() => {})),
      getTopology: vi.fn(() => new Promise(() => {})),
      listFolders: vi.fn(() => Promise.resolve([])),
      listConnections: vi.fn(() => Promise.resolve([{ ...newProfile(), id: "c", name: "billing" }])),
    },
  };
});

import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "../api/client";
import { mcpStatus } from "../test/mcp";
import { SUPPORT_URL } from "../lib/links";
import { useWorkspace } from "../state/workspace";
import { Sidebar } from "./Sidebar";

beforeEach(() => {
  localStorage.clear();
  useWorkspace.setState({ tabs: [], activeId: null });
  vi.mocked(openUrl).mockClear();
});

describe("Sidebar", () => {
  it("индикатор LLM — первым внизу панели, пока есть сессии MCP", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValueOnce(
      mcpStatus({ sessions: [{ id: "s1", clientName: "qwen-code", clientVersion: "0.26.7", startedMs: 0, calls: 1, active: false }] }),
    );
    const { container } = renderWithClient(<Sidebar />);
    const indicator = await screen.findByRole("button", { name: "LLM: qwen-code" });
    expect(container.querySelector(".sidebar-footer button")).toBe(indicator);
  });

  it("показывает дерево подключений и ссылки внизу в заданном порядке", async () => {
    const user = userEvent.setup();
    const { container } = renderWithClient(<Sidebar />);
    expect(await screen.findByRole("button", { name: "billing" })).toBeInTheDocument();
    expect(screen.getByText("Подключения")).toBeInTheDocument();
    const footer = [...container.querySelectorAll<HTMLButtonElement>(".sidebar-footer button")].map((b) => b.textContent);
    expect(footer).toEqual(["Настройки", "Поддержать проект", "Справка", "О программе"]);
    expect(screen.queryByRole("button", { name: /^Лицензия/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Настройки" }));
    expect(useWorkspace.getState().activeId).toBe("settings");
  });

  it("«Настройки» при открытой вкладке настроек переключается на неё и не сбрасывает выбранный раздел", async () => {
    const user = userEvent.setup();
    useWorkspace.setState({ tabs: [{ kind: "settings", section: "audit", id: "settings" }, { kind: "about", id: "about" }], activeId: "about" });
    renderWithClient(<Sidebar />);
    await user.click(await screen.findByRole("button", { name: "Настройки" }));
    expect(useWorkspace.getState().activeId).toBe("settings");
    expect(useWorkspace.getState().tabs[0]).toEqual({ kind: "settings", section: "audit", id: "settings" });
  });

  it("«Поддержать проект» открывает страницу поддержки в браузере, вкладку не открывает", async () => {
    const user = userEvent.setup();
    renderWithClient(<Sidebar />);
    await user.click(await screen.findByRole("button", { name: "Поддержать проект" }));
    expect(openUrl).toHaveBeenCalledWith(SUPPORT_URL);
    expect(useWorkspace.getState().tabs).toEqual([]);
  });
});

describe("Sidebar: ширина панели", () => {
  const aside = (c: HTMLElement) => c.querySelector<HTMLElement>("aside.sidebar")!;

  it("по умолчанию 260 px; граница — разделитель с текущим значением и пределами", () => {
    const { container } = renderWithClient(<Sidebar />);
    expect(aside(container).style.width).toBe("260px");
    const sep = screen.getByRole("separator", { name: "Ширина панели подключений" });
    expect(sep).toHaveAttribute("aria-orientation", "vertical");
    expect(sep).toHaveAttribute("aria-valuenow", "260");
    expect(sep).toHaveAttribute("aria-valuemin", "180");
    expect(sep).toHaveAttribute("aria-valuemax", "640");
  });

  it("стрелки меняют ширину с шагом 16 px (Shift — 64), Home и End — до пределов; ширина запоминается", async () => {
    const user = userEvent.setup();
    const { container, unmount } = renderWithClient(<Sidebar />);
    const sep = screen.getByRole("separator", { name: "Ширина панели подключений" });
    sep.focus();
    await user.keyboard("{ArrowRight}{ArrowRight}");
    expect(aside(container).style.width).toBe("292px");
    await user.keyboard("{Shift>}{ArrowLeft}{/Shift}");
    expect(aside(container).style.width).toBe("228px");
    await user.keyboard("{Home}");
    expect(sep).toHaveAttribute("aria-valuenow", "180");
    await user.keyboard("{End}");
    expect(sep).toHaveAttribute("aria-valuenow", "640");
    await user.keyboard("{ArrowLeft}");
    unmount();
    expect(localStorage.getItem("glafira.sidebar.width")).toBe("624");
    const again = renderWithClient(<Sidebar />);
    expect(aside(again.container).style.width).toBe("624px");
  });

  it("перетаскивание границы меняет ширину в пределах; двойной щелчок возвращает ширину по умолчанию", () => {
    const { container } = renderWithClient(<Sidebar />);
    const sep = screen.getByRole("separator", { name: "Ширина панели подключений" });
    fireEvent.pointerDown(sep, { clientX: 260, pointerId: 1, button: 0 });
    fireEvent.pointerMove(sep, { clientX: 340, pointerId: 1 });
    expect(aside(container).style.width).toBe("340px");
    fireEvent.pointerMove(sep, { clientX: 10, pointerId: 1 });
    expect(aside(container).style.width).toBe("180px");
    fireEvent.pointerUp(sep, { clientX: 10, pointerId: 1 });
    fireEvent.pointerMove(sep, { clientX: 400, pointerId: 1 });
    expect(aside(container).style.width).toBe("180px");
    fireEvent.doubleClick(sep);
    expect(aside(container).style.width).toBe("260px");
  });

  it("значение и база перетаскивания — фактическая ширина панели; хранилище пишется по отпусканию (M10 ревью)", () => {
    let observed: (() => void) | null = null;
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(cb: () => void) {
          observed = cb;
        }
        observe() {}
        disconnect() {}
      },
    );
    localStorage.setItem("glafira.sidebar.width", "400");
    const { container } = renderWithClient(<Sidebar />);
    // Узкое окно: flex сжал панель до 200 px.
    vi.spyOn(aside(container), "getBoundingClientRect").mockReturnValue({ width: 200 } as DOMRect);
    act(() => observed?.());
    const sep = screen.getByRole("separator", { name: "Ширина панели подключений" });
    expect(sep).toHaveAttribute("aria-valuenow", "200");
    const setItem = vi.spyOn(Storage.prototype, "setItem");
    fireEvent.pointerDown(sep, { clientX: 500, pointerId: 1, button: 0 });
    fireEvent.pointerMove(sep, { clientX: 540, pointerId: 1 });
    expect(aside(container).style.width).toBe("240px");
    fireEvent.pointerMove(sep, { clientX: 560, pointerId: 1 });
    expect(setItem).not.toHaveBeenCalled();
    fireEvent.pointerUp(sep, { clientX: 560, pointerId: 1 });
    expect(setItem).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem("glafira.sidebar.width")).toBe("260");
    setItem.mockRestore();
    vi.unstubAllGlobals();
  });

  it("сохранённая ширина вне пределов или не число — приводится к пределам или к значению по умолчанию", () => {
    localStorage.setItem("glafira.sidebar.width", "5000");
    const first = renderWithClient(<Sidebar />);
    expect(aside(first.container).style.width).toBe("640px");
    first.unmount();
    localStorage.setItem("glafira.sidebar.width", "abc");
    const second = renderWithClient(<Sidebar />);
    expect(aside(second.container).style.width).toBe("260px");
  });
});
