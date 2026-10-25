import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DiagTool, Finding } from "../../api/types";
import { formatTime, formatTimestamp } from "../../lib/format";
import { useWorkspace } from "../../state/workspace";
import { connection, diagReport, holder, member, pulseFacts, pulseRates, replication, sender, sessionRow, slot, slotsData, topology, topologyView, waitsData } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { runDiagnostic: vi.fn(), getTopology: vi.fn(), listConnections: vi.fn(), connect: vi.fn(), startSampler: vi.fn(), stopSession: vi.fn(), previewSessionAction: vi.fn(), executeSessionAction: vi.fn(), previewDropSlot: vi.fn(), dropSlot: vi.fn() } };
});

import { api } from "../../api/client";
import { DiagTab } from "./DiagTab";

/** Вкладка в рабочей области, как в приложении: узел берётся из спеки вкладки (`TabContent` передаёт `tab.member`). */
function Host({ tool }: { tool: DiagTool }) {
  const tab = useWorkspace((s) => s.tabs.find((t) => t.id === `diag:c:${tool}`));
  if (!tab || tab.kind !== "diag") return null;
  return <DiagTab connectionId="c" tool={tool} member={tab.member ?? null} active tabId={tab.id} />;
}

/** Открытие вкладки инструмента из другого места (ссылка находки, клетка пульса). */
const openTab = (tool: DiagTool, member: string | null) => act(() => useWorkspace.getState().open({ kind: "diag", connectionId: "c", tool, member }));

