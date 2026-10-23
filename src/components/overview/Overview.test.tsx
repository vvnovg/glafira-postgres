import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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
      reconnect: vi.fn(),
    },
  };
});

import { api } from "../../api/client";
import { i18n } from "../../i18n";
import { formatTimestamp } from "../../lib/format";
import { applyState, disconnectedView } from "../../lib/useConnectionEvents";
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

  describe("подключение при открытии вкладки (спец. P1 §7)", () => {
    const disconnected = () =>
      vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "disconnected", topology: null, catalogMember: null }));

    it("неподключённое подключение подключается само, а после явного отключения предлагает подключиться кнопкой", async () => {
      const user = userEvent.setup();
      disconnected();
      vi.mocked(api.connect).mockResolvedValue(topologyView());
      const { client } = renderWithClient(<Overview connectionId="c" />);
      expect(await screen.findByText("Подключено")).toBeInTheDocument();
      expect(api.connect).toHaveBeenCalledTimes(1);
      expect(api.connect).toHaveBeenCalledWith("c");

      // Пользователь отключил подключение при открытой вкладке (событие Rust): само оно не возвращается.
      vi.mocked(api.connect).mockClear();
      act(() => applyState(client, { connectionId: "c", state: "disconnected", error: null }));
      expect(api.connect).not.toHaveBeenCalled();
      const button = await screen.findByRole("button", { name: "Подключиться" });
      expect(screen.queryByText("Подключено")).not.toBeInTheDocument();
      expect(api.connect).not.toHaveBeenCalled();

      await user.click(button);
      expect(api.connect).toHaveBeenCalledTimes(1);
      expect(await screen.findByText("Подключено")).toBeInTheDocument();
    });

    // Кэш топологии тёплый, как в приложении (дерево уже загрузило вид): состояние известно уже при монтировании,
    // и эффект StrictMode выполняется дважды, пока подключение ещё не закончилось.
    const renderStrictWithWarmCache = () => {
      const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
      client.setQueryData(["topology", "c"], disconnectedView);
      // StrictMode снаружи провайдера — как в main.tsx: так React повторяет эффекты вложенных компонентов.
      return render(
        <StrictMode>
          <QueryClientProvider client={client}>
            <Overview connectionId="c" />
          </QueryClientProvider>
        </StrictMode>,
      );
    };

    it("StrictMode (двойной монтаж) при тёплом кэше подключает один раз", async () => {
      vi.mocked(api.connect).mockResolvedValue(topologyView());
      renderStrictWithWarmCache();
      expect(await screen.findByText("Подключено")).toBeInTheDocument();
      expect(api.connect).toHaveBeenCalledTimes(1);
    });

    it("StrictMode: подключение, ещё не закончившееся к повторному монтажу, не запускается дважды", async () => {
      vi.mocked(api.connect).mockReturnValue(new Promise(() => {}));
      renderStrictWithWarmCache();
      expect(await screen.findByRole("button", { name: "Подключиться" })).toBeDisabled();
      expect(api.connect).toHaveBeenCalledTimes(1);
    });

    it.each([
      ["connecting", "Подключение…", false],
      ["connected", "Подключено", false],
      ["degraded", "Часть узлов недоступна", true],
      ["failed", "Не удалось подключиться", false],
    ] as const)("состояние %s само подключение не запускает; кнопка «Переподключить» — только при частичной недоступности", async (state, label, reopen) => {
      vi.mocked(api.getTopology).mockResolvedValue(
        topologyView({ state, error: state === "failed" ? { code: "CONNECTION_FAILED", message: "gone", retryable: true } : null }),
      );
      renderWithClient(<Overview connectionId="c" />);
      expect(await screen.findByText(label)).toBeInTheDocument();
      expect(api.connect).not.toHaveBeenCalled();
      // reconnect не сериализован с идущим connect, поэтому при connecting кнопки быть не должно.
      expect(screen.queryByRole("button", { name: "Переподключить" }) !== null).toBe(reopen);
    });

    it("часть узлов недоступна: кнопка «Переподключить» закрывает и открывает подключение заново", async () => {
      vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "degraded" }));
      vi.mocked(api.reconnect).mockResolvedValue(topologyView());
      const user = userEvent.setup();
      const { client } = renderWithClient(<Overview connectionId="c" />);
      client.setQueryData(["catalog", "c", "databases"], []);
      await user.click(await screen.findByRole("button", { name: "Переподключить" }));
      expect(api.reconnect).toHaveBeenCalledWith("c");
      expect(await screen.findByText("Подключено")).toBeInTheDocument();
      expect(api.connect).not.toHaveBeenCalled();
      expect(screen.queryByRole("button", { name: "Переподключить" })).not.toBeInTheDocument();
      // Каталог подключения перечитывается: события Disconnected при переподключении нет.
      await waitFor(() => expect(client.getQueryState(["catalog", "c", "databases"])?.isInvalidated).toBe(true));
    });

    it("ошибка переподключения показывается баннером, кнопка снова доступна", async () => {
      vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "degraded" }));
      vi.mocked(api.reconnect).mockRejectedValue({ code: "CONNECTION_FAILED", message: "reopen refused", retryable: true });
      const user = userEvent.setup();
      renderWithClient(<Overview connectionId="c" />);
      await user.click(await screen.findByRole("button", { name: "Переподключить" }));
      expect(await screen.findByText("reopen refused")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Переподключить" })).toBeEnabled();
    });

    it("сбой автоподключения: ошибка баннером, таблицы и фактов нет, кнопка повторяет попытку", async () => {
      const user = userEvent.setup();
      disconnected();
      vi.mocked(api.connect).mockRejectedValue({ code: "CONNECTION_FAILED", message: "Connection refused", retryable: true });
      renderWithClient(<Overview connectionId="c" />);
      expect(await screen.findByText("Connection refused")).toBeInTheDocument();
      expect(api.connect).toHaveBeenCalledTimes(1);
      expect(screen.queryByRole("table")).not.toBeInTheDocument();
      expect(api.getMemberFacts).not.toHaveBeenCalled();
      await user.click(screen.getByRole("button", { name: "Подключиться" }));
      expect(api.connect).toHaveBeenCalledTimes(2);
    });
  });

  it("ошибка прошлой попытки пропадает, когда подключение стало живым (например, из дерева)", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "disconnected", topology: null, catalogMember: null }));
    vi.mocked(api.connect).mockRejectedValue({ code: "CONNECTION_FAILED", message: "Connection refused", retryable: true });
    const { client } = renderWithClient(<Overview connectionId="c" />);
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

  it("несколько primary: предупреждение красным, лидеры перечислены в шапке", async () => {
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

  it("недоступный участник помечен значком, причина видна текстом без наведения; строки фактов — прочерки", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(
      topologyView({
        state: "degraded",
        topology: topology({
          members: [member("pg-1", { role: "primary" }), member("pg-2", { reachable: false, role: "unknown", state: null, lagBytes: null, error: { code: "NODE_UNAVAILABLE", message: "connection refused", retryable: true } })],
        }),
      }),
    );
    renderWithClient(<Overview connectionId="c" />);
    const mark = await screen.findByRole("img", { name: "Узел недоступен" });
    const row = mark.closest("tr")!;
    // Причина — обычный текст в ячейке (видна клавиатурным и сенсорным пользователям), а не только title значка.
    const reason = within(row).getByText("connection refused");
    expect(reason).toBeVisible();
    expect(reason).toHaveClass("member-error");
    expect(reason.closest("td")).toBe(mark.closest("td"));
    expect(within(row).getAllByText("—").length).toBeGreaterThanOrEqual(3);
    expect(within(row).queryByText("12 / 100")).not.toBeInTheDocument();
  });

  it("недоступный узел без причины: значок есть, текста причины нет", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(
      topologyView({ state: "degraded", topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2", { reachable: false, role: "unknown", error: null })] }) }),
    );
    renderWithClient(<Overview connectionId="c" />);
    const row = (await screen.findByRole("img", { name: "Узел недоступен" })).closest("tr")!;
    expect(row.querySelector(".member-error")).toBeNull();
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
    expect(api.connect).not.toHaveBeenCalled();
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
      // Роль одиночного сервера — значок в шапке (как в дереве) и ячейка таблицы.
      expect(screen.getAllByText("Primary")).toHaveLength(2);
      expect(screen.getByText("Primary", { selector: ".kind-badge" })).toBeInTheDocument();
      expect(within(screen.getByRole("table")).getByText("Primary")).toBeInTheDocument();
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

describe("Overview: шапка", () => {
  it("кластер без лидера: «Leader: —» и предупреждение, а не пустое место", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(
      topologyView({
        state: "degraded",
        topology: topology({ members: [member("pg-1"), member("pg-2")], warnings: [{ kind: "noPrimary" }] }),
      }),
    );
    renderWithClient(<Overview connectionId="c" />);
    expect(await screen.findByText("Leader: —")).toBeInTheDocument();
    expect(screen.getByText(/Нет primary/)).toBeInTheDocument();
  });

  it("список узлов без primary: «Primary: —»", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([{ ...newProfile(), id: "c", name: "pair", kind: "hostList" }]);
    vi.mocked(api.getTopology).mockResolvedValue(
      topologyView({ topology: topology({ kind: "hostList", scope: null, paused: null, members: [member("a:5432"), member("b:5432")] }) }),
    );
    renderWithClient(<Overview connectionId="c" />);
    expect(await screen.findByText("Primary: —")).toBeInTheDocument();
  });

  it("одиночный сервер: роль значком в шапке, как в дереве; неизвестная роль значка не даёт", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([{ ...newProfile(), id: "c", name: "solo", kind: "server" }]);
    const server = (role: "primary" | "replica" | "unknown") =>
      topologyView({ topology: topology({ kind: "server", scope: null, paused: null, members: [member("db:5432", { role })] }) });
    vi.mocked(api.getTopology).mockResolvedValue(server("replica"));
    const first = renderWithClient(<Overview connectionId="c" />);
    expect(await screen.findByText("Реплика", { selector: ".kind-badge" })).toBeInTheDocument();
    first.unmount();

    vi.mocked(api.getTopology).mockResolvedValue(server("unknown"));
    renderWithClient(<Overview connectionId="c" />);
    expect(await screen.findByRole("heading", { level: 2, name: "solo" })).toBeInTheDocument();
    expect(document.querySelector(".kind-badge")).toBeNull();
  });

  it("у кластера Patroni значка роли в шапке нет", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView());
    renderWithClient(<Overview connectionId="c" />);
    expect(await screen.findByText("Leader: pg-1")).toBeInTheDocument();
    expect(document.querySelector(".kind-badge")).toBeNull();
  });
});

