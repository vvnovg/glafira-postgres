import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { QueriesData, QueryStat, StatementsEvent } from "../../api/types";
import { useWorkspace } from "../../state/workspace";
import { diagReport } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { runDiagnostic: vi.fn(), queryTexts: vi.fn(), startStatementsSampler: vi.fn(), stopSession: vi.fn() } };
});

import { api } from "../../api/client";
import { QueriesView } from "./QueriesView";

let emit: (e: StatementsEvent) => void;

function row(id: string, patch: Partial<QueryStat> = {}): QueryStat {
  return {
    key: { queryId: id, dbid: 5, userid: 10, toplevel: true }, datname: "perf", rolname: "app",
    calls: 100, totalExecMs: 1000, meanExecMs: 10, minExecMs: 1, maxExecMs: 50, stddevExecMs: 5, rows: 100, rowsPerCall: 1,
    plans: 0, totalPlanMs: 0, sharedHit: 900, sharedRead: 100, sharedDirtied: 0, sharedWritten: 0, localHit: 0, localRead: 0, localDirtied: 0, localWritten: 0,
    tempRead: 0, tempWritten: 0, ioReadMs: 0, ioWriteMs: 0, tempIoMs: null, walRecords: 0, walFpi: 0, walBytes: 0,
    jitFunctions: null, jitMs: null, parallelPlanned: null, parallelLaunched: null, sharePct: 50, hitRatio: 0.9, text: null, textTruncated: false, ...patch,
  };
}

function data(patch: Partial<QueriesData> = {}): { kind: "queries" } & QueriesData {
  return {
    kind: "queries", mode: { kind: "sinceReset" }, coverageS: null, ticks: null,
    pgss: { version: "1.11", max: 5000, trackPlanning: true, statsResetMs: 1_700_000_000_000, dealloc: 0 },
    totals: { calls: 200, totalExecMs: 2000, rows: 200 }, rows: [row("1"), row("2", { totalExecMs: 500, sharePct: 25, text: "select 2" })], ...patch,
  };
}

