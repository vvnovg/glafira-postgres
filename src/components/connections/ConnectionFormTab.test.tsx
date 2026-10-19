import { screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithClient } from "../../test/render";
import { newProfile } from "./profile";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return {
    ...orig,
    api: {
      listConnections: vi.fn(),
      listFolders: vi.fn(() => Promise.resolve([{ id: "f1", name: "Прод", parentId: null, order: 0 }])),
      getLicenseStatus: vi.fn(() => Promise.resolve({ tier: "pro", state: "valid" })),
    },
  };
});

import { api } from "../../api/client";
import { ConnectionFormTab } from "./ConnectionFormTab";

beforeEach(() => vi.clearAllMocks());

describe("ConnectionFormTab", () => {
  it("новое подключение открывается в выбранной папке", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([]);
    renderWithClient(<ConnectionFormTab connectionId={null} folderId="f1" tabId="connection-form:new" />);
    expect(await screen.findByRole("heading", { name: "Новое подключение" })).toBeInTheDocument();
    expect(await screen.findByRole("option", { name: "Прод" })).toBeInTheDocument();
    await waitFor(() => expect(screen.getByLabelText("Папка")).toHaveValue("f1"));
  });

  it("существующее подключение загружается в форму, удалённое — сообщение", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([{ ...newProfile(), id: "c1", name: "dev-1", environment: "dev" }]);
    const { unmount } = renderWithClient(<ConnectionFormTab connectionId="c1" tabId="connection-form:c1" />);
    expect(await screen.findByRole("heading", { name: "Подключение «dev-1»" })).toBeInTheDocument();
    expect(screen.getByLabelText("Имя")).toHaveValue("dev-1");
    unmount();
    renderWithClient(<ConnectionFormTab connectionId="gone" tabId="connection-form:gone" />);
    expect(await screen.findByText("Подключение удалено")).toBeInTheDocument();
  });
});
