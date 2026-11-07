import { fireEvent, screen } from "@testing-library/react";
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

describe("Sidebar: ширина панели (useResizable, спец. U1 §7)", () => {
  const aside = (c: HTMLElement) => c.querySelector<HTMLElement>("aside.sidebar")!;

  it("по умолчанию 248 px; граница — разделитель с текущим значением и пределами 200–400", () => {
    const { container } = renderWithClient(<Sidebar />);
    expect(aside(container).style.width).toBe("248px");
    const sep = screen.getByRole("separator", { name: "Ширина панели подключений" });
    expect(sep).toHaveAttribute("aria-orientation", "vertical");
    expect(sep).toHaveAttribute("aria-valuenow", "248");
    expect(sep).toHaveAttribute("aria-valuemin", "200");
    expect(sep).toHaveAttribute("aria-valuemax", "400");
    expect(sep).toHaveAttribute("title", "Перетащите, чтобы изменить ширину; двойной щелчок — ширина по умолчанию");
  });

  it("стрелки меняют ширину с шагом 16 px, Home и End — до пределов, Enter — по умолчанию; ширина запоминается", async () => {
    const user = userEvent.setup();
    const { container, unmount } = renderWithClient(<Sidebar />);
    const sep = screen.getByRole("separator", { name: "Ширина панели подключений" });
    sep.focus();
    await user.keyboard("{ArrowRight}{ArrowRight}");
    expect(aside(container).style.width).toBe("280px");
    await user.keyboard("{Home}");
    expect(sep).toHaveAttribute("aria-valuenow", "200");
    await user.keyboard("{End}");
    expect(sep).toHaveAttribute("aria-valuenow", "400");
    await user.keyboard("{Enter}");
    expect(sep).toHaveAttribute("aria-valuenow", "248");
    await user.keyboard("{ArrowLeft}");
    unmount();
    expect(localStorage.getItem("glafira.sidebar.width")).toBe("232");
    const again = renderWithClient(<Sidebar />);
    expect(aside(again.container).style.width).toBe("232px");
  });

  it("перетаскивание границы меняет ширину в пределах; двойной щелчок возвращает ширину по умолчанию", () => {
    const { container } = renderWithClient(<Sidebar />);
    const sep = screen.getByRole("separator", { name: "Ширина панели подключений" });
    fireEvent.mouseDown(sep, { clientX: 248, button: 0 });
    fireEvent.mouseMove(document, { clientX: 328 });
    expect(aside(container).style.width).toBe("328px");
    fireEvent.mouseMove(document, { clientX: 10 });
    expect(aside(container).style.width).toBe("200px");
    fireEvent.mouseUp(document);
    fireEvent.mouseMove(document, { clientX: 400 });
    expect(aside(container).style.width).toBe("200px");
    fireEvent.doubleClick(sep);
    expect(aside(container).style.width).toBe("248px");
  });

  it("сохранённая ширина вне пределов или не число — приводится к пределам или к значению по умолчанию", () => {
    // Ширина до U1 была до 640 px: запомненное значение больше 400 сжимается до предела.
    localStorage.setItem("glafira.sidebar.width", "600");
    const first = renderWithClient(<Sidebar />);
    expect(aside(first.container).style.width).toBe("400px");
    first.unmount();
    localStorage.setItem("glafira.sidebar.width", "abc");
    const second = renderWithClient(<Sidebar />);
    expect(aside(second.container).style.width).toBe("248px");
  });
});
