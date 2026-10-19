import { act, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TopologyEvent } from "../../api/types";
import { renderWithClient } from "../../test/render";
import { member, topology, topologyView } from "../../test/fixtures";
import { newProfile } from "../connections/profile";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return {
    ...orig,
    api: {
      listConnections: vi.fn(() => Promise.resolve([{ ...newProfile(), id: "c", name: "billing", kind: "patroni" }])),
      getTopology: vi.fn(),
      listTopologyEvents: vi.fn(() => Promise.resolve([{ atMs: 1_000, kind: "leaderChanged", from: "pg-2", to: "pg-1" }])),
      getMemberFacts: vi.fn(() => Promise.resolve({ startedAtMs: Date.now() - 3_600_000, connections: 12, maxConnections: 100, databases: 3 })),
      connect: vi.fn(),
    },
  };
});

import { api } from "../../api/client";
import { i18n } from "../../i18n";
import { formatTimestamp } from "../../lib/format";
import { Overview, eventText } from "./Overview";

beforeEach(() => {
  vi.clearAllMocks();
  // clearAllMocks не сбрасывает реализации: значения по умолчанию возвращаем сами, чтобы тесты не зависели от порядка.
  vi.mocked(api.listConnections).mockResolvedValue([{ ...newProfile(), id: "c", name: "billing", kind: "patroni" }]);
  vi.mocked(api.listTopologyEvents).mockResolvedValue([{ atMs: 1_000, kind: "leaderChanged", from: "pg-2", to: "pg-1" }]);
  vi.mocked(api.getMemberFacts).mockImplementation(() =>
    Promise.resolve({ startedAtMs: Date.now() - 3_600_000, connections: 12, maxConnections: 100, databases: 3 }),
  );
});

