import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { newProfile } from "./connections/profile";
import { renderWithClient } from "../test/render";

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

import { useWorkspace } from "../state/workspace";
import { Sidebar } from "./Sidebar";

beforeEach(() => {
  localStorage.clear();
  useWorkspace.setState({ tabs: [], activeId: null });
});

describe("Sidebar", () => {
  it("показывает дерево подключений и ссылки внизу", async () => {
    const user = userEvent.setup();
    renderWithClient(<Sidebar />);
    expect(await screen.findByRole("button", { name: "billing" })).toBeInTheDocument();
    expect(screen.getByText("Подключения")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Лицензия/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Настройки" }));
    expect(useWorkspace.getState().activeId).toBe("settings");
  });
});
