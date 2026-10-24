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
      connectionSecrets: vi.fn(() => Promise.resolve({ pgPassword: false, pgKeyPassword: false, patroniPassword: false, patroniKeyPassword: false, sshPassword: false, sshPassphrase: false })),
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

  it("копия: настройки источника, новое имя, пароли пустые, удалять нечего", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([
      { ...newProfile(), id: "c1", name: "dev-1", environment: "dev", hosts: [{ host: "pg-1", port: 5460 }], folderId: "f1" },
    ]);
    renderWithClient(<ConnectionFormTab connectionId={null} copyOf="c1" tabId="connection-form:copy:c1" />);
    expect(await screen.findByRole("heading", { name: "Копия подключения «dev-1»" })).toBeInTheDocument();
    expect(screen.getByText("Пароли не копируются: введите их заново.")).toBeInTheDocument();
    expect(screen.getByLabelText("Имя")).toHaveValue("dev-1 (копия)");
    expect(screen.getByLabelText("Хост")).toHaveValue("pg-1");
    expect(screen.getByLabelText("Пароль")).toHaveValue("");
    expect(screen.queryByRole("button", { name: "Удалить" })).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByLabelText("Папка")).toHaveValue("f1"));
    // У копии сохранённых секретов нет: keychain источника не читается.
    expect(api.connectionSecrets).not.toHaveBeenCalled();
  });

  it("копия prod-подключения: prod доступен, «Только чтение» как у источника", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([{ ...newProfile(), id: "p1", name: "billing", environment: "prod", readOnly: true }]);
    renderWithClient(<ConnectionFormTab connectionId={null} copyOf="p1" tabId="connection-form:copy:p1" />);
    expect(await screen.findByRole("heading", { name: "Копия подключения «billing»" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "prod" })).toBeEnabled();
    expect(screen.getByLabelText("Только чтение")).toBeChecked();
  });

  it("копия удалённого подключения — сообщение", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([]);
    renderWithClient(<ConnectionFormTab connectionId={null} copyOf="gone" tabId="connection-form:copy:gone" />);
    expect(await screen.findByText("Подключение удалено")).toBeInTheDocument();
  });

  it("существующее подключение загружается в форму, удалённое — сообщение", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([{ ...newProfile(), id: "c1", name: "dev-1", environment: "dev" }]);
    const { unmount } = renderWithClient(<ConnectionFormTab connectionId="c1" tabId="connection-form:c1" />);
    expect(await screen.findByRole("heading", { name: "Подключение «dev-1»" })).toBeInTheDocument();
    expect(screen.getByLabelText("Имя")).toHaveValue("dev-1");
    await waitFor(() => expect(api.connectionSecrets).toHaveBeenCalledWith("c1"));
    unmount();
    renderWithClient(<ConnectionFormTab connectionId="gone" tabId="connection-form:gone" />);
    expect(await screen.findByText("Подключение удалено")).toBeInTheDocument();
  });
});