describe("Overview", () => {
  it("участники, предупреждения, журнал и факты", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(
      topologyView({
        state: "degraded",
        topology: topology({
          paused: true,
          members: [member("pg-1", { role: "primary", state: "running", lagBytes: null }), member("pg-2", { reachable: false, role: "unknown", error: { code: "PATRONI_ERROR", message: "x", retryable: false } })],
          warnings: [{ kind: "memberUnreachable", name: "pg-2" }],
        }),
      }),
    );
    renderWithClient(<Overview connectionId="c" />);
    expect(await screen.findByText("Узел pg-2 недоступен")).toBeInTheDocument();
    expect(screen.getByText("Пауза")).toBeInTheDocument();
    expect(screen.getAllByText("Leader").length).toBeGreaterThan(0);
    expect(await screen.findByText(/pg-2 → pg-1/)).toBeInTheDocument();
    expect(await screen.findByText("12 / 100")).toBeInTheDocument();
    expect(api.getMemberFacts).toHaveBeenCalledWith("c", "pg-1");
    expect(api.getMemberFacts).not.toHaveBeenCalledWith("c", "pg-2");
  });

  it("неподключённое подключение предлагает подключиться", async () => {
    const user = userEvent.setup();
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "disconnected", topology: null, catalogMember: null }));
    vi.mocked(api.connect).mockResolvedValue(topologyView());
    renderWithClient(<Overview connectionId="c" />);
    await user.click(await screen.findByRole("button", { name: "Подключиться" }));
    expect(api.connect).toHaveBeenCalledWith("c");
  });

  it("ошибка подключения показывается баннером, а таблицы и фактов нет", async () => {
    const user = userEvent.setup();
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "disconnected", topology: null, catalogMember: null }));
    vi.mocked(api.connect).mockRejectedValue({ code: "CONNECTION_FAILED", message: "Connection refused", retryable: true });
    renderWithClient(<Overview connectionId="c" />);
    await user.click(await screen.findByRole("button", { name: "Подключиться" }));
    expect(await screen.findByText("Connection refused")).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
    expect(api.getMemberFacts).not.toHaveBeenCalled();
  });

  it("ошибка прошлой попытки пропадает, когда подключение стало живым (например, из дерева)", async () => {
    const user = userEvent.setup();
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "disconnected", topology: null, catalogMember: null }));
    vi.mocked(api.connect).mockRejectedValue({ code: "CONNECTION_FAILED", message: "Connection refused", retryable: true });
    const { client } = renderWithClient(<Overview connectionId="c" />);
    await user.click(await screen.findByRole("button", { name: "Подключиться" }));
    expect(await screen.findByText("Connection refused")).toBeInTheDocument();
    act(() => client.setQueryData(["topology", "c"], topologyView()));
    expect(await screen.findByText("Подключено")).toBeInTheDocument();
    expect(screen.queryByText("Connection refused")).not.toBeInTheDocument();
  });

  it("шапка: имя, вид, scope, лидер и состояние подключения", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView());
    renderWithClient(<Overview connectionId="c" />);
    expect(await screen.findByRole("heading", { level: 2, name: "billing" })).toBeInTheDocument();
    expect(screen.getByText("Кластер Patroni")).toBeInTheDocument();
    expect(screen.getByText("scope: demo")).toBeInTheDocument();
    expect(screen.getByText("Leader: pg-1")).toBeInTheDocument();
    expect(screen.getByText("Подключено")).toBeInTheDocument();
    expect(screen.queryByText("Пауза")).not.toBeInTheDocument();
  });

  it("подписи следуют за языком интерфейса", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ paused: true }) }));
    const { unmount } = renderWithClient(<Overview connectionId="c" />);
    expect(await screen.findByText("Пауза")).toBeInTheDocument();
    try {
      await act(async () => {
        await i18n.changeLanguage("en");
      });
      expect(screen.getByText("Paused")).toBeInTheDocument();
      expect(screen.getByText("Patroni cluster")).toBeInTheDocument();
      expect(screen.getByRole("columnheader", { name: "Role" })).toBeInTheDocument();
      expect(await screen.findByText(/Leader changed: pg-2 → pg-1/)).toBeInTheDocument();
    } finally {
      // Сначала размонтируем (иначе обратная смена языка обновит смонтированный вид вне act), затем возвращаем язык остальным тестам.
      unmount();
      await i18n.changeLanguage("ru");
    }
  });

  it("Несколько primary: предупреждение красным, лидеры перечислены в шапке", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(
      topologyView({
        topology: topology({
          members: [member("pg-1", { role: "primary" }), member("pg-2", { role: "primary" })],
          warnings: [{ kind: "multiplePrimaries", members: ["pg-1", "pg-2"] }, { kind: "noPrimary" }],
        }),
      }),
    );
    renderWithClient(<Overview connectionId="c" />);
    const split = await screen.findByText(/Несколько primary \(pg-1, pg-2\)/);
    expect(split).toHaveClass("warning-red");
    expect(screen.getByText(/Нет primary/)).not.toHaveClass("warning-red");
    expect(screen.getByText("Leader: pg-1, pg-2")).toBeInTheDocument();
  });

  it("выбранный участник выделен и помечен для скринридера", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView());
    renderWithClient(<Overview connectionId="c" member="pg-2" />);
    const selected = await screen.findByRole("row", { name: /^pg-2 / });
    expect(selected).toHaveAttribute("aria-current", "true");
    expect(selected).toHaveClass("selected");
    expect(screen.getByRole("row", { name: /^pg-1 / })).not.toHaveAttribute("aria-current");
  });

  it("недоступный участник помечен значком с текстом и ошибкой в подсказке, строки фактов — прочерки", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(
      topologyView({
        state: "degraded",
        topology: topology({
          members: [member("pg-1", { role: "primary" }), member("pg-2", { reachable: false, role: "unknown", state: null, lagBytes: null, error: { code: "NODE_UNAVAILABLE", message: "connection refused", retryable: true } })],
        }),
      }),
    );
    renderWithClient(<Overview connectionId="c" />);
    const mark = await screen.findByRole("img", { name: "Узел недоступен: connection refused" });
    expect(mark).toHaveAttribute("title", "Узел недоступен: connection refused");
    const row = mark.closest("tr")!;
    expect(within(row).getAllByText("—").length).toBeGreaterThanOrEqual(3);
    expect(within(row).queryByText("12 / 100")).not.toBeInTheDocument();
  });

  it("теги: флаги и значения, без выключенных; pending restart — подписью", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(
      topologyView({
        topology: topology({
          members: [
            member("pg-1", { role: "primary", tags: { nofailover: true, noloadbalance: false } }),
            member("pg-2", { pendingRestart: true, tags: { replicatefrom: "pg-1", failover_priority: 2, clonefrom: false } }),
          ],
        }),
      }),
    );
    renderWithClient(<Overview connectionId="c" />);
    expect(await screen.findByText("nofailover")).toBeInTheDocument();
    expect(screen.getByText("replicatefrom: pg-1, failover_priority: 2")).toBeInTheDocument();
    expect(screen.queryByText(/noloadbalance|clonefrom/)).not.toBeInTheDocument();
    const rows = screen.getAllByRole("row");
    expect(within(rows[1]).queryByText("Нужен перезапуск")).not.toBeInTheDocument();
    expect(within(rows[2]).getByText("Нужен перезапуск")).toBeInTheDocument();
  });

  it("версии PG и Patroni, DCS, лаг и задержка replay", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(
      topologyView({
        topology: topology({
          members: [
            member("pg-1", { role: "primary", lagBytes: null, dcsLastSeenMs: null }),
            member("pg-2", { lagBytes: 2048, replayDelayMs: 1500, upstream: "pg-1", timeline: 7, dcsLastSeenMs: 1_000 }),
          ],
        }),
      }),
    );
    renderWithClient(<Overview connectionId="c" />);
    const row = (await screen.findByRole("row", { name: /^pg-2 / })) as HTMLElement;
    expect(within(row).getByText("2,0 KB")).toBeInTheDocument();
    expect(within(row).getByText("1,5 s")).toBeInTheDocument();
    expect(within(row).getByText("pg-1")).toBeInTheDocument();
    expect(within(row).getByText("7")).toBeInTheDocument();
    expect(within(row).getByText("pg-2:5432")).toBeInTheDocument();
    expect(within(row).getByText(/17\.2/)).toBeInTheDocument();
    expect(within(row).getByText(/Patroni 4\.0\.4/)).toBeInTheDocument();
    expect(within(row).getByText(`DCS: ${formatTimestamp(1_000)}`)).toBeInTheDocument();
  });

  it("время работы: единицы со склонением, будущий запуск (разные часы) — 0 минут", async () => {
    const ago: Record<string, number> = { "pg-1": 5 * 60_000, "pg-2": 3 * 3_600_000, "pg-3": 10 * 86_400_000, "pg-4": -5 * 60_000 };
    vi.mocked(api.getMemberFacts).mockImplementation((_id, name) =>
      Promise.resolve({ startedAtMs: Date.now() - ago[name], connections: 1, maxConnections: 100, databases: 3 }),
    );
    vi.mocked(api.getTopology).mockResolvedValue(
      topologyView({ topology: topology({ members: ["pg-1", "pg-2", "pg-3", "pg-4"].map((n) => member(n)) }) }),
    );
    renderWithClient(<Overview connectionId="c" />);
    expect(await screen.findByText("5 минут")).toBeInTheDocument();
    expect(await screen.findByText("3 часа")).toBeInTheDocument();
    expect(await screen.findByText("10 дней")).toBeInTheDocument();
    expect(await screen.findByText("0 минут")).toBeInTheDocument();
  });

  it("факты запрашиваются только у живого подключения", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "failed", error: { code: "CONNECTION_FAILED", message: "gone", retryable: true } }));
    renderWithClient(<Overview connectionId="c" />);
    expect(await screen.findByText("gone")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Подключиться" })).toBeInTheDocument();
    expect(api.getMemberFacts).not.toHaveBeenCalled();
  });

  it("сбой чтения фактов узла помечен значком с причиной", async () => {
    vi.mocked(api.getMemberFacts).mockRejectedValue({ code: "TIMEOUT", message: "no answer", retryable: true });
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" })] }) }));
    renderWithClient(<Overview connectionId="c" />);
    expect(await screen.findByRole("img", { name: "Не удалось получить сведения об узле: no answer" })).toBeInTheDocument();
  });

  it("журнал: новые сверху; пустой журнал объясняет, почему пуст", async () => {
    const events: TopologyEvent[] = [
      { atMs: 3_000, kind: "memberUnreachable", name: "pg-3" },
      { atMs: 2_000, kind: "pauseChanged", paused: true },
      { atMs: 1_000, kind: "leaderChanged", from: null, to: "pg-1" },
    ];
    vi.mocked(api.listTopologyEvents).mockResolvedValue(events);
    vi.mocked(api.getTopology).mockResolvedValue(topologyView());
    const { unmount } = renderWithClient(<Overview connectionId="c" />);
    const journal = await screen.findByRole("region", { name: "Журнал событий" });
    const items = within(journal).getAllByRole("listitem");
    expect(items.map((li) => li.textContent)).toEqual([
      expect.stringContaining("Узел pg-3 недоступен"),
      expect.stringContaining("Кластер переведён в паузу"),
      expect.stringContaining("Смена лидера: — → pg-1"),
    ]);
    unmount();

    vi.mocked(api.listTopologyEvents).mockResolvedValue([]);
    renderWithClient(<Overview connectionId="c" />);
    expect(await screen.findByText("Пока без событий: журнал ведётся, пока подключение открыто")).toBeInTheDocument();
  });

  describe("одиночный сервер", () => {
    it("без столбцов и значков, которые бывают только у Patroni", async () => {
      vi.mocked(api.listConnections).mockResolvedValue([{ ...newProfile(), id: "c", name: "solo", kind: "server" }]);
      vi.mocked(api.getTopology).mockResolvedValue(
        topologyView({
          topology: topology({
            kind: "server",
            scope: null,
            paused: null,
            members: [member("db:5432", { role: "primary", state: null, timeline: null, lagBytes: null, patroniVersion: null, serverVersion: "17.2" })],
          }),
        }),
      );
      renderWithClient(<Overview connectionId="c" />);
      expect(await screen.findByRole("columnheader", { name: "Узел" })).toBeInTheDocument();
      for (const hidden of ["Таймлайн", "Теги", "Лаг", "Задержка replay", "Источник WAL", "Состояние"]) {
        expect(screen.queryByRole("columnheader", { name: hidden })).not.toBeInTheDocument();
      }
      for (const shown of ["Роль", "Адрес", "Версии", "Работает", "Соединения", "БД"]) {
        expect(screen.getByRole("columnheader", { name: shown })).toBeInTheDocument();
      }
      expect(screen.getByText("Primary")).toBeInTheDocument();
      expect(screen.queryByText("Leader")).not.toBeInTheDocument();
      expect(screen.queryByText(/scope/)).not.toBeInTheDocument();
      expect(screen.queryByText("Пауза")).not.toBeInTheDocument();
      expect(screen.queryByText(/^Leader:|^Primary:/)).not.toBeInTheDocument();
      expect(screen.queryByText(/Patroni \d/)).not.toBeInTheDocument();
      expect(await screen.findByText("12 / 100")).toBeInTheDocument();
      expect(api.getMemberFacts).toHaveBeenCalledWith("c", "db:5432");
    });

    it("реплика-сервер показывает лаг и источник WAL, если они известны", async () => {
      vi.mocked(api.listConnections).mockResolvedValue([{ ...newProfile(), id: "c", name: "solo", kind: "server" }]);
      vi.mocked(api.getTopology).mockResolvedValue(
        topologyView({
          topology: topology({ kind: "server", scope: null, paused: null, members: [member("db:5432", { role: "replica", upstream: "up:5432", lagBytes: 0, timeline: null, patroniVersion: null })] }),
        }),
      );
      renderWithClient(<Overview connectionId="c" />);
      expect(await screen.findByRole("columnheader", { name: "Лаг" })).toBeInTheDocument();
      expect(screen.getByRole("columnheader", { name: "Источник WAL" })).toBeInTheDocument();
      expect(screen.getByText("up:5432")).toBeInTheDocument();
    });
  });
});