describe("Overview: столбцы по виду подключения", () => {
  const headers = () => screen.getAllByRole("columnheader").map((h) => h.textContent);

  it("список узлов: лаг, replay и источник WAL есть всегда, а столбцов Patroni (таймлайн, теги) и версии Patroni нет", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([{ ...newProfile(), id: "c", name: "pair", kind: "hostList" }]);
    vi.mocked(api.getTopology).mockResolvedValue(
      topologyView({
        topology: topology({
          kind: "hostList",
          scope: null,
          paused: null,
          members: [
            member("a:5432", { role: "primary", state: null, timeline: null, lagBytes: null, patroniVersion: null }),
            member("b:5432", { role: "replica", state: null, timeline: null, lagBytes: null, patroniVersion: null }),
          ],
        }),
      }),
    );
    renderWithClient(<Overview connectionId="c" />);
    await screen.findByRole("columnheader", { name: "Узел" });
    expect(headers()).toEqual(["Узел", "Роль", "Состояние", "Адрес", "Лаг", "Задержка replay", "Источник WAL", "Версии", "Работает", "Соединения", "БД"]);
    expect(screen.getByText("Primary: a:5432")).toBeInTheDocument();
    expect(screen.queryByText(/Patroni \d/)).not.toBeInTheDocument();
    expect(screen.queryByText(/scope|Пауза/)).not.toBeInTheDocument();
  });

  it("кластер Patroni: полный набор столбцов", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView());
    renderWithClient(<Overview connectionId="c" />);
    await screen.findByRole("columnheader", { name: "Узел" });
    expect(headers()).toEqual([
      "Узел", "Роль", "Состояние", "Адрес", "Таймлайн", "Лаг", "Задержка replay", "Источник WAL", "Теги", "Версии", "Работает", "Соединения", "БД",
    ]);
  });
});

