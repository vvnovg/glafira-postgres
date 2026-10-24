import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppError, ConnectionProfile } from "../../../api/types";
import { member, topology, topologyView } from "../../../test/fixtures";
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
      reconnect: vi.fn(),
      trustSshHostKey: vi.fn(),
      setCatalogMember: vi.fn(),
      getUiSettings: vi.fn(() => Promise.resolve({ showSystemObjects: false })),
      listDatabases: vi.fn(() => Promise.resolve([])),
      listRoles: vi.fn(() => Promise.resolve([])),
    },
  };
});

import { api } from "../../../api/client";
import { i18n } from "../../../i18n";
import { disconnectedView } from "../../../lib/useConnectionEvents";
import { useWorkspace } from "../../../state/workspace";
import { ConnectionNode } from "./ConnectionNode";

const conn = { ...newProfile(), id: "c", name: "billing", kind: "patroni" as const };
const toggleName = "Развернуть или свернуть подключение billing";

function renderNode(connection: ConnectionProfile = conn) {
  return renderWithClient(
    <ul>
      <ConnectionNode connection={connection} />
    </ul>,
  );
}

const serverConn = { ...conn, kind: "server" as const };
/** Вид одиночного сервера: один участник с заданной ролью. */
const serverView = (role: "primary" | "replica" | "unknown") =>
  topologyView({ topology: topology({ kind: "server", members: [member("pg-1", { role, state: "running", lagBytes: null })] }) });

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
      "Дублировать подключение",
      "Каталог — с узла по умолчанию",
      "Каталог — с узла pg-1",
      "Каталог — с узла pg-2",
      "Переподключить",
      "Отключиться",
    ]);
    await user.click(within(menu).getByRole("menuitem", { name: "Каталог — с узла pg-2" }));
    expect(api.setCatalogMember).toHaveBeenCalledWith("c", "pg-2");
    await waitFor(() => expect(client.getQueryData<{ catalogMember: string }>(["topology", "c"])?.catalogMember).toBe("pg-2"));
    expect(client.getQueryState(["catalog", "c", "databases"])?.isInvalidated).toBe(true);
    // Каталог другого подключения не затронут.
    expect(client.getQueryState(["catalog", "other", "databases"])?.isInvalidated).toBe(false);
  });

  it("меню: «Дублировать подключение» открывает копию в той же папке", async () => {
    const user = userEvent.setup();
    renderNode({ ...conn, folderId: "f1" });
    await screen.findByRole("img", { name: "Не подключено" });
    await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
    await user.click(screen.getByRole("menuitem", { name: "Дублировать подключение" }));
    const { tabs, activeId } = useWorkspace.getState();
    expect(tabs).toEqual([{ kind: "connection-form", connectionId: null, copyOf: "c", folderId: "f1", id: "connection-form:copy:c" }]);
    expect(activeId).toBe("connection-form:copy:c");
    expect(api.connect).not.toHaveBeenCalled();
  });

  it("меню живого подключения: «Переподключить» оставляет подключение открытым, вид — из ответа, узел остаётся раскрытым", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "degraded" }));
    vi.mocked(api.reconnect).mockResolvedValue(topologyView());
    const user = userEvent.setup();
    const { client } = renderNode();
    client.setQueryData(["catalog", "c", "databases"], []);
    client.setQueryData(["memberFacts", "c", "pg-1"], { startedAtMs: 0, connections: 1, maxConnections: 100, databases: 1 });
    client.setQueryData(["catalog", "other", "databases"], []);
    await screen.findByRole("img", { name: "Часть узлов недоступна" });
    await user.click(screen.getByRole("button", { name: toggleName }));
    await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
    await user.click(screen.getByRole("menuitem", { name: "Переподключить" }));
    expect(api.reconnect).toHaveBeenCalledWith("c");
    expect(await screen.findByRole("img", { name: "Подключено" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: toggleName })).toHaveAttribute("aria-expanded", "true");
    // Пулы и секреты в Rust перечитаны, события Disconnected нет: каталог и факты этого подключения перечитываются, чужие — нет.
    await waitFor(() => expect(client.getQueryState(["catalog", "c", "databases"])?.isInvalidated).toBe(true));
    expect(client.getQueryState(["memberFacts", "c", "pg-1"])?.isInvalidated).toBe(true);
    expect(client.getQueryState(["catalog", "other", "databases"])?.isInvalidated).toBe(false);
  });

  it("меню: пока идёт подключение (connecting), «Переподключить» не предлагается", async () => {
    // Переподключать ещё нечего: подключение только открывается.
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "connecting" }));
    const user = userEvent.setup();
    renderNode();
    await screen.findByRole("img", { name: "Подключение…" });
    await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
    const items = within(screen.getByRole("menu")).getAllByRole("menuitem");
    expect(items.map((m) => m.textContent)).not.toContain("Переподключить");
    expect(api.reconnect).not.toHaveBeenCalled();
  });

  it("меню: пока идёт переподключение, «Переподключить» не предлагается — повторные нажатия не копятся", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "degraded" }));
    let finish: (v: ReturnType<typeof topologyView>) => void = () => {};
    vi.mocked(api.reconnect).mockReturnValue(new Promise((resolve) => (finish = resolve)));
    const user = userEvent.setup();
    renderNode();
    await screen.findByRole("img", { name: "Часть узлов недоступна" });
    const menuButton = screen.getByRole("button", { name: "Действия с подключением billing" });
    await user.click(menuButton);
    await user.click(screen.getByRole("menuitem", { name: "Переподключить" }));
    expect(api.reconnect).toHaveBeenCalledTimes(1);

    await user.click(menuButton);
    const busyItems = within(screen.getByRole("menu")).getAllByRole("menuitem");
    expect(busyItems.map((m) => m.textContent)).not.toContain("Переподключить");
    await user.click(menuButton);

    await act(async () => finish(topologyView()));
    expect(await screen.findByRole("img", { name: "Подключено" })).toBeInTheDocument();
    await user.click(menuButton);
    expect(within(screen.getByRole("menu")).getByRole("menuitem", { name: "Переподключить" })).toBeInTheDocument();
    expect(api.reconnect).toHaveBeenCalledTimes(1);
  });

  it("ошибка переподключения показывается под узлом", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "degraded" }));
    vi.mocked(api.reconnect).mockRejectedValue({ code: "CONNECTION_FAILED", message: "reopen refused", retryable: true });
    const user = userEvent.setup();
    renderNode();
    await screen.findByRole("img", { name: "Часть узлов недоступна" });
    await user.click(screen.getByRole("button", { name: toggleName }));
    await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
    await user.click(screen.getByRole("menuitem", { name: "Переподключить" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("reopen refused");
    // После ошибки пункт снова предлагается: флаг переподключения сброшен.
    await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
    expect(within(screen.getByRole("menu")).getByRole("menuitem", { name: "Переподключить" })).toBeInTheDocument();
  });

  it("«Подключиться» после «Отключиться» во время долгого переподключения не пропадает", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "degraded" }));
    vi.mocked(api.reconnect).mockReturnValue(new Promise(() => {}));
    vi.mocked(api.disconnect).mockResolvedValue(undefined);
    vi.mocked(api.connect).mockResolvedValue(topologyView());
    const user = userEvent.setup();
    const { client } = renderNode();
    await screen.findByRole("img", { name: "Часть узлов недоступна" });
    const menuButton = screen.getByRole("button", { name: "Действия с подключением billing" });
    await user.click(menuButton);
    await user.click(screen.getByRole("menuitem", { name: "Переподключить" }));
    await user.click(menuButton);
    await user.click(screen.getByRole("menuitem", { name: "Отключиться" }));
    act(() => client.setQueryData(["topology", "c"], disconnectedView));
    await screen.findByRole("img", { name: "Не подключено" });
    await user.click(menuButton);
    await user.click(screen.getByRole("menuitem", { name: "Подключиться" }));
    expect(api.connect).toHaveBeenCalledWith("c");
    expect(await screen.findByRole("img", { name: "Подключено" })).toBeInTheDocument();
  });

  it("меню: отключение сворачивает узел", async () => {
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

  it("меню одиночного сервера: выбора узла каталога нет", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(serverView("primary"));
    const user = userEvent.setup();
    renderNode(serverConn);
    await screen.findByRole("img", { name: "Подключено" });
    await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
    const items = within(screen.getByRole("menu")).getAllByRole("menuitem");
    expect(items.map((m) => m.textContent)).toEqual(["Обзор", "Изменить подключение", "Дублировать подключение", "Переподключить", "Отключиться"]);
    expect(within(screen.getByRole("menu")).queryByRole("menuitem", { name: /Каталог/ })).not.toBeInTheDocument();
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

  it("меню упавшего подключения: повторить (раскрывает и подключает) и отключиться", async () => {
    vi.mocked(api.getTopology).mockResolvedValue({
      ...disconnectedView,
      state: "failed",
      error: { code: "AUTH_FAILED", message: "auth failed", retryable: false },
    });
    vi.mocked(api.connect).mockResolvedValue(topologyView());
    const user = userEvent.setup();
    renderNode();
    await screen.findByRole("img", { name: "Не удалось подключиться" });
    await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
    const menu = screen.getByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual([
      "Обзор",
      "Изменить подключение",
      "Дублировать подключение",
      "Повторить подключение",
      "Отключиться",
    ]);
    await user.click(within(menu).getByRole("menuitem", { name: "Повторить подключение" }));
    expect(api.connect).toHaveBeenCalledWith("c");
    expect(screen.getByRole("button", { name: toggleName })).toHaveAttribute("aria-expanded", "true");
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

  describe("роль одиночного сервера в заголовке", () => {
    it("primary и standby показывают роль единственного узла", async () => {
      vi.mocked(api.getTopology).mockResolvedValue(serverView("primary"));
      const first = renderNode(serverConn);
      expect(await screen.findByText("Primary")).toBeInTheDocument();
      first.unmount();

      vi.mocked(api.getTopology).mockResolvedValue(serverView("replica"));
      renderNode(serverConn);
      expect(await screen.findByText("Реплика")).toBeInTheDocument();
      expect(screen.queryByText("Primary")).not.toBeInTheDocument();
    });

    it("подпись следует за языком интерфейса", async () => {
      vi.mocked(api.getTopology).mockResolvedValue(serverView("replica"));
      renderNode(serverConn);
      expect(await screen.findByText("Реплика")).toBeInTheDocument();
      try {
        await act(() => i18n.changeLanguage("en"));
        expect(screen.getByText("Replica")).toBeInTheDocument();
      } finally {
        await act(() => i18n.changeLanguage("ru"));
      }
    });

    it("пока топология неизвестна (не подключено) — значка нет", async () => {
      renderNode(serverConn); // getTopology → disconnectedView, топологии нет
      await screen.findByRole("img", { name: "Не подключено" });
      expect(document.querySelector(".kind-badge")).toBeNull();
    });

    it("роль не определена — значка нет, а не «—»", async () => {
      vi.mocked(api.getTopology).mockResolvedValue(serverView("unknown"));
      renderNode(serverConn);
      await screen.findByRole("img", { name: "Подключено" });
      expect(document.querySelector(".kind-badge")).toBeNull();
    });

    it("у Patroni и списка хостов роль в заголовке не показывается", async () => {
      vi.mocked(api.getTopology).mockResolvedValue(topologyView());
      const first = renderNode();
      await screen.findByRole("img", { name: "Подключено" });
      expect(screen.getByText("Patroni")).toBeInTheDocument();
      for (const text of ["Primary", "Leader", "Реплика", "Sync"]) expect(screen.queryByText(text)).not.toBeInTheDocument();
      first.unmount();

      vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ kind: "hostList" }) }));
      renderNode({ ...conn, kind: "hostList" });
      await screen.findByRole("img", { name: "Подключено" });
      for (const text of ["Primary", "Leader", "Реплика", "Sync"]) expect(screen.queryByText(text)).not.toBeInTheDocument();
    });
  });

  describe("устаревшая локальная ошибка", () => {
    const openMenuAndPick = async (user: ReturnType<typeof userEvent.setup>, item: string) => {
      await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
      await user.click(screen.getByRole("menuitem", { name: item }));
    };

    it("ошибка смены узла каталога исчезает, когда подключение снова connected", async () => {
      vi.mocked(api.getTopology).mockResolvedValue(topologyView());
      vi.mocked(api.setCatalogMember).mockRejectedValue({ code: "NODE_UNAVAILABLE", message: "node down", retryable: true });
      const user = userEvent.setup();
      const { client } = renderNode();
      await screen.findByRole("img", { name: "Подключено" });
      await user.click(screen.getByRole("button", { name: toggleName }));
      await openMenuAndPick(user, "Каталог — с узла pg-2");
      expect(await screen.findByRole("alert")).toHaveTextContent("node down");

      // Часть узлов недоступна — подключение ещё не здорово, ошибка остаётся.
      act(() => client.setQueryData(["topology", "c"], topologyView({ state: "degraded" })));
      await screen.findByRole("img", { name: "Часть узлов недоступна" });
      expect(screen.getByRole("alert")).toHaveTextContent("node down");
      // Снова connected — ошибка устарела.
      act(() => client.setQueryData(["topology", "c"], topologyView()));
      await screen.findByRole("img", { name: "Подключено" });
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    });

    it("ошибка подключения не висит под узлом, который ожил по событию", async () => {
      vi.mocked(api.connect).mockRejectedValue({ code: "CONNECTION_FAILED", message: "connection refused", retryable: true });
      const user = userEvent.setup();
      const { client } = renderNode();
      await screen.findByRole("img", { name: "Не подключено" });
      await user.click(screen.getByRole("button", { name: toggleName }));
      expect(await screen.findByRole("alert")).toHaveTextContent("connection refused");

      act(() => client.setQueryData(["topology", "c"], topologyView()));
      expect(await screen.findByRole("button", { name: /^Узлы/ })).toBeInTheDocument();
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    });

    it("следующая удачная смена узла каталога очищает прежнюю ошибку", async () => {
      vi.mocked(api.getTopology).mockResolvedValue(topologyView());
      vi.mocked(api.setCatalogMember).mockRejectedValueOnce({ code: "NODE_UNAVAILABLE", message: "node down", retryable: true });
      vi.mocked(api.setCatalogMember).mockResolvedValueOnce(topologyView({ catalogMember: "pg-1" }));
      const user = userEvent.setup();
      renderNode();
      await screen.findByRole("img", { name: "Подключено" });
      await user.click(screen.getByRole("button", { name: toggleName }));
      await openMenuAndPick(user, "Каталог — с узла pg-2");
      expect(await screen.findByRole("alert")).toHaveTextContent("node down");

      await openMenuAndPick(user, "Каталог — с узла pg-1");
      await waitFor(() => expect(api.setCatalogMember).toHaveBeenCalledTimes(2));
      await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
    });
  });

  describe("доверие ключу хоста ssh", () => {
    const hostKeyError = (patch: Partial<AppError> = {}, params: Record<string, string> = {}): AppError => ({
      code: "SSH_ERROR",
      key: "errors.ssh.hostKeyUnknown",
      message: "unknown host key",
      retryable: false,
      params: { kind: "hostKeyUnknown", target: "bastion", host: "bastion", fingerprint: "SHA256:abc", ...params },
      ...patch,
    });

    it("неизвестный ключ хоста при подключении из дерева: диалог, доверие, повтор", async () => {
      const hostKey = { code: "SSH_ERROR", key: "errors.ssh.hostKeyUnknown", message: "unknown host key", retryable: false, params: { kind: "hostKeyUnknown", target: "bastion", host: "bastion", fingerprint: "SHA256:abc" } };
      vi.mocked(api.connect).mockRejectedValueOnce(hostKey).mockResolvedValueOnce(topologyView());
      vi.mocked(api.trustSshHostKey).mockResolvedValue(undefined);
      const user = userEvent.setup();
      renderNode();
      await screen.findByRole("img", { name: "Не подключено" });
      await user.click(screen.getByRole("button", { name: toggleName }));
      const dialog = await screen.findByRole("dialog", { name: "Неизвестный ключ хоста" });
      await user.click(within(dialog).getByRole("button", { name: "Доверять и подключиться" }));
      expect(api.trustSshHostKey).toHaveBeenCalledWith("bastion", "SHA256:abc");
      expect(await screen.findByRole("img", { name: "Подключено" })).toBeInTheDocument();
      expect(api.connect).toHaveBeenCalledTimes(2);
      expect(api.reconnect).not.toHaveBeenCalled();
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

    it("открытое, но ограниченное подключение: после доверия — «Переподключить», а не «Подключиться»", async () => {
      // Туннель на стоянке с hostKeyUnknown сам не поднимется: после доверия переподключение запускает интерфейс.
      const failing = topologyView({ state: "degraded", error: hostKeyError() });
      vi.mocked(api.getTopology).mockResolvedValue(failing);
      vi.mocked(api.reconnect).mockResolvedValueOnce(failing).mockResolvedValueOnce(topologyView());
      vi.mocked(api.trustSshHostKey).mockResolvedValue(undefined);
      const user = userEvent.setup();
      renderNode();
      await screen.findByRole("img", { name: "Часть узлов недоступна" });
      // Диалог не всплывает сам по событию из фона: только после действия пользователя.
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
      await user.click(screen.getByRole("menuitem", { name: "Переподключить" }));
      const dialog = await screen.findByRole("dialog", { name: "Неизвестный ключ хоста" });
      await user.click(within(dialog).getByRole("button", { name: "Доверять и подключиться" }));
      expect(api.trustSshHostKey).toHaveBeenCalledWith("bastion", "SHA256:abc");
      expect(await screen.findByRole("img", { name: "Подключено" })).toBeInTheDocument();
      expect(api.reconnect).toHaveBeenCalledTimes(2);
      expect(api.connect).not.toHaveBeenCalled();
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

    it("«Отмена»: ничего не доверяется, диалог закрыт, причина остаётся под узлом", async () => {
      vi.mocked(api.connect).mockRejectedValueOnce(hostKeyError());
      const user = userEvent.setup();
      renderNode();
      await screen.findByRole("img", { name: "Не подключено" });
      await user.click(screen.getByRole("button", { name: toggleName }));
      const dialog = await screen.findByRole("dialog", { name: "Неизвестный ключ хоста" });
      await user.click(within(dialog).getByRole("button", { name: "Отмена" }));
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      expect(screen.getByRole("alert")).toHaveTextContent("Ключ хоста bastion ещё не известен (SHA256:abc)");
      expect(api.trustSshHostKey).not.toHaveBeenCalled();
      expect(api.connect).toHaveBeenCalledTimes(1);
    });

    it("«Переподключить»: другая ошибка туннеля из вида (смена ключа хоста) показывается под узлом, без диалога", async () => {
      // Ответ reconnect — degraded с ошибкой в `view.error`, а не отказ команды; фоновая ошибка при degraded под узлом
      // не показывается, поэтому действие пользователя поднимает её в локальную.
      const changed = hostKeyError({ key: "errors.ssh.hostKeyChanged", message: "The host key of bastion has changed" }, { kind: "hostKeyChanged" });
      vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "degraded" }));
      vi.mocked(api.reconnect).mockResolvedValue(topologyView({ state: "degraded", error: changed }));
      const user = userEvent.setup();
      renderNode();
      await screen.findByRole("img", { name: "Часть узлов недоступна" });
      await user.click(screen.getByRole("button", { name: toggleName }));
      await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
      await user.click(screen.getByRole("menuitem", { name: "Переподключить" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("Ключ хоста bastion изменился: проверьте ~/.ssh/known_hosts сами");
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      expect(api.trustSshHostKey).not.toHaveBeenCalled();
    });

    it("отказ команды доверия показывается в диалоге, подключение не повторяется", async () => {
      vi.mocked(api.connect).mockRejectedValueOnce(hostKeyError());
      vi.mocked(api.trustSshHostKey).mockRejectedValue({ code: "INVALID_INPUT", message: "Fingerprint is empty", retryable: false });
      const user = userEvent.setup();
      renderNode();
      await screen.findByRole("img", { name: "Не подключено" });
      await user.click(screen.getByRole("button", { name: toggleName }));
      const dialog = await screen.findByRole("dialog", { name: "Неизвестный ключ хоста" });
      await user.click(within(dialog).getByRole("button", { name: "Доверять и подключиться" }));
      expect(await within(dialog).findByText("Fingerprint is empty")).toBeInTheDocument();
      expect(api.connect).toHaveBeenCalledTimes(1);
    });

    it("смена ключа хоста и ошибка без отпечатка — обычным текстом под узлом, без диалога", async () => {
      vi.mocked(api.connect)
        .mockRejectedValueOnce(hostKeyError({ key: "errors.ssh.hostKeyChanged", message: "The host key of bastion has changed" }, { kind: "hostKeyChanged" }))
        .mockRejectedValueOnce(hostKeyError({}, { fingerprint: "" }));
      const user = userEvent.setup();
      renderNode();
      await screen.findByRole("img", { name: "Не подключено" });
      await user.click(screen.getByRole("button", { name: toggleName }));
      expect(await screen.findByRole("alert")).toHaveTextContent("known_hosts");
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

      await user.click(screen.getByRole("button", { name: toggleName })); // свернуть
      await user.click(screen.getByRole("button", { name: toggleName })); // развернуть — вторая попытка
      await waitFor(() => expect(api.connect).toHaveBeenCalledTimes(2));
      expect(await screen.findByRole("alert")).toBeInTheDocument();
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
  });

  it("пока идёт подключение, показывается статус «Подключение…»", async () => {
    vi.mocked(api.connect).mockReturnValue(new Promise(() => {}));
    const user = userEvent.setup();
    renderNode();
    await screen.findByRole("img", { name: "Не подключено" });
    await user.click(screen.getByRole("button", { name: toggleName }));
    expect(await screen.findByRole("status")).toHaveTextContent("Подключение…");
  });
});
