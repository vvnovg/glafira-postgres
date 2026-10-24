import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { connection, diagReport, member, topology, topologyView } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { runDiagnostic: vi.fn(), getTopology: vi.fn(), listConnections: vi.fn(), connect: vi.fn() } };
});

import { api } from "../../api/client";
import { DiagTab } from "./DiagTab";

describe("DiagTab", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2", { role: "replica" })] }) }));
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", kind: "patroni" })]);
  });

  it("за лидером по умолчанию; выбор узла перечитывает отчёт с этим узлом", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "sessions", rows: [], settings: { idleInTransactionSessionTimeoutMs: 0, trackActivityQuerySize: 4096 } }));
    const user = userEvent.setup();
    renderWithClient(<DiagTab connectionId="c" tool="sessions" member={null} active tabId="diag:c:sessions" />);
    expect(await screen.findByRole("heading", { name: "Сессии" })).toBeInTheDocument();
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "sessions", member: null });
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Узел" })).toHaveDisplayValue("За лидером (pg-1)"));
    await user.selectOptions(screen.getByRole("combobox", { name: "Узел" }), "pg-2");
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", { tool: "sessions", member: "pg-2" }));
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
    const { rerender } = renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
    await screen.findByText("Проблем не найдено");
    rerender(<DiagTab connectionId="c" tool="locks" member="pg-2" active tabId="diag:c:locks" />);
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", { tool: "locks", member: "pg-2" }));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Узел" })).toHaveDisplayValue("pg-2 — Реплика"));
    expect(screen.queryByText(/Узел сменился/)).not.toBeInTheDocument();
  });

  it("idleTx — вкладка поверх sessions: заголовок свой, запрос sessions", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "sessions", rows: [], settings: { idleInTransactionSessionTimeoutMs: null, trackActivityQuerySize: null } }));
    renderWithClient(<DiagTab connectionId="c" tool="idleTx" member={null} active tabId="diag:c:idleTx" />);
    expect(await screen.findByRole("heading", { name: "Idle in transaction" })).toBeInTheDocument();
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "sessions", member: null });
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
    expect(await screen.findByText("walDir: нужна роль pg_monitor")).toBeInTheDocument();
    expect(screen.getByText("pg-2:")).toBeInTheDocument();
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