describe("Overview: доступность", () => {
  it("прокручиваемая таблица — именованная область, достижимая с клавиатуры", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView());
    renderWithClient(<Overview connectionId="c" />);
    const region = await screen.findByRole("region", { name: "Таблица узлов" });
    expect(region).toHaveAttribute("tabindex", "0");
    expect(within(region).getByRole("table", { name: "Узлы" })).toBeInTheDocument();
  });

  it("заголовки столбцов — scope=col, в обоих языках", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView());
    const { unmount } = renderWithClient(<Overview connectionId="c" />);
    const headers = await screen.findAllByRole("columnheader");
    expect(headers.length).toBeGreaterThan(10);
    for (const h of headers) expect(h).toHaveAttribute("scope", "col");
    try {
      await act(async () => {
        await i18n.changeLanguage("en");
      });
      expect(screen.getByRole("region", { name: "Nodes table" })).toBeInTheDocument();
    } finally {
      unmount();
      await i18n.changeLanguage("ru");
    }
  });

  it("журнал: добавление события сверху не пересоздаёт прежние строки", async () => {
    const old: TopologyEvent[] = [
      { atMs: 2_000, kind: "memberUnreachable", name: "pg-3" },
      { atMs: 1_000, kind: "leaderChanged", from: null, to: "pg-1" },
    ];
    vi.mocked(api.listTopologyEvents).mockResolvedValue(old);
    vi.mocked(api.getTopology).mockResolvedValue(topologyView());
    const { client } = renderWithClient(<Overview connectionId="c" />);
    const journal = await screen.findByRole("region", { name: "Журнал событий" });
    const before = within(journal).getAllByRole("listitem");
    expect(before).toHaveLength(2);

    act(() => client.setQueryData<TopologyEvent[]>(["topologyEvents", "c"], [{ atMs: 3_000, kind: "memberReachable", name: "pg-3" }, ...old]));
    await within(journal).findByText(/Узел pg-3 снова доступен/);
    const after = within(journal).getAllByRole("listitem");
    expect(after).toHaveLength(3);
    expect(after[0].textContent).toContain("Узел pg-3 снова доступен");
    // Те же DOM-узлы, сдвинутые вниз: ключ не зависит от позиции.
    expect(after[1]).toBe(before[0]);
    expect(after[2]).toBe(before[1]);
  });

  it("журнал: полные дубли различаются номером и не склеиваются", async () => {
    const dup: TopologyEvent = { atMs: 5_000, kind: "memberUnreachable", name: "pg-3" };
    vi.mocked(api.listTopologyEvents).mockResolvedValue([dup, dup, { atMs: 1_000, kind: "pauseChanged", paused: true }]);
    vi.mocked(api.getTopology).mockResolvedValue(topologyView());
    // React 19 сообщает о повторных ключах через console.error: склейка строк иначе прошла бы незамеченной.
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      renderWithClient(<Overview connectionId="c" />);
      const journal = await screen.findByRole("region", { name: "Журнал событий" });
      expect(within(journal).getAllByRole("listitem")).toHaveLength(3);
      expect(within(journal).getAllByText(/Узел pg-3 недоступен/)).toHaveLength(2);
      expect(errors.mock.calls.flat().join(" ")).not.toMatch(/same key/);
    } finally {
      errors.mockRestore();
    }
  });
});

