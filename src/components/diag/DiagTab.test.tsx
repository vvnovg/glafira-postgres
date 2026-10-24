import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DiagTool } from "../../api/types";
import { useWorkspace } from "../../state/workspace";
import { connection, diagReport, member, pulseFacts, pulseRates, topology, topologyView } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { runDiagnostic: vi.fn(), getTopology: vi.fn(), listConnections: vi.fn(), connect: vi.fn() } };
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

  it("недоступное по версии показано серой строкой", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }, {
      unavailable: [{ part: "queryId", reason: { key: "diag.unavailable.version", params: { part: "queryId", version: "14" }, text: "queryId: PostgreSQL 14+" } }],
    }));
    renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
    expect(await screen.findByText("queryId: PostgreSQL 14+")).toBeInTheDocument();
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
    expect(screen.getByRole("tab", { name: "Idle in transaction", selected: true })).toBeInTheDocument();
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
    expect(await screen.findByText("pg-2: walDir: нужна роль pg_monitor")).toBeInTheDocument();
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

  it("JSON тела не пересчитывается, пока данные отчёта те же", async () => {
    const stringify = vi.spyOn(JSON, "stringify");
    try {
      // Инструмент без собственного вида (здесь матрица health): его тело — JSON отчёта.
      vi.mocked(api.runDiagnostic).mockImplementation(async () => diagReport({ kind: "health", endpoints: [], members: [] }, { member: null, followsLeader: false, observedAtMs: Date.now() }));
      const user = userEvent.setup();
      renderWithClient(<DiagTab connectionId="c" tool="health" member={null} active tabId="diag:c:health" />);
      await screen.findByText("Проблем не найдено");
      const raw = () => stringify.mock.calls.filter((c) => c[2] === 2).length;
      expect(raw()).toBe(1);
      for (let n = 2; n <= 3; n++) {
        await user.click(screen.getByRole("button", { name: "Обновить" }));
        await waitFor(() => expect(api.runDiagnostic).toHaveBeenCalledTimes(n));
      }
      // Время ответа меняется при каждом опросе, данные инструмента — нет: тело остаётся прежним.
      expect(screen.getByText(/Обновлено/)).toBeInTheDocument();
      expect(raw()).toBe(1);
    } finally {
      stringify.mockRestore();
    }
  });

  it("пауза: выбор «Пауза» оставляет данные и убирает опрос; интервал по умолчанию — 5 с (10 с у Patroni)", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "health", endpoints: [], members: [] }, { member: null, followsLeader: false }));
    const user = userEvent.setup();
    renderWithClient(<DiagTab connectionId="c" tool="health" member={null} active tabId="diag:c:health" />);
    await screen.findByRole("heading", { name: "Матрица health" });
    const interval = screen.getByRole("combobox", { name: "Обновление" });
    expect(interval).toHaveDisplayValue("10 с");
    await user.selectOptions(interval, "Пауза");
    expect(interval).toHaveDisplayValue("Пауза");
    expect(screen.getByText("Проблем не найдено")).toBeInTheDocument();
  });
});