describe("eventText", () => {
  const at = { atMs: 1 };
  it.each<[TopologyEvent, "patroni" | "server", string]>([
    [{ ...at, kind: "leaderChanged", from: "pg-2", to: "pg-1" }, "patroni", "Смена лидера: pg-2 → pg-1"],
    [{ ...at, kind: "leaderChanged", from: null, to: null }, "patroni", "Смена лидера: — → —"],
    [{ ...at, kind: "memberAdded", name: "pg-3" }, "patroni", "Добавлен узел pg-3"],
    [{ ...at, kind: "memberRemoved", name: "pg-3" }, "patroni", "Узел pg-3 исчез из кластера"],
    [{ ...at, kind: "memberRoleChanged", name: "pg-2", from: "replica", to: "primary" }, "patroni", "pg-2: роль Реплика → Leader"],
    [{ ...at, kind: "memberRoleChanged", name: "pg-2", from: "replica", to: "primary" }, "server", "pg-2: роль Реплика → Primary"],
    [{ ...at, kind: "memberStateChanged", name: "pg-2", from: "running", to: null }, "patroni", "pg-2: состояние running → —"],
    [{ ...at, kind: "memberUnreachable", name: "pg-2" }, "patroni", "Узел pg-2 недоступен"],
    [{ ...at, kind: "memberReachable", name: "pg-2" }, "patroni", "Узел pg-2 снова доступен"],
    [{ ...at, kind: "timelineChanged", from: 7, to: 8 }, "patroni", "Таймлайн 7 → 8"],
    [{ ...at, kind: "pauseChanged", paused: true }, "patroni", "Кластер переведён в паузу"],
    [{ ...at, kind: "pauseChanged", paused: false }, "patroni", "Пауза снята"],
  ])("%j (%s)", (event, kind, text) => {
    expect(eventText(event, kind)).toBe(text);
  });
});
