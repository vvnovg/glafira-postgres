import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { member, topology, topologyView } from "../../../test/fixtures";
import { renderWithClient } from "../../../test/render";
import { newProfile } from "../profile";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return {
    ...orig,
    api: {
      getUiSettings: vi.fn(),
      listDatabases: vi.fn(),
      listSchemas: vi.fn(),
      listObjects: vi.fn(),
      listExtensions: vi.fn(),
      listRoles: vi.fn(),
    },
  };
});

import { api } from "../../../api/client";
import { useWorkspace } from "../../../state/workspace";
import { CatalogNodes } from "./CatalogNodes";

const conn = { ...newProfile(), id: "c", name: "billing", kind: "patroni" as const };
const role = (name: string, patch: Record<string, unknown> = {}) => ({
  name,
  superuser: false,
  login: true,
  createDb: false,
  createRole: false,
  replication: false,
  bypassRls: false,
  connLimit: -1,
  validUntil: null,
  memberOf: [],
  ...patch,
});

beforeEach(() => {
  vi.clearAllMocks();
  useWorkspace.setState({ tabs: [], activeId: null });
  vi.mocked(api.getUiSettings).mockResolvedValue({ showSystemObjects: false });
  vi.mocked(api.listDatabases).mockResolvedValue([{ name: "orders", sizeBytes: 1024 }]);
  vi.mocked(api.listSchemas).mockResolvedValue([]);
  vi.mocked(api.listExtensions).mockResolvedValue([]);
  vi.mocked(api.listRoles).mockResolvedValue([]);
});

describe("CatalogNodes: детали", () => {
  it("«Показывать системные объекты» уходит в схемы и роли, первый запрос — уже с нужным значением", async () => {
    vi.mocked(api.getUiSettings).mockResolvedValue({ showSystemObjects: true });
    const user = userEvent.setup();
    renderWithClient(<CatalogNodes connection={conn} view={topologyView()} />);
    await user.click(screen.getByRole("button", { name: /Базы данных/ }));
    await user.click(await screen.findByRole("button", { name: /^orders/ }));
    await user.click(screen.getByRole("button", { name: "Роли" }));
    await waitFor(() => expect(api.listRoles).toHaveBeenCalledWith("c", true));
    await waitFor(() => expect(api.listSchemas).toHaveBeenCalledWith("c", "orders", true));
    expect(api.listSchemas).toHaveBeenCalledTimes(1);
    expect(api.listRoles).toHaveBeenCalledTimes(1);
  });

  it("не прочитанная настройка не блокирует дерево: значение по умолчанию", async () => {
    vi.mocked(api.getUiSettings).mockRejectedValue({ code: "INTERNAL", message: "no settings", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<CatalogNodes connection={conn} view={topologyView()} />);
    await user.click(screen.getByRole("button", { name: /Базы данных/ }));
    await user.click(await screen.findByRole("button", { name: /^orders/ }));
    await waitFor(() => expect(api.listSchemas).toHaveBeenCalledWith("c", "orders", false));
  });

  it("роли: superuser и «без входа»", async () => {
    vi.mocked(api.listRoles).mockResolvedValue([role("postgres", { superuser: true }), role("app_ro", { login: false })]);
    const user = userEvent.setup();
    renderWithClient(<CatalogNodes connection={conn} view={topologyView()} />);
    await user.click(screen.getByRole("button", { name: "Роли" }));
    expect(await screen.findByText("postgres")).toBeInTheDocument();
    expect(screen.getByText("superuser")).toBeInTheDocument();
    expect(screen.getByText("без входа")).toBeInTheDocument();
  });

  it("расширения с доступным обновлением и база без права CONNECT имеют текстовые альтернативы", async () => {
    vi.mocked(api.listDatabases).mockResolvedValue([{ name: "orders", sizeBytes: null }]);
    vi.mocked(api.listExtensions).mockResolvedValue([
      { name: "pg_stat_statements", version: "1.10", schema: "public", defaultVersion: "1.11", updateAvailable: true },
      { name: "plpgsql", version: "1.0", schema: "pg_catalog", defaultVersion: "1.0", updateAvailable: false },
    ]);
    const user = userEvent.setup();
    renderWithClient(<CatalogNodes connection={conn} view={topologyView()} />);
    await user.click(screen.getByRole("button", { name: /Базы данных/ }));
    expect(await screen.findByRole("img", { name: "Нет права CONNECT на базу" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^orders/ }));
    await user.click(await screen.findByRole("button", { name: "Расширения" }));
    expect(await screen.findByText("pg_stat_statements")).toBeInTheDocument();
    expect(screen.getAllByRole("img", { name: "Доступна версия 1.11" })).toHaveLength(1);
  });

  it("недоступный узел и большой лаг помечены с подписью; узел открывает обзор", async () => {
    const view = topologyView({
      topology: topology({
        members: [
          member("pg-1", { role: "primary", state: "running", lagBytes: null }),
          member("pg-2", { reachable: false }),
          member("pg-3", { lagBytes: 32 * 1024 * 1024 }),
        ],
      }),
    });
    const user = userEvent.setup();
    renderWithClient(<CatalogNodes connection={conn} view={view} />);
    await user.click(screen.getByRole("button", { name: /^Узлы/ }));
    expect(screen.getByRole("img", { name: "Узел недоступен" })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Лаг больше 16 МБ" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "○ pg-2" }));
    expect(useWorkspace.getState().tabs).toEqual([{ kind: "overview", connectionId: "c", member: "pg-2", id: "overview:c" }]);
  });

  it("нет узла для каталога — сообщение вместо баз и ролей", () => {
    renderWithClient(<CatalogNodes connection={conn} view={topologyView({ catalogMember: null })} />);
    expect(screen.getByText("Нет доступного узла для каталога")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Базы данных/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Роли" })).not.toBeInTheDocument();
  });

  it("ошибка списка баз показывается с повтором", async () => {
    vi.mocked(api.listDatabases).mockRejectedValueOnce({ code: "NODE_UNAVAILABLE", message: "node down", retryable: true });
    const user = userEvent.setup();
    renderWithClient(<CatalogNodes connection={conn} view={topologyView()} />);
    await user.click(screen.getByRole("button", { name: /Базы данных/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("node down");
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(await screen.findByRole("button", { name: /^orders/ })).toBeInTheDocument();
  });

  it("вложенные списки названы по заголовкам групп", async () => {
    vi.mocked(api.listSchemas).mockResolvedValue([
      { name: "shop", system: false, counts: { tables: 1, views: 0, matviews: 0, sequences: 0, functions: 0 } },
    ]);
    const user = userEvent.setup();
    renderWithClient(<CatalogNodes connection={conn} view={topologyView()} />);
    await user.click(screen.getByRole("button", { name: /Базы данных/ }));
    await user.click(await screen.findByRole("button", { name: /^orders/ }));
    await user.click(await screen.findByRole("button", { name: /^shop/ }));
    expect(screen.getByRole("list", { name: "shop" })).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "orders" })).toBeInTheDocument();
  });
});
