import { screen } from "@testing-library/react";
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
      getTopology: vi.fn(() => new Promise(() => {})),
      listFolders: vi.fn(() => Promise.resolve([])),
      listConnections: vi.fn(() => Promise.resolve([{ ...newProfile(), id: "c", name: "billing" }])),
    },
  };
});

import { openUrl } from "@tauri-apps/plugin-opener";
import { SUPPORT_URL } from "../lib/links";
import { useWorkspace } from "../state/workspace";
import { Sidebar } from "./Sidebar";

beforeEach(() => {
  localStorage.clear();
  useWorkspace.setState({ tabs: [], activeId: null });
  vi.mocked(openUrl).mockClear();
});

describe("Sidebar", () => {
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

  it("«Поддержать проект» открывает страницу поддержки в браузере, вкладку не открывает", async () => {
    const user = userEvent.setup();
    renderWithClient(<Sidebar />);
    await user.click(await screen.findByRole("button", { name: "Поддержать проект" }));
    expect(openUrl).toHaveBeenCalledWith(SUPPORT_URL);
    expect(useWorkspace.getState().tabs).toEqual([]);
  });
});