describe("QueriesView", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    useWorkspace.setState({ tabs: [], activeId: null });
    emit = () => {};
    vi.mocked(api.startStatementsSampler).mockImplementation(async (_id, _m, onEvent) => { emit = onEvent; return "s-1"; });
    vi.mocked(api.stopSession).mockResolvedValue(true);
    vi.mocked(api.queryTexts).mockResolvedValue([{ key: { queryId: "1", dbid: 5, userid: 10, toplevel: true }, text: "select 1 from t", truncated: false }]);
  });

  const view = (member: string | null = null) => <QueriesView connectionId="c" member={member} active live members={null} kind="server" onSelectMember={() => {}} />;

  it("«с момента сброса»: строки, тексты подгружаются по ключам без текста, подвал с версией pgss", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data(), { member: "pg-1" }));
    renderWithClient(view());
    expect(await screen.findByText("select 2")).toBeInTheDocument();
    await waitFor(() => expect(api.queryTexts).toHaveBeenCalledWith("c", "pg-1", [{ queryId: "1", dbid: 5, userid: 10, toplevel: true }]));
    expect(await screen.findByText("select 1 from t")).toBeInTheDocument();
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "queries", member: null, queries: { mode: { kind: "sinceReset" }, sort: "totalTime", limit: 50, database: null, toplevelOnly: true } });
    expect(screen.getByText(/pg_stat_statements 1\.11/)).toBeInTheDocument();
    expect(api.startStatementsSampler).toHaveBeenCalledWith("c", null, expect.any(Function));
  });

  it("окно: покрытие из отчёта, обновление по тику сэмплера, stddev недоступен", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data({ mode: { kind: "window", s: 300 }, coverageS: 120, ticks: 2, rows: [row("1", { stddevExecMs: null, maxExecMs: null })] }), { member: "pg-1" }));
    const user = userEvent.setup();
    renderWithClient(view());
    await screen.findByRole("table", { name: "Запросы" });
    await user.selectOptions(screen.getByRole("combobox", { name: "Режим" }), "window:300");
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", expect.objectContaining({ queries: expect.objectContaining({ mode: { kind: "window", s: 300 } }) })));
    expect(await screen.findByText("Накоплено 2 из 5 мин")).toBeInTheDocument();
    const calls = vi.mocked(api.runDiagnostic).mock.calls.length;
    act(() => emit({ kind: "tick", atMs: 5, coverageS: 180, ticks: 3, changed: 4 }));
    await waitFor(() => expect(vi.mocked(api.runDiagnostic).mock.calls.length).toBeGreaterThan(calls));
    expect(screen.getByRole("columnheader", { name: /Stddev/ }).querySelector("button")).toBeDisabled();
    expect(screen.getByRole("columnheader", { name: /Среднее/ }).querySelector("button")).toBeEnabled();
  });

  it("клик по заголовку меняет сортировку; клик по строке открывает вкладку запроса с узлом отчёта", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data(), { member: "pg-1" }));
    const user = userEvent.setup();
    renderWithClient(view());
    const table = await screen.findByRole("table", { name: "Запросы" });
    await user.click(within(screen.getByRole("columnheader", { name: /Вызовов/ })).getByRole("button"));
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", expect.objectContaining({ queries: expect.objectContaining({ sort: "calls" }) })));
    await user.click(within(table).getByRole("button", { name: /select 2/ }));
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ kind: "query", connectionId: "c", member: "pg-1", database: "perf", queryId: "2", title: "select 2" });
  });

  it("«Открыть консоль EXPLAIN» открывает единственную консоль подключения: на закреплённом узле или «за лидером»", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data(), { member: "pg-1" }));
    const user = userEvent.setup();
    const { rerender } = renderWithClient(view("pg-2"));
    await screen.findByRole("table", { name: "Запросы" });
    await user.click(screen.getByRole("button", { name: "Открыть консоль EXPLAIN" }));
    expect(useWorkspace.getState().tabs).toEqual([{ kind: "explain", connectionId: "c", member: "pg-2", id: "explain:c" }]);
    expect(useWorkspace.getState().activeId).toBe("explain:c");
    rerender(view(null));
    await user.click(screen.getByRole("button", { name: "Открыть консоль EXPLAIN" }));
    expect(useWorkspace.getState().tabs).toEqual([{ kind: "explain", connectionId: "c", member: null, id: "explain:c" }]);
  });

  it("база в фильтре остаётся в списке и тогда, когда отчёт по ней пуст; переключение в окно сбрасывает недоступную сортировку", async () => {
    vi.mocked(api.runDiagnostic).mockImplementation(async (_id, req) =>
      diagReport(data({ rows: req.queries?.database ? [] : [row("1", { datname: "perf", text: "select 1" }), row("2", { datname: "shop", text: "select 2" })] }), { member: "pg-1" }),
    );
    const user = userEvent.setup();
    renderWithClient(view());
    await screen.findByText("select 2");
    await user.selectOptions(screen.getByRole("combobox", { name: "База" }), "shop");
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", expect.objectContaining({ queries: expect.objectContaining({ database: "shop" }) })));
    expect(await screen.findByText("В pg_stat_statements нет строк")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "База" })).toHaveDisplayValue("shop");
    await user.click(within(screen.getByRole("columnheader", { name: /Stddev/ })).getByRole("button"));
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", expect.objectContaining({ queries: expect.objectContaining({ sort: "stddevTime" }) })));
    await user.selectOptions(screen.getByRole("combobox", { name: "Режим" }), "window:900");
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", expect.objectContaining({ queries: expect.objectContaining({ mode: { kind: "window", s: 900 }, sort: "totalTime" }) })));
  });

  it("тексты, которые не удалось получить, таблицу не ломают: остаётся queryId; ошибка не повторяется на каждом обновлении", async () => {
    // Каждый отчёт — новый (счётчики растут): строки без текста приходят снова.
    let n = 0;
    vi.mocked(api.runDiagnostic).mockImplementation(async () => diagReport(data({ rows: [row("1", { calls: 100 + n++ }), row("2", { text: "select 2" })] }), { member: "pg-1" }));
    vi.mocked(api.queryTexts).mockRejectedValue({ code: "TIMEOUT", message: "x", retryable: true });
    const user = userEvent.setup();
    renderWithClient(view());
    await waitFor(() => expect(api.queryTexts).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("button", { name: "1" })).toBeInTheDocument();
    expect(screen.getByText("select 2")).toBeInTheDocument();
    // Новый отчёт (ручное обновление) с той же строкой без текста: ключ после сбоя сразу не перезапрашивается.
    const reads = vi.mocked(api.runDiagnostic).mock.calls.length;
    await user.click(screen.getByRole("button", { name: "Обновить" }));
    await waitFor(() => expect(vi.mocked(api.runDiagnostic).mock.calls.length).toBeGreaterThan(reads));
    await waitFor(() => expect(screen.getByRole("table", { name: "Запросы" })).toHaveTextContent("101"));
    await act(() => new Promise((r) => setTimeout(r, 0)));
    expect(api.queryTexts).toHaveBeenCalledTimes(1);
  });

  it("текст, полученный по ключу, переживает обновление отчёта, ушедшее раньше ответа с текстами", async () => {
    let answer: (v: unknown) => void = () => {};
    vi.mocked(api.queryTexts).mockReturnValue(new Promise((r) => { answer = r; }) as never);
    let refreshed: (v: unknown) => void = () => {};
    vi.mocked(api.runDiagnostic)
      .mockResolvedValueOnce(diagReport(data(), { member: "pg-1" }))
      .mockReturnValueOnce(new Promise((r) => { refreshed = r; }) as never);
    const user = userEvent.setup();
    renderWithClient(view());
    await waitFor(() => expect(api.queryTexts).toHaveBeenCalledTimes(1));
    // Обновление уходит, пока тексты ещё читаются, а приходит после них; его строки — снова без текста.
    await user.click(screen.getByRole("button", { name: "Обновить" }));
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenCalledTimes(2));
    await act(async () => answer([{ key: { queryId: "1", dbid: 5, userid: 10, toplevel: true }, text: "select 1 from t", truncated: false }]));
    expect(await screen.findByText("select 1 from t")).toBeInTheDocument();
    await act(async () => refreshed(diagReport(data({ rows: [row("1", { calls: 101 })] }), { member: "pg-1" })));
    await waitFor(() => expect(screen.queryByText("select 2")).toBeNull());
    expect(screen.getByText("select 1 from t")).toBeInTheDocument();
    expect(api.queryTexts).toHaveBeenCalledTimes(1);
  });

  it("список баз копится по отчётам; стрелка сортировки — только у выбранной колонки, Hit % — по возрастанию", async () => {
    vi.mocked(api.runDiagnostic).mockImplementation(async (_id, req) =>
      diagReport(data({ rows: req.queries?.database === "shop" ? [row("2", { datname: "shop", text: "select 2" })] : [row("1", { datname: "perf", text: "select 1" }), row("2", { datname: "shop", text: "select 2" })] }), { member: "pg-1" }),
    );
    const user = userEvent.setup();
    renderWithClient(view());
    await screen.findByText("select 2");
    await user.selectOptions(screen.getByRole("combobox", { name: "База" }), "shop");
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", expect.objectContaining({ queries: expect.objectContaining({ database: "shop" }) })));
    await waitFor(() => expect(screen.queryByText("select 1")).toBeNull());
    // Отчёт по shop не содержит perf, но база, уже встречавшаяся на узле, остаётся в выборе.
    expect(within(screen.getByRole("combobox", { name: "База" })).getAllByRole("option").map((o) => o.textContent)).toEqual(["Все базы", "perf", "shop"]);
    // По умолчанию — «Доля»; «Всего» сортирует так же, но стрелка — только у той, по которой щёлкнули.
    const sorted = () => screen.getAllByRole("columnheader").filter((h) => h.hasAttribute("aria-sort"));
    expect(sorted().map((h) => h.textContent)).toEqual(["Доля ▾"]);
    await user.click(within(screen.getByRole("columnheader", { name: /Всего/ })).getByRole("button"));
    expect(sorted().map((h) => [h.textContent, h.getAttribute("aria-sort")])).toEqual([["Всего ▾", "descending"]]);
    await user.click(within(screen.getByRole("columnheader", { name: /Hit %/ })).getByRole("button"));
    expect(sorted().map((h) => [h.textContent, h.getAttribute("aria-sort")])).toEqual([["Hit % ▴", "ascending"]]);
  });

  it("покрытие окна — по режиму показанного отчёта: отчёт «с момента сброса» на месте окна пометки не даёт", async () => {
    let finish: (v: unknown) => void = () => {};
    vi.mocked(api.runDiagnostic).mockImplementation(async (_id, req) =>
      req.queries?.mode.kind === "window" ? (new Promise((r) => { finish = r; }) as never) : diagReport(data(), { member: "pg-1" }),
    );
    const user = userEvent.setup();
    renderWithClient(view());
    await screen.findByText("select 2");
    await user.selectOptions(screen.getByRole("combobox", { name: "Режим" }), "window:300");
    // Пока окно читается, на экране прежний отчёт — «Идёт первый тик…» не мелькает.
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", expect.objectContaining({ queries: expect.objectContaining({ mode: { kind: "window", s: 300 } }) })));
    expect(screen.queryByText("Идёт первый тик сэмплера…")).toBeNull();
    await act(async () => finish(diagReport(data({ mode: { kind: "window", s: 300 }, coverageS: null, ticks: 0, rows: [] }), { member: "pg-1" })));
    expect(await screen.findByText("Идёт первый тик сэмплера…")).toBeInTheDocument();
  });

  it("без pg_stat_statements — строка «недоступно», таблицы нет", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data({ pgss: null, rows: [] }), {
      unavailable: [{ part: "statements", reason: { key: "diag.unavailable.statementsMissing", params: { part: "statements" }, text: "statements: pg_stat_statements is not installed in this database" } }],
    }));
    renderWithClient(view());
    expect(await screen.findByText("Статистика запросов (pg_stat_statements): pg_stat_statements не установлен в этой базе (CREATE EXTENSION pg_stat_statements)")).toBeInTheDocument();
    expect(screen.queryByRole("table", { name: "Запросы" })).not.toBeInTheDocument();
  });
});