describe("Overview: опрос фактов и устаревшие данные (фейковые таймеры)", () => {
  const callsFor = (name: string) => vi.mocked(api.getMemberFacts).mock.calls.filter(([, n]) => n === name).length;
  /** Продвигает часы и выполняет всё, что успело запуститься (опрос, ответы, перерисовки). */
  const tick = (ms: number) =>
    act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });
  /** Загрузка идёт цепочкой (подключения → топология → таблица → факты), каждое звено — свой тик планировщика (таймер в 0 мс срабатывает на 1 мс фейковых часов). */
  const settle = async () => {
    for (let i = 0; i < 8; i++) await tick(1);
  };
  const onePrimary = () => topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" })] }) });

  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("факты перечитываются раз в 10 секунд", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(onePrimary());
    renderWithClient(<Overview connectionId="c" />);
    await settle();
    expect(screen.getByText("12 / 100")).toBeInTheDocument();
    expect(callsFor("pg-1")).toBe(1);
    await tick(9_000);
    expect(callsFor("pg-1")).toBe(1);
    await tick(1_000);
    expect(callsFor("pg-1")).toBe(2);
    await tick(20_000);
    expect(callsFor("pg-1")).toBe(4);
  });

  it("после отказа во входе (AUTH_FAILED) узел больше не опрашивается: повтор с неверным паролем блокирует учётную запись", async () => {
    vi.mocked(api.getMemberFacts).mockRejectedValue({ code: "AUTH_FAILED", key: "errors.pg.authFailed", message: "auth failed", retryable: false });
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2")] }) }));
    renderWithClient(<Overview connectionId="c" />);
    await settle();
    expect([callsFor("pg-1"), callsFor("pg-2")]).toEqual([1, 1]);
    await tick(60_000);
    expect([callsFor("pg-1"), callsFor("pg-2")]).toEqual([1, 1]);
    // Причина видна у узла значком.
    expect(screen.getAllByRole("img", { name: /Аутентификация не прошла/ })).toHaveLength(2);
  });

  it("другие сбои чтения фактов опрос не останавливают", async () => {
    vi.mocked(api.getMemberFacts).mockRejectedValue({ code: "CONNECTION_FAILED", message: "gone", retryable: true });
    vi.mocked(api.getTopology).mockResolvedValue(onePrimary());
    renderWithClient(<Overview connectionId="c" />);
    await settle();
    await tick(20_000);
    expect(callsFor("pg-1")).toBe(3);
  });

  it("скрытая вкладка узлы по таймеру не опрашивает, видимая — опрашивает снова", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(onePrimary());
    const { rerender } = renderWithClient(<Overview connectionId="c" active={false} />);
    await settle();
    expect(callsFor("pg-1")).toBe(1);
    await tick(60_000);
    expect(callsFor("pg-1")).toBe(1);
    rerender(<Overview connectionId="c" active />);
    await tick(10_000);
    expect(callsFor("pg-1")).toBe(2);
  });

  it("опрос останавливается, когда подключение закрыто или упало", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(onePrimary());
    const { client } = renderWithClient(<Overview connectionId="c" />);
    await settle();
    await tick(10_000);
    expect(callsFor("pg-1")).toBe(2);

    // Rust сообщил о сбое: таблица остаётся последней известной, а спрашивать узлы нечем.
    act(() => applyState(client, { connectionId: "c", state: "failed", error: { code: "CONNECTION_FAILED", message: "gone", retryable: true } }));
    await settle();
    expect(screen.getByText("gone")).toBeInTheDocument();
    await tick(60_000);
    expect(callsFor("pg-1")).toBe(2);

    // Подключение вернулось — опрос возобновляется; затем его закрыли.
    act(() => applyState(client, { connectionId: "c", state: "connected", error: null }));
    await settle();
    const resumed = callsFor("pg-1");
    expect(resumed).toBeGreaterThan(2);
    await tick(10_000);
    expect(callsFor("pg-1")).toBe(resumed + 1);
    act(() => applyState(client, { connectionId: "c", state: "disconnected", error: null }));
    expect(api.connect).not.toHaveBeenCalled();
    await tick(60_000);
    expect(callsFor("pg-1")).toBe(resumed + 1);
    expect(api.connect).not.toHaveBeenCalled();
  });

  it("узел, ставший недоступным, больше не опрашивается, остальные — продолжают", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2")] }) }));
    const { client } = renderWithClient(<Overview connectionId="c" />);
    await settle();
    expect([callsFor("pg-1"), callsFor("pg-2")]).toEqual([1, 1]);

    act(() =>
      client.setQueryData(
        ["topology", "c"],
        topologyView({
          state: "degraded",
          topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2", { reachable: false, role: "unknown", error: null })] }),
        }),
      ),
    );
    await tick(30_000);
    expect(callsFor("pg-1")).toBe(4);
    expect(callsFor("pg-2")).toBe(1);
  });

  it("пока идёт подключение (connecting), узлы не опрашиваются", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "connecting" }));
    renderWithClient(<Overview connectionId="c" />);
    await settle();
    expect(screen.getByText("Подключение…")).toBeInTheDocument();
    await tick(30_000);
    expect(api.getMemberFacts).not.toHaveBeenCalled();
    expect(api.connect).not.toHaveBeenCalled();
  });

  it("время работы идёт вперёд при каждом опросе, даже если факты не изменились", async () => {
    // 4 мин 45 с на момент первого ответа; ответы равны по значению (react-query отдаёт тот же объект).
    const startedAtMs = Date.now() - (4 * 60 + 45) * 1000;
    vi.mocked(api.getMemberFacts).mockImplementation(() => Promise.resolve({ startedAtMs, connections: 12, maxConnections: 100, databases: 3 }));
    vi.mocked(api.getTopology).mockResolvedValue(onePrimary());
    renderWithClient(<Overview connectionId="c" />);
    await settle();
    expect(screen.getByText("4 минуты")).toBeInTheDocument();
    await tick(10_000);
    expect(callsFor("pg-1")).toBe(2);
    expect(screen.getByText("4 минуты")).toBeInTheDocument();
    await tick(10_000);
    expect(callsFor("pg-1")).toBe(3);
    expect(screen.getByText("5 минут")).toBeInTheDocument();
    expect(screen.queryByText("4 минуты")).not.toBeInTheDocument();
  });

  it("сбой перечитывания после успеха: числа остаются, но помечены значком с причиной и приглушены; успех снимает пометку", async () => {
    vi.mocked(api.getMemberFacts)
      .mockResolvedValueOnce({ startedAtMs: Date.now() - 3_600_000, connections: 12, maxConnections: 100, databases: 3 })
      .mockRejectedValueOnce({ code: "PG_ERROR", message: "too many clients already", retryable: true })
      .mockResolvedValue({ startedAtMs: Date.now() - 3_600_000, connections: 13, maxConnections: 100, databases: 3 });
    vi.mocked(api.getTopology).mockResolvedValue(onePrimary());
    renderWithClient(<Overview connectionId="c" />);
    await settle();
    expect(screen.getByText("12 / 100")).not.toHaveClass("stale");
    expect(screen.queryByRole("img", { name: /последние известные данные/ })).not.toBeInTheDocument();

    await tick(10_000);
    const mark = screen.getByRole("img", { name: "Показаны последние известные данные, обновить не удалось: too many clients already" });
    expect(mark).toHaveAttribute("title", mark.getAttribute("aria-label"));
    const stale = screen.getByText("12 / 100");
    expect(stale).toHaveClass("stale");
    expect(screen.getByText("3")).toHaveClass("stale");
    expect(mark.closest("td")).toHaveClass("stale");

    await tick(10_000);
    expect(screen.getByText("13 / 100")).not.toHaveClass("stale");
    expect(screen.queryByRole("img", { name: /последние известные данные/ })).not.toBeInTheDocument();
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