describe("DiagTab", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    useWorkspace.setState({ tabs: [], activeId: null });
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2", { role: "replica" })] }) }));
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", kind: "patroni" })]);
  });

  it("за лидером по умолчанию; выбор узла перечитывает отчёт с этим узлом", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "sessions", rows: [], settings: { idleInTransactionSessionTimeoutMs: 0, trackActivityQuerySize: 4096 } }));
    const user = userEvent.setup();
    openTab("sessions", null);
    renderWithClient(<Host tool="sessions" />);
    expect(await screen.findByRole("heading", { name: "Сессии" })).toBeInTheDocument();
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "sessions", member: null });
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Узел" })).toHaveDisplayValue("За лидером (pg-1)"));
    await user.selectOptions(screen.getByRole("combobox", { name: "Узел" }), "pg-2");
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", { tool: "sessions", member: "pg-2" }));
    // Выбор узла в шапке записывается в спеку вкладки: она — единственный источник узла.
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ kind: "diag", tool: "sessions", member: "pg-2" });
  });

  it("узел, выбранный в шапке, не мешает повторному открытию вкладки со старым узлом: оно всегда действует", async () => {
    vi.mocked(api.runDiagnostic).mockImplementation(async (_id, req) =>
      diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }, { member: req.member ?? "pg-1", followsLeader: req.member === null }),
    );
    const user = userEvent.setup();
    // Вкладка открыта ссылкой «за лидером» (member = null), пользователь выбирает pg-2 в шапке.
    openTab("locks", null);
    renderWithClient(<Host tool="locks" />);
    await screen.findByText("Проблем не найдено");
    await user.selectOptions(screen.getByRole("combobox", { name: "Узел" }), "pg-2");
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", { tool: "locks", member: "pg-2" }));
    expect(screen.getByRole("combobox", { name: "Узел" })).toHaveDisplayValue("pg-2 — Реплика");
    // Та же ссылка («за лидером», member = null) открывает вкладку снова: выбор возвращается к лидеру.
    openTab("locks", null);
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", { tool: "locks", member: null }));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Узел" })).toHaveDisplayValue("За лидером (pg-1)"));
    expect(screen.queryByText(/Узел сменился/)).not.toBeInTheDocument();
    // И с закреплённым узлом, равным прежнему значению спеки, тоже.
    openTab("locks", "pg-1");
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Узел" })).toHaveDisplayValue("pg-1 — Leader"));
    await user.selectOptions(screen.getByRole("combobox", { name: "Узел" }), "pg-2");
    openTab("locks", "pg-1");
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", { tool: "locks", member: "pg-1" }));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Узел" })).toHaveDisplayValue("pg-1 — Leader"));
  });

  it("смена лидера в режиме «за лидером» объявляется строкой", async () => {
    vi.mocked(api.runDiagnostic)
      .mockResolvedValueOnce(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }, { member: "pg-1" }))
      .mockResolvedValue(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }, { member: "pg-2" }));
    const user = userEvent.setup();
    renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
    await screen.findByRole("heading", { name: "Блокировки" });
    await user.click(screen.getByRole("button", { name: "Обновить" }));
    expect(await screen.findByText("Узел сменился: pg-1 → pg-2")).toBeInTheDocument();
  });

  it("ошибка обновления оставляет прошлые данные и помечает их устаревшими", async () => {
    vi.mocked(api.runDiagnostic)
      .mockResolvedValueOnce(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }))
      .mockRejectedValue({ code: "TIMEOUT", key: "errors.pg.statementTimeout", message: "x", retryable: true });
    const user = userEvent.setup();
    renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
    await screen.findByText("Проблем не найдено");
    await user.click(screen.getByRole("button", { name: "Обновить" }));
    expect(await screen.findByText("устарело")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(screen.getByText("Проблем не найдено")).toBeInTheDocument();
  });

  it("«Обновлено» — время получения отчёта приложением, а не часы сервера из отчёта", async () => {
    const received = 1_700_000_500_000;
    const serverClock = 1_700_000_000_000;
    const now = vi.spyOn(Date, "now").mockReturnValue(received);
    try {
      vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }, { observedAtMs: serverClock }));
      renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
      await screen.findByText("Проблем не найдено");
      expect(screen.getByText(`Обновлено ${formatTime(received)}`)).toBeInTheDocument();
      expect(screen.getByText(`Обновлено ${formatTime(received)}`)).toHaveAttribute("title", formatTimestamp(received));
      expect(screen.queryByText(`Обновлено ${formatTime(serverClock)}`)).not.toBeInTheDocument();
    } finally {
      now.mockRestore();
    }
  });

  it("недоступное по версии показано серой строкой", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }, {
      unavailable: [{ part: "queryId", reason: { key: "diag.unavailable.version", params: { part: "queryId", version: "14" }, text: "queryId: PostgreSQL 14+" } }],
    }));
    renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
    expect(await screen.findByText("Идентификатор запроса (query_id): только с PostgreSQL 14")).toBeInTheDocument();
  });

  it("закрепленный в спеке узел: запрос идёт на него, «за лидером» не помечается", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }, { member: "pg-2", followsLeader: false }));
    renderWithClient(<DiagTab connectionId="c" tool="locks" member="pg-2" active tabId="diag:c:locks" />);
    await screen.findByText("Проблем не найдено");
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "locks", member: "pg-2" });
    expect(screen.getByRole("combobox", { name: "Узел" })).toHaveDisplayValue("pg-2 — Реплика");
    expect(screen.queryByText(/Узел сменился/)).not.toBeInTheDocument();
  });

  it("повторное открытие инструмента с другим узлом (спека вкладки) переключает выбор; смена узла не объявляется как смена лидера", async () => {
    vi.mocked(api.runDiagnostic).mockImplementation(async (_id, req) =>
      diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }, { member: req.member ?? "pg-1", followsLeader: req.member === null }),
    );
    openTab("locks", null);
    renderWithClient(<Host tool="locks" />);
    await screen.findByText("Проблем не найдено");
    openTab("locks", "pg-2");
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", { tool: "locks", member: "pg-2" }));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Узел" })).toHaveDisplayValue("pg-2 — Реплика"));
    expect(screen.queryByText(/Узел сменился/)).not.toBeInTheDocument();
  });

  it("idleTx — вкладка поверх sessions: заголовок свой, запрос sessions", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "sessions", rows: [], settings: { idleInTransactionSessionTimeoutMs: null, trackActivityQuerySize: null } }));
    renderWithClient(<DiagTab connectionId="c" tool="idleTx" member={null} active tabId="diag:c:idleTx" />);
    expect(await screen.findByRole("heading", { name: "Idle in transaction" })).toBeInTheDocument();
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "sessions", member: null });
    // Данные те же, что у «Сессий», но список открывается на наборе idle in transaction.
    expect(await screen.findByRole("tab", { name: "Idle in transaction", selected: true })).toBeInTheDocument();
  });

  it("вкладка «Сессии» открывается на наборе «Все»", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "sessions", rows: [], settings: { idleInTransactionSessionTimeoutMs: null, trackActivityQuerySize: null } }));
    renderWithClient(<DiagTab connectionId="c" tool="sessions" member={null} active tabId="diag:c:sessions" />);
    expect(await screen.findByRole("tab", { name: "Все", selected: true })).toBeInTheDocument();
  });

  it("пульс: клик по клетке открывает инструмент на узле через спеку вкладки", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(
      diagReport(
        { kind: "pulse", nodes: [{ member: "pg-2", role: "replica", reachable: true, error: null, facts: pulseFacts({ used: 7, available: 100 }), rates: pulseRates() }] },
        { member: null, followsLeader: false, serverVersionNum: null, inRecovery: null },
      ),
    );
    const user = userEvent.setup();
    renderWithClient(<DiagTab connectionId="c" tool="pulse" member={null} active tabId="diag:c:pulse" />);
    await user.click(await screen.findByRole("button", { name: /7 \/ 100/ }));
    expect(useWorkspace.getState().tabs.find((t) => t.kind === "diag" && t.tool === "sessions")).toMatchObject({ connectionId: "c", member: "pg-2" });
  });

  it("инструмент кластера не выбирает узел и не передаёт его; недоступное называет узел", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(
      diagReport({ kind: "pulse", nodes: [] }, {
        member: null,
        followsLeader: false,
        serverVersionNum: null,
        inRecovery: null,
        unavailable: [{ part: "walDir", member: "pg-2", reason: { key: "diag.unavailable.privilege", params: { part: "walDir" }, text: "walDir: needs the pg_monitor role" } }],
      }),
    );
    renderWithClient(<DiagTab connectionId="c" tool="pulse" member="pg-2" active tabId="diag:c:pulse" />);
    expect(await screen.findByRole("heading", { name: "Пульс" })).toBeInTheDocument();
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "pulse", member: null });
    expect(screen.queryByRole("combobox", { name: "Узел" })).not.toBeInTheDocument();
    expect(await screen.findByText("pg-2: Размер pg_wal: нужна роль pg_monitor")).toBeInTheDocument();
  });

  it("у одиночного сервера выбора узла нет", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ kind: "server", members: [member("db", { role: "primary" })] }) }));
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }, { member: "db" }));
    renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
    await screen.findByText("Проблем не найдено");
    expect(screen.queryByRole("combobox", { name: "Узел" })).not.toBeInTheDocument();
  });

  it("окно профиля ожиданий по умолчанию — 15 минут", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }));
    renderWithClient(<DiagTab connectionId="c" tool="waits" member={null} active tabId="diag:c:waits" />);
    await screen.findByText("Проблем не найдено");
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "waits", member: null, windowS: 900 });
  });

  it("закрытое подключение: запрос не уходит, кнопка «Подключиться» открывает подключение", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "disconnected", topology: null, catalogMember: null }));
    vi.mocked(api.connect).mockResolvedValue(topologyView());
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }));
    const user = userEvent.setup();
    renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
    expect(await screen.findByText("Подключение закрыто")).toBeInTheDocument();
    expect(api.runDiagnostic).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Подключиться" }));
    expect(api.connect).toHaveBeenCalledWith("c");
    await screen.findByText("Проблем не найдено");
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);
  });

  it("профиль ожиданий: окно по умолчанию 15 минут, выбор окна перечитывает отчёт с ним и не трогает подписку на сэмплер", async () => {
    vi.mocked(api.runDiagnostic).mockImplementation(async (_id, req) => diagReport(waitsData({ windowS: req.windowS ?? 0, totalSamples: 1, topEvents: [{ class: "io", event: "DataFileRead", samples: 1, share: 1 }] }), { member: "pg-1" }));
    vi.mocked(api.startSampler).mockResolvedValue("s-1");
    vi.mocked(api.stopSession).mockResolvedValue(true);
    const user = userEvent.setup();
    renderWithClient(<DiagTab connectionId="c" tool="waits" member={null} active tabId="diag:c:waits" />);
    expect(await screen.findByRole("table", { name: "Топ ожиданий" })).toBeInTheDocument();
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "waits", member: null, windowS: 900 });
    const window = screen.getByRole("combobox", { name: "Окно" });
    expect(within(window).getAllByRole("option").map((o) => o.textContent)).toEqual(["5 мин", "15 мин", "60 мин"]);
    expect(window).toHaveDisplayValue("15 мин");
    await waitFor(() => expect(api.startSampler).toHaveBeenCalledWith("c", "pg-1", expect.any(Function)));
    await user.selectOptions(window, "60 мин");
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", { tool: "waits", member: null, windowS: 3600 }));
    await user.selectOptions(window, "5 мин");
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", { tool: "waits", member: null, windowS: 300 }));
    // Смена окна — тот же узел: сэмплер не переподписывается.
    expect(api.startSampler).toHaveBeenCalledTimes(1);
    expect(api.stopSession).not.toHaveBeenCalled();
  });

  it("выбор окна есть только у профиля ожиданий", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }));
    renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
    await screen.findByText("Проблем не найдено");
    expect(screen.queryByRole("combobox", { name: "Окно" })).toBeNull();
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "locks", member: null });
  });

  it("сэмплер держится, пока вкладка смонтирована, и скрытая вкладка его не снимает; закрытие вкладки — снимает", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(waitsData(), { member: "pg-1" }));
    vi.mocked(api.startSampler).mockResolvedValue("s-1");
    vi.mocked(api.stopSession).mockResolvedValue(true);
    const { rerender, unmount } = renderWithClient(<DiagTab connectionId="c" tool="waits" member={null} active tabId="diag:c:waits" />);
    await waitFor(() => expect(api.startSampler).toHaveBeenCalledTimes(1));
    rerender(<DiagTab connectionId="c" tool="waits" member={null} active={false} tabId="diag:c:waits" />);
    expect(api.stopSession).not.toHaveBeenCalled();
    unmount();
    await waitFor(() => expect(api.stopSession).toHaveBeenCalledWith("s-1"));
  });

  it("тела инструментов задач 12: горизонт, реплики, слоты, готовность к failover и матрица health — свои виды, а не JSON", async () => {
    const reports: Record<string, ReturnType<typeof diagReport>> = {
      horizon: diagReport({ kind: "horizon", holders: [holder({ kind: "slot", name: "forgotten", xminAge: 5 })], hotStandbyFeedback: false }),
      replication: diagReport({ kind: "replication", ...replication({ senders: [sender({ application: "pg-2", member: "pg-2" })] }) }),
      slots: diagReport(slotsData({ slots: [slot({ name: "pg_2" })] })),
      failover: diagReport({ kind: "failover", paused: false, readyCount: 1, config: { maximumLagOnFailover: 1048576, synchronousMode: "off", synchronousModeStrict: false, synchronousNodeCount: null, ttl: 30, loopWait: 10, retryTimeout: 10, permanentSlots: [] }, candidates: [{ member: "pg-2", ready: true, reasons: [], lagBytes: 0, pendingRestart: false }] }, { member: null }),
      health: diagReport({ kind: "health", endpoints: ["primary"], members: [{ member: "pg-1", apiUrl: "http://pg-1:8008", cells: [{ endpoint: "primary", status: 200, error: null, grade: "ok" }] }] }, { member: null }),
    };
    const tables: Record<string, string> = { horizon: "Держатели горизонта xmin", replication: "Отправители WAL", slots: "Слоты репликации", failover: "Кандидаты на роль лидера", health: "Матрица health" };
    for (const tool of ["horizon", "replication", "slots", "failover", "health"] as const) {
      vi.mocked(api.runDiagnostic).mockResolvedValue(reports[tool]);
      const { unmount } = renderWithClient(<DiagTab connectionId="c" tool={tool} member={null} active tabId={`diag:c:${tool}`} />);
      expect(await screen.findByRole("table", { name: tables[tool] })).toBeInTheDocument();
      unmount();
    }
  });

  it("пауза: выбор «Пауза» оставляет данные и убирает опрос; интервал по умолчанию — 5 с (10 с у Patroni)", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "health", endpoints: [], members: [] }, { member: null, followsLeader: false }));
    const user = userEvent.setup();
    renderWithClient(<DiagTab connectionId="c" tool="health" member={null} active tabId="diag:c:health" />);
    await screen.findByRole("heading", { name: "Матрица health" });
    const interval = screen.getByRole("combobox", { name: "Интервал" });
    expect(interval).toHaveDisplayValue("10 с");
    await user.selectOptions(interval, "Пауза");
    expect(interval).toHaveDisplayValue("Пауза");
    expect(screen.getByText("Проблем не найдено")).toBeInTheDocument();
  });

  describe("действия над сессиями", () => {
    const settings = { idleInTransactionSessionTimeoutMs: null, trackActivityQuerySize: null };
    const sessionsReport = (findings: Finding[] = []) =>
      diagReport({ kind: "sessions", rows: [sessionRow({ pid: 5, state: "idle in transaction", stateAgeS: 900, backendStartUs: 50 })], settings }, { findings });
    const previewRow = (pid: number, backendStartUs: number) => ({
      session: { pid, backendStartUs, user: "app", database: "orders", application: "api", client: null, state: "idle in transaction", backendType: "client backend", xactAgeS: 900, stateAgeS: 900, query: "update t" },
      allowed: true,
      reason: null,
    });

    it("кнопка в панели сессии открывает диалог на узле отчёта; закрытие убирает его", async () => {
      vi.mocked(api.runDiagnostic).mockResolvedValue(sessionsReport());
      vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-1", rows: [previewRow(5, 50)] });
      const user = userEvent.setup();
      renderWithClient(<DiagTab connectionId="c" tool="sessions" member={null} active tabId="diag:c:sessions" />);
      await user.click(await screen.findByRole("row", { name: /5/ }));
      await user.click(screen.getByRole("button", { name: "Завершить сессию…" }));
      expect(await screen.findByRole("table", { name: "Сессии для завершения" })).toBeInTheDocument();
      expect(api.previewSessionAction).toHaveBeenCalledWith("c", { member: "pg-1", kind: "terminate", target: { kind: "selected", sessions: [{ pid: 5, backendStartUs: 50 }] } });
      await user.click(screen.getByRole("button", { name: "Отмена" }));
      expect(screen.queryByRole("table", { name: "Сессии для завершения" })).toBeNull();
    });

    it("порог в наборе idle in transaction открывает диалог с критерием; после выполнения отчёт и журнал перечитываются", async () => {
      vi.mocked(api.runDiagnostic).mockResolvedValue(sessionsReport());
      vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-1", rows: [previewRow(5, 50)] });
      vi.mocked(api.executeSessionAction).mockResolvedValue({ member: "pg-1", results: [{ pid: 5, backendStartUs: 50, result: "terminated" }], outcome: "ok", journalWritten: true });
      const user = userEvent.setup();
      const { client } = renderWithClient(<DiagTab connectionId="c" tool="idleTx" member={null} active tabId="diag:c:idleTx" />);
      const input = await screen.findByRole("spinbutton", { name: "Старше, мин" });
      await user.clear(input);
      await user.type(input, "10");
      await user.click(screen.getByRole("button", { name: "Завершить idle in transaction старше 10 мин…" }));
      expect(await screen.findByText("Сессии в состоянии idle in transaction дольше 10 мин")).toBeInTheDocument();
      expect(api.previewSessionAction).toHaveBeenCalledWith("c", { member: "pg-1", kind: "terminate", target: { kind: "idleInTransaction", olderThanS: 600 } });
      const spy = vi.spyOn(client, "invalidateQueries");
      const before = vi.mocked(api.runDiagnostic).mock.calls.length;
      await user.click(screen.getByRole("button", { name: "Завершить сессию" }));
      expect(await screen.findByRole("table", { name: "Результат" })).toBeInTheDocument();
      expect(spy).toHaveBeenCalledWith({ queryKey: ["actions", "c"] });
      await waitFor(() => expect(vi.mocked(api.runDiagnostic).mock.calls.length).toBeGreaterThan(before));
    });

    it("блокировки: кнопка у корня открывает диалог rootBlocker", async () => {
      const lockSession = { pid: 1, backendStartUs: 10, user: "app", application: "api", database: "orders", state: "active", query: "update", xactAgeS: 100, queryAgeS: 10 };
      vi.mocked(api.runDiagnostic).mockResolvedValue(
        diagReport({ kind: "locks", roots: [{ session: lockSession, lock: null, alsoBlockedBy: [], victims: 1, children: [] }], cycles: [], ddlQueue: [] }),
      );
      vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-1", rows: [previewRow(1, 10)] });
      const user = userEvent.setup();
      renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
      await user.click(await screen.findByRole("button", { name: "Завершить корневой блокирующий…" }));
      expect(await screen.findByRole("table", { name: "Сессии для завершения" })).toBeInTheDocument();
      expect(api.previewSessionAction).toHaveBeenCalledWith("c", { member: "pg-1", kind: "terminate", target: { kind: "rootBlocker", pid: 1, backendStartUs: 10 } });
    });

    it("кнопка находки открывает диалог; узел берётся из ссылки находки", async () => {
      const finding: Finding = {
        id: "idleInTransaction",
        severity: "critical",
        member: "pg-2",
        target: null,
        params: { maxS: 2000, count: 1 },
        links: [{ kind: "action", action: { type: "terminateIdleInTransaction", olderThanS: 300 }, member: "pg-2" }],
      };
      vi.mocked(api.runDiagnostic).mockResolvedValue(sessionsReport([finding]));
      vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-2", rows: [previewRow(5, 50)] });
      const user = userEvent.setup();
      renderWithClient(<DiagTab connectionId="c" tool="sessions" member={null} active tabId="diag:c:sessions" />);
      await user.click(await screen.findByRole("button", { name: "Завершить idle in transaction старше 5 мин" }));
      expect(await screen.findByRole("table", { name: "Сессии для завершения" })).toBeInTheDocument();
      expect(api.previewSessionAction).toHaveBeenCalledWith("c", { member: "pg-2", kind: "terminate", target: { kind: "idleInTransaction", olderThanS: 300 } });
    });

    it("кнопка находки «корневой блокирующий» — одна сессия по паре из ссылки", async () => {
      const finding: Finding = {
        id: "rootBlocker",
        severity: "warning",
        member: "pg-1",
        target: { kind: "session", pid: 7, backendStartUs: 70 },
        params: { count: 1, maxS: 20 },
        links: [{ kind: "action", action: { type: "terminateRootBlocker", pid: 7, backendStartUs: 70 }, member: null }],
      };
      vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }, { findings: [finding] }));
      vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-1", rows: [previewRow(7, 70)] });
      const user = userEvent.setup();
      renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
      // Ссылка без узла — узел отчёта.
      await user.click(await screen.findByRole("button", { name: "Завершить сессию 7" }));
      await screen.findByRole("table", { name: "Сессии для завершения" });
      expect(api.previewSessionAction).toHaveBeenCalledWith("c", { member: "pg-1", kind: "terminate", target: { kind: "rootBlocker", pid: 7, backendStartUs: 70 } });
    });

    it("отчёт без узла (member: null): кнопок сессий, порога и корня блокировок нет; кнопка находки — только если узел назван в ссылке", async () => {
      const link = (member: string | null) => ({ kind: "action" as const, action: { type: "terminateIdleInTransaction" as const, olderThanS: 300 }, member });
      const idle = (member: string | null): Finding => ({ id: "idleInTransaction", severity: "critical", member, target: null, params: { maxS: 2000, count: 1 }, links: [link(member)] });
      vi.mocked(api.runDiagnostic).mockResolvedValue({ ...sessionsReport([idle(null)]), member: null });
      const user = userEvent.setup();
      const { unmount } = renderWithClient(<DiagTab connectionId="c" tool="idleTx" member={null} active tabId="diag:c:idleTx" />);
      await user.click(await screen.findByRole("row", { name: /5/ }));
      expect(screen.getByRole("region", { name: "Сессия 5" })).toBeInTheDocument();
      // Ни у отчёта, ни у ссылки узла нет: выполнять негде, кнопок нет (а не кнопка, которая молча ничего не делает).
      expect(screen.queryByRole("button", { name: /Завершить|Отменить запрос/ })).toBeNull();
      expect(screen.queryByRole("spinbutton")).toBeNull();
      unmount();
      // Отчёт кластера: узла нет у отчёта, но ссылка находки называет его — кнопка есть и действует на узле ссылки.
      vi.mocked(api.runDiagnostic).mockResolvedValue({ ...sessionsReport([idle("pg-2")]), member: null });
      vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-2", rows: [previewRow(5, 50)] });
      renderWithClient(<DiagTab connectionId="c" tool="idleTx" member={null} active tabId="diag:c:idleTx" />);
      await user.click(await screen.findByRole("button", { name: "Завершить idle in transaction старше 5 мин" }));
      expect(api.previewSessionAction).toHaveBeenCalledWith("c", { member: "pg-2", kind: "terminate", target: { kind: "idleInTransaction", olderThanS: 300 } });
    });

    it("отчёт без узла: у корня дерева блокировок кнопки нет", async () => {
      const lockSession = { pid: 1, backendStartUs: 10, user: "app", application: "api", database: "orders", state: "active", query: "update", xactAgeS: 100, queryAgeS: 10 };
      vi.mocked(api.runDiagnostic).mockResolvedValue(
        diagReport({ kind: "locks", roots: [{ session: lockSession, lock: null, alsoBlockedBy: [], victims: 1, children: [] }], cycles: [], ddlQueue: [] }, { member: null }),
      );
      renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
      await screen.findByText("Дерево блокировок");
      expect(screen.queryByRole("button", { name: "Завершить корневой блокирующий…" })).toBeNull();
    });

    it("повторный выбор того же действия открывает диалог заново: предпросмотр читается снова, результат прошлого не остаётся", async () => {
      vi.mocked(api.runDiagnostic).mockResolvedValue(sessionsReport());
      vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-1", rows: [previewRow(5, 50)] });
      vi.mocked(api.executeSessionAction).mockResolvedValue({ member: "pg-1", results: [{ pid: 5, backendStartUs: 50, result: "terminated" }], outcome: "ok", journalWritten: true });
      const user = userEvent.setup();
      renderWithClient(<DiagTab connectionId="c" tool="sessions" member={null} active tabId="diag:c:sessions" />);
      await user.click(await screen.findByRole("row", { name: /5/ }));
      const open = () => user.click(screen.getByRole("button", { name: "Завершить сессию…" }));
      await open();
      await user.click(await screen.findByRole("button", { name: "Завершить сессию" }));
      expect(await screen.findByRole("table", { name: "Результат" })).toBeInTheDocument();
      expect(api.previewSessionAction).toHaveBeenCalledTimes(1);
      // То же самое действие по той же кнопке: старая таблица результата уходит, предпросмотр читается заново.
      await open();
      expect(await screen.findByRole("table", { name: "Сессии для завершения" })).toBeInTheDocument();
      expect(screen.queryByRole("table", { name: "Результат" })).toBeNull();
      await waitFor(() => expect(api.previewSessionAction).toHaveBeenCalledTimes(2));
      // И повторное нажатие при открытом предпросмотре — тоже новое чтение (список не берётся из кэша прошлого диалога).
      await open();
      await waitFor(() => expect(api.previewSessionAction).toHaveBeenCalledTimes(3));
      expect(await screen.findByRole("table", { name: "Сессии для завершения" })).toBeInTheDocument();
    });

    it("подключение только для чтения: ни кнопок сессий, ни порога, ни кнопок находок", async () => {
      vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", kind: "patroni", readOnly: true })]);
      const finding: Finding = {
        id: "idleInTransaction",
        severity: "critical",
        member: "pg-1",
        target: null,
        params: { maxS: 2000, count: 1 },
        links: [{ kind: "action", action: { type: "terminateIdleInTransaction", olderThanS: 300 }, member: "pg-1" }],
      };
      vi.mocked(api.runDiagnostic).mockResolvedValue(sessionsReport([finding]));
      const user = userEvent.setup();
      renderWithClient(<DiagTab connectionId="c" tool="idleTx" member={null} active tabId="diag:c:idleTx" />);
      await user.click(await screen.findByRole("row", { name: /5/ }));
      expect(screen.getByRole("region", { name: "Сессия 5" })).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: /Завершить|Отменить запрос/ })).toBeNull();
      expect(screen.queryByRole("spinbutton")).toBeNull();
    });

    it("профиль подключения ещё не загружен: действий нет", async () => {
      vi.mocked(api.listConnections).mockReturnValue(new Promise(() => {}));
      vi.mocked(api.runDiagnostic).mockResolvedValue(sessionsReport());
      const user = userEvent.setup();
      renderWithClient(<DiagTab connectionId="c" tool="idleTx" member={null} active tabId="diag:c:idleTx" />);
      await user.click(await screen.findByRole("row", { name: /5/ }));
      expect(screen.queryByRole("button", { name: /Завершить|Отменить запрос/ })).toBeNull();
    });
  });

  describe("удаление слота", () => {
    const slotsReport = (patch: Partial<Parameters<typeof diagReport>[1]> = {}) =>
      diagReport(slotsData({ slots: [slot({ name: "dead", active: false }), slot({ name: "other", active: false })], canDropSlots: true }), patch);
    const dropPreview = (name: string) => ({ member: "pg-1", slot: slot({ name, active: false, walStatus: "extended" }) });

    it("кнопка слота открывает диалог на узле отчёта; закрытие убирает его; после удаления отчёт и журнал перечитываются", async () => {
      vi.mocked(api.runDiagnostic).mockResolvedValue(slotsReport());
      vi.mocked(api.previewDropSlot).mockImplementation(async (_id, req) => dropPreview(req.slot));
      vi.mocked(api.dropSlot).mockResolvedValue({ journalWritten: true });
      const user = userEvent.setup();
      const { client } = renderWithClient(<DiagTab connectionId="c" tool="slots" member={null} active tabId="diag:c:slots" />);
      await user.click(within(await screen.findByRole("row", { name: /dead/ })).getByRole("button", { name: "Удалить…" }));
      expect(await screen.findByRole("group", { name: "Удалить слот dead?" })).toBeInTheDocument();
      expect(api.previewDropSlot).toHaveBeenCalledWith("c", { member: "pg-1", slot: "dead" });
      await user.click(screen.getByRole("button", { name: "Отмена" }));
      expect(screen.queryByRole("group", { name: "Удалить слот dead?" })).toBeNull();
      // Повторное открытие того же слота читает его заново.
      await user.click(within(screen.getByRole("row", { name: /dead/ })).getByRole("button", { name: "Удалить…" }));
      await user.type(await screen.findByLabelText("Подтверждение: имя слота"), "dead");
      expect(api.previewDropSlot).toHaveBeenCalledTimes(2);
      const spy = vi.spyOn(client, "invalidateQueries");
      const before = vi.mocked(api.runDiagnostic).mock.calls.length;
      await user.click(screen.getByRole("button", { name: "Удалить слот" }));
      expect(await screen.findByText("Слот удалён")).toBeInTheDocument();
      expect(api.dropSlot).toHaveBeenCalledWith("c", { member: "pg-1", slot: "dead" }, { confirm: null, typedName: "dead" });
      expect(spy).toHaveBeenCalledWith({ queryKey: ["actions", "c"] });
      await waitFor(() => expect(vi.mocked(api.runDiagnostic).mock.calls.length).toBeGreaterThan(before));
    });

    it("повторное нажатие той же кнопки при открытом диалоге открывает его заново: слот читается снова, а не берётся из кэша", async () => {
      vi.mocked(api.runDiagnostic).mockResolvedValue(slotsReport());
      vi.mocked(api.previewDropSlot).mockImplementation(async (_id, req) => dropPreview(req.slot));
      const user = userEvent.setup();
      renderWithClient(<DiagTab connectionId="c" tool="slots" member={null} active tabId="diag:c:slots" />);
      const open = async () => user.click(within(await screen.findByRole("row", { name: /dead/ })).getByRole("button", { name: "Удалить…" }));
      await open();
      await user.type(await screen.findByLabelText("Подтверждение: имя слота"), "dead");
      await open();
      await waitFor(() => expect(api.previewDropSlot).toHaveBeenCalledTimes(2));
      // Новый диалог: введённое имя прошлого не остаётся.
      expect(await screen.findByLabelText("Подтверждение: имя слота")).toHaveValue("");
    });

    it("узел диалога — узел отчёта на момент открытия, а не тот, что придёт позже", async () => {
      vi.mocked(api.runDiagnostic)
        .mockResolvedValueOnce(slotsReport({ member: "pg-1" }))
        .mockResolvedValue(slotsReport({ member: "pg-2" }));
      vi.mocked(api.previewDropSlot).mockResolvedValue({ member: "pg-1", slot: slot({ name: "dead", active: false }) });
      const user = userEvent.setup();
      renderWithClient(<DiagTab connectionId="c" tool="slots" member={null} active tabId="diag:c:slots" />);
      await user.click(within(await screen.findByRole("row", { name: /dead/ })).getByRole("button", { name: "Удалить…" }));
      await screen.findByRole("group", { name: "Удалить слот dead?" });
      await user.click(screen.getByRole("button", { name: "Обновить" }));
      await screen.findByText("Узел сменился: pg-1 → pg-2");
      expect(api.previewDropSlot).toHaveBeenCalledTimes(1);
      expect(api.previewDropSlot).toHaveBeenCalledWith("c", { member: "pg-1", slot: "dead" });
    });

    it("пока слот удаляется, кнопка другого слота диалог не подменяет", async () => {
      vi.mocked(api.runDiagnostic).mockResolvedValue(slotsReport());
      vi.mocked(api.previewDropSlot).mockImplementation(async (_id, req) => dropPreview(req.slot));
      vi.mocked(api.dropSlot).mockReturnValue(new Promise(() => {}));
      const user = userEvent.setup();
      renderWithClient(<DiagTab connectionId="c" tool="slots" member={null} active tabId="diag:c:slots" />);
      const dropOf = async (name: string) => within(await screen.findByRole("row", { name: new RegExp(name) })).getByRole("button", { name: "Удалить…" });
      await user.click(await dropOf("dead"));
      await user.type(await screen.findByLabelText("Подтверждение: имя слота"), "dead");
      await user.click(screen.getByRole("button", { name: "Удалить слот" }));
      await waitFor(() => expect(api.dropSlot).toHaveBeenCalled());
      await user.click(await dropOf("other"));
      expect(screen.getByRole("group", { name: "Удалить слот dead?" })).toBeInTheDocument();
      expect(screen.queryByRole("group", { name: "Удалить слот other?" })).toBeNull();
      expect(api.previewDropSlot).toHaveBeenCalledTimes(1);
    });

    it("без права REPLICATION кнопка неактивна и диалог не открывается", async () => {
      vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(slotsData({ slots: [slot({ name: "dead", active: false })], canDropSlots: false })));
      const user = userEvent.setup();
      renderWithClient(<DiagTab connectionId="c" tool="slots" member={null} active tabId="diag:c:slots" />);
      const button = within(await screen.findByRole("row", { name: /dead/ })).getByRole("button", { name: "Удалить…" });
      expect(button).toBeDisabled();
      await user.click(button);
      expect(api.previewDropSlot).not.toHaveBeenCalled();
    });

    it("подключение только для чтения, профиль не загружен, отчёт без узла: кнопок нет", async () => {
      vi.mocked(api.runDiagnostic).mockResolvedValue(slotsReport());
      vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", kind: "patroni", readOnly: true })]);
      const first = renderWithClient(<DiagTab connectionId="c" tool="slots" member={null} active tabId="diag:c:slots" />);
      await screen.findByRole("row", { name: /dead/ });
      await waitFor(() => expect(api.listConnections).toHaveBeenCalled());
      expect(screen.queryByRole("button", { name: "Удалить…" })).toBeNull();
      first.unmount();

      vi.mocked(api.listConnections).mockReturnValue(new Promise(() => {}));
      const second = renderWithClient(<DiagTab connectionId="c" tool="slots" member={null} active tabId="diag:c:slots" />);
      await screen.findByRole("row", { name: /dead/ });
      expect(screen.queryByRole("button", { name: "Удалить…" })).toBeNull();
      second.unmount();

      vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", kind: "patroni" })]);
      vi.mocked(api.runDiagnostic).mockResolvedValue(slotsReport({ member: null }));
      renderWithClient(<DiagTab connectionId="c" tool="slots" member={null} active tabId="diag:c:slots" />);
      await screen.findByRole("row", { name: /dead/ });
      await waitFor(() => expect(api.listConnections).toHaveBeenCalledTimes(3));
      expect(screen.queryByRole("button", { name: "Удалить…" })).toBeNull();
    });
  });
});
