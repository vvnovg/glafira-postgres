import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { topologyView } from "../../../test/fixtures";
import { renderWithClient } from "../../../test/render";
import { newProfile } from "../profile";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return {
    ...orig,
    api: {
      getTopology: vi.fn(),
      connect: vi.fn(),
      disconnect: vi.fn(),
      setCatalogMember: vi.fn(),
      getUiSettings: vi.fn(() => Promise.resolve({ showSystemObjects: false })),
      listDatabases: vi.fn(() => Promise.resolve([])),
      listRoles: vi.fn(() => Promise.resolve([])),
    },
  };
});

import { api } from "../../../api/client";
import { disconnectedView } from "../../../lib/useConnectionEvents";
import { useWorkspace } from "../../../state/workspace";
import { ConnectionNode } from "./ConnectionNode";

const conn = { ...newProfile(), id: "c", name: "billing", kind: "patroni" as const };
const toggleName = "Развернуть или свернуть подключение billing";

function renderNode() {
  return renderWithClient(
    <ul>
      <ConnectionNode connection={conn} />
    </ul>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.getTopology).mockResolvedValue(disconnectedView);
  vi.mocked(api.getUiSettings).mockResolvedValue({ showSystemObjects: false });
  vi.mocked(api.listDatabases).mockResolvedValue([]);
  vi.mocked(api.listRoles).mockResolvedValue([]);
  useWorkspace.setState({ tabs: [], activeId: null });
});

describe("ConnectionNode", () => {
  it("раскрытие открывает подключение и показывает узлы каталога", async () => {
    vi.mocked(api.connect).mockResolvedValue(topologyView());
    const user = userEvent.setup();
    renderNode();
    expect(await screen.findByRole("img", { name: "Не подключено" })).toBeInTheDocument();
    expect(screen.getByText("Patroni")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: toggleName }));
    expect(api.connect).toHaveBeenCalledWith("c");
    expect(await screen.findByRole("button", { name: /^Узлы/ })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Подключено" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: toggleName })).toHaveAttribute("aria-expanded", "true");
  });

  it("уже подключённое раскрывается без повторного подключения", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView());
    const user = userEvent.setup();
    renderNode();
    await screen.findByRole("img", { name: "Подключено" });
    await user.click(screen.getByRole("button", { name: toggleName }));
    expect(await screen.findByRole("button", { name: /^Узлы/ })).toBeInTheDocument();
    expect(api.connect).not.toHaveBeenCalled();
  });

  it("ошибка подключения показывается под узлом; повторное раскрытие пробует снова", async () => {
    vi.mocked(api.connect).mockRejectedValueOnce({ code: "CONNECTION_FAILED", message: "connection refused", retryable: true });
    const user = userEvent.setup();
    renderNode();
    await screen.findByRole("img", { name: "Не подключено" });
    await user.click(screen.getByRole("button", { name: toggleName }));
    expect(await screen.findByRole("alert")).toHaveTextContent("connection refused");
    expect(screen.queryByRole("button", { name: /^Узлы/ })).not.toBeInTheDocument();

    vi.mocked(api.connect).mockResolvedValueOnce(topologyView());
    await user.click(screen.getByRole("button", { name: toggleName })); // свернуть
    await user.click(screen.getByRole("button", { name: toggleName })); // развернуть
    expect(await screen.findByRole("button", { name: /^Узлы/ })).toBeInTheDocument();
    expect(api.connect).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("причина недоступности из события показывается, если своя ошибка пуста", async () => {
    vi.mocked(api.getTopology).mockResolvedValue({
      ...disconnectedView,
      state: "failed",
      error: { code: "TIMEOUT", message: "timed out", retryable: true },
    });
    const user = userEvent.setup();
    vi.mocked(api.connect).mockReturnValue(new Promise(() => {}));
    renderNode();
    expect(await screen.findByRole("img", { name: "Не удалось подключиться" })).toBeInTheDocument();
    // Раскрытие повторяет попытку; пока она идёт, видна причина прошлой неудачи.
    await user.click(screen.getByRole("button", { name: toggleName }));
    expect(await screen.findByRole("alert")).toHaveTextContent("timed out");
  });

  it("клик по имени открывает обзор подключения", async () => {
    const user = userEvent.setup();
    renderNode();
    await user.click(screen.getByRole("button", { name: "billing" }));
    expect(useWorkspace.getState().tabs).toEqual([{ kind: "overview", connectionId: "c", id: "overview:c" }]);
  });

  it("меню: узел каталога меняется, кэш каталога перечитывается", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView());
    vi.mocked(api.setCatalogMember).mockResolvedValue(topologyView({ catalogMember: "pg-2" }));
    const user = userEvent.setup();
    const { client } = renderNode();
    client.setQueryData(["catalog", "c", "databases"], []);
    client.setQueryData(["catalog", "other", "databases"], []);
    await screen.findByRole("img", { name: "Подключено" });

    await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
    const menu = screen.getByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual([
      "Обзор",
      "Изменить подключение",
      "Каталог — с узла по умолчанию",
      "Каталог — с узла pg-1",
      "Каталог — с узла pg-2",
      "Отключиться",
    ]);
    await user.click(within(menu).getByRole("menuitem", { name: "Каталог — с узла pg-2" }));
    expect(api.setCatalogMember).toHaveBeenCalledWith("c", "pg-2");
    await waitFor(() => expect(client.getQueryData<{ catalogMember: string }>(["topology", "c"])?.catalogMember).toBe("pg-2"));
    expect(client.getQueryState(["catalog", "c", "databases"])?.isInvalidated).toBe(true);
    // Каталог другого подключения не затронут.
    expect(client.getQueryState(["catalog", "other", "databases"])?.isInvalidated).toBe(false);
  });

  it("меню: отключение сворачивает узел; у одиночного сервера выбора узла каталога нет", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView());
    vi.mocked(api.disconnect).mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderNode();
    await screen.findByRole("img", { name: "Подключено" });
    await user.click(screen.getByRole("button", { name: toggleName }));
    await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
    await user.click(screen.getByRole("menuitem", { name: "Отключиться" }));
    expect(api.disconnect).toHaveBeenCalledWith("c");
    await waitFor(() => expect(screen.getByRole("button", { name: toggleName })).toHaveAttribute("aria-expanded", "false"));
  });

  it("меню отключённого подключения: «Подключиться» раскрывает и подключает", async () => {
    vi.mocked(api.connect).mockResolvedValue(topologyView());
    const user = userEvent.setup();
    renderNode();
    await screen.findByRole("img", { name: "Не подключено" });
    await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
    const menu = screen.getByRole("menu");
    expect(within(menu).queryByRole("menuitem", { name: /Каталог/ })).not.toBeInTheDocument();
    await user.click(within(menu).getByRole("menuitem", { name: "Подключиться" }));
    expect(api.connect).toHaveBeenCalledWith("c");
    expect(await screen.findByRole("button", { name: /^Узлы/ })).toBeInTheDocument();
  });

  it("ошибка отключения показывается, а не теряется", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView());
    vi.mocked(api.disconnect).mockRejectedValue({ code: "INTERNAL", message: "boom", retryable: false });
    const user = userEvent.setup();
    renderNode();
    await screen.findByRole("img", { name: "Подключено" });
    await user.click(screen.getByRole("button", { name: toggleName }));
    await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
    await user.click(screen.getByRole("menuitem", { name: "Отключиться" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("boom");
  });
});
