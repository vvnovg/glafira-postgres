import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Finding, QueriesData, QueryStat, StatementsEvent } from "../../api/types";
import { formatTimestamp } from "../../lib/format";
import { useWorkspace } from "../../state/workspace";
import { diagReport } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { runDiagnostic: vi.fn(), queryTexts: vi.fn(), startStatementsSampler: vi.fn(), stopSession: vi.fn(), snapshotList: vi.fn(), snapshotCompare: vi.fn(), snapshotDelete: vi.fn() } };
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
    jitFunctions: null, jitMs: null, parallelPlanned: null, parallelLaunched: null, cpuUserMs: null, cpuSysMs: null, diskReadBytes: null, diskWriteBytes: null, sharePct: 50, hitRatio: 0.9, text: null, textTruncated: false, own: false, ...patch,
  };
}

function data(patch: Partial<QueriesData> = {}): { kind: "queries" } & QueriesData {
  return {
    kind: "queries", mode: { kind: "sinceReset" }, coverageS: null, ticks: null,
    pgss: { version: "1.11", max: 5000, trackPlanning: true, hasInfo: true, statsResetMs: 1_700_000_000_000, dealloc: 0, kcache: false, kcacheSinceMs: null, kcacheSeparate: false },
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
    // «За лидером»: сэмплер — на узле, который назвал отчёт.
    await waitFor(() => expect(api.startStatementsSampler).toHaveBeenCalledWith("c", "pg-1", expect.any(Function)));
    expect(api.startStatementsSampler).not.toHaveBeenCalledWith("c", null, expect.any(Function));
  });

  it("«за лидером»: после смены лидера отчёт называет новый узел, и сэмплер переподписывается на него", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data(), { member: "pg-1" }));
    const user = userEvent.setup();
    renderWithClient(view());
    await waitFor(() => expect(api.startStatementsSampler).toHaveBeenCalledWith("c", "pg-1", expect.any(Function)));
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data(), { member: "pg-2" }));
    await user.click(screen.getByRole("button", { name: "Обновить" }));
    await waitFor(() => expect(api.startStatementsSampler).toHaveBeenLastCalledWith("c", "pg-2", expect.any(Function)));
    expect(api.stopSession).toHaveBeenCalledWith("s-1");
    expect(api.startStatementsSampler).toHaveBeenCalledTimes(2);
  });

  it("закреплённый узел: сэмплер — на нём сразу, без ожидания отчёта", async () => {
    vi.mocked(api.runDiagnostic).mockReturnValue(new Promise(() => {}));
    renderWithClient(view("pg-3"));
    await waitFor(() => expect(api.startStatementsSampler).toHaveBeenCalledWith("c", "pg-3", expect.any(Function)));
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

  it("находка о запросе показывает начало его текста (title — полный), без текста — queryId", async () => {
    const long = `select ${"x".repeat(120)} from t`;
    const finding = (queryId: string): Finding => ({
      id: "queries_dominant", severity: "warning", member: null, target: { kind: "query", queryId, database: "perf" },
      params: { sharePct: 40 }, links: [{ kind: "openQuery", queryId, database: "perf", member: null }],
    });
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data({ rows: [row("1", { text: long }), row("2")] }), { member: "pg-1", findings: [finding("1"), finding("2")] }));
    renderWithClient(view());
    const list = (await screen.findByRole("region", { name: /Находки/ })).querySelectorAll(".finding-query");
    expect(list).toHaveLength(2);
    expect(list[0].textContent).toBe(`${long.slice(0, 89)}…`);
    expect(list[0]).toHaveAttribute("title", long);
    expect(list[1].textContent).toBe("2");
  });

  it("строка Глафиры помечена; строка без базы — серая кнопка с причиной", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data({ rows: [row("1", { own: true, text: "select own" }), row("2", { datname: null, text: "select gone" })] }), { member: "pg-1" }));
    renderWithClient(view());
    const own = (await screen.findByText("select own")).closest("td")!;
    expect(within(own).getByText("Глафира")).toHaveAttribute("title", "Запрос самой Глафиры: не участвует в находках");
    expect(screen.getByRole("button", { name: "select gone" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "select gone" })).toHaveAttribute("title", "Запроса уже нет в pg_stat_statements (сброс или вытеснение)");
    expect(within(own).queryByText("Запроса уже нет", { exact: false })).toBeNull();
  });

  it("поиск: запрос уходит с задержкой, пустое поле — без фильтра; пустой результат называет строку", async () => {
    vi.mocked(api.runDiagnostic).mockImplementation(async (_id, req) => diagReport(data({ rows: req.queries?.search ? [] : [row("2", { text: "select 2" })] }), { member: "pg-1" }));
    const user = userEvent.setup();
    renderWithClient(view());
    await screen.findByText("select 2");
    const box = screen.getByRole("searchbox", { name: "Поиск" });
    await user.type(box, "shop.customers");
    // Пока печатают, лишних запросов нет.
    expect(vi.mocked(api.runDiagnostic).mock.calls.filter(([, r]) => r.queries?.search)).toHaveLength(0);
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", expect.objectContaining({ queries: expect.objectContaining({ search: "shop.customers" }) })));
    expect(vi.mocked(api.runDiagnostic).mock.calls.filter(([, r]) => r.queries?.search)).toHaveLength(1);
    expect(await screen.findByText("Ничего не найдено по «shop.customers»")).toBeInTheDocument();
    await user.clear(box);
    expect(await screen.findByText("select 2")).toBeInTheDocument();
    expect(vi.mocked(api.runDiagnostic).mock.lastCall![1].queries?.search ?? "").toBe("");
  });

  it("pgss без pg_stat_statements_info: подвал без «сброс статистики —»; track_planning выключен — «Планирование» «—»", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data({ pgss: { version: "1.8", hasInfo: false, max: 5000, trackPlanning: false, statsResetMs: null, dealloc: null, kcache: false, kcacheSinceMs: null, kcacheSeparate: false }, rows: [row("1", { totalPlanMs: 7, text: "select 1" })] }), { member: "pg-1" }));
    renderWithClient(view());
    await screen.findByText("select 1");
    expect(screen.getByText(/время сброса недоступно/)).toBeInTheDocument();
    expect(screen.queryByText(/сброс статистики/)).toBeNull();
    const col = screen.getAllByRole("columnheader").findIndex((h) => /Планирование/.test(h.textContent ?? ""));
    expect(within(screen.getByText("select 1").closest("tr")!).getAllByRole("cell")[col].textContent).toBe("—");
  });

  it("pg_stat_kcache: колонки CPU и «Чтение с диска» есть только с расширением, сортировка по CPU уходит в запрос", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data(), { member: "pg-1" }));
    const user = userEvent.setup();
    const { unmount } = renderWithClient(view());
    await screen.findByText("select 2");
    expect(screen.queryByRole("columnheader", { name: /CPU/ })).toBeNull();
    expect(screen.queryByRole("columnheader", { name: /Чтение с диска/ })).toBeNull();
    unmount();
    const kc = data({
      pgss: { version: "1.11", max: 5000, trackPlanning: true, hasInfo: true, statsResetMs: null, dealloc: 0, kcache: true, kcacheSinceMs: null, kcacheSeparate: false },
      rows: [row("1", { text: "select 1", cpuUserMs: 1500, cpuSysMs: 500, diskReadBytes: 2048, diskWriteBytes: 0 }), row("2", { text: "select 2" })],
    });
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(kc, { member: "pg-1" }));
    renderWithClient(view());
    await screen.findByText("select 1");
    const headers = screen.getAllByRole("columnheader");
    const cpu = headers.findIndex((h) => /CPU/.test(h.textContent ?? ""));
    const disk = headers.findIndex((h) => /Чтение с диска/.test(h.textContent ?? ""));
    expect(cpu).toBeGreaterThan(0);
    expect(within(headers[disk]).getByRole("button")).toBeDisabled();
    const cells = within(screen.getByText("select 1").closest("tr")!).getAllByRole("cell");
    expect(cells[cpu].textContent).not.toBe("—");
    expect(cells[disk].textContent).not.toBe("—");
    expect(within(screen.getByText("select 2").closest("tr")!).getAllByRole("cell")[cpu].textContent).toBe("—");
    await user.click(within(headers[cpu]).getByRole("button"));
    await waitFor(() => expect(vi.mocked(api.runDiagnostic).mock.lastCall![1].queries?.sort).toBe("cpuTime"));
  });

  it("pg_stat_kcache сброшен отдельно от pgss: колонки CPU и диска помечены, подвал называет период и советует сбрасывать оба", async () => {
    const pgss = { version: "1.11", max: 5000, trackPlanning: true, hasInfo: true, statsResetMs: 1_700_000_000_000, dealloc: 0, kcache: true };
    const rows = [row("1", { text: "select 1", cpuUserMs: 1500, cpuSysMs: 500, diskReadBytes: 2048, diskWriteBytes: 0 })];
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data({ pgss: { ...pgss, kcacheSinceMs: null, kcacheSeparate: false }, rows }), { member: "pg-1" }));
    const { unmount } = renderWithClient(view());
    await screen.findByText("select 1");
    expect(screen.queryByText(/pg_stat_kcache_reset/)).toBeNull();
    expect(screen.getByRole("columnheader", { name: /CPU/ }).textContent).not.toContain("*");
    unmount();
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data({ pgss: { ...pgss, kcacheSinceMs: 1_690_000_000_000, kcacheSeparate: true }, rows }), { member: "pg-1" }));
    const { unmount: unmount2 } = renderWithClient(view());
    await screen.findByText("select 1");
    expect(screen.getByRole("columnheader", { name: /CPU/ }).textContent).toContain("*");
    expect(screen.getByRole("columnheader", { name: /Чтение с диска/ }).textContent).toContain("*");
    expect(screen.getByText(/CPU и диск \(pg_stat_kcache\) накоплены с .*pg_stat_statements_reset\(\) их не сбрасывает/)).toBeInTheDocument();
    expect(screen.getByText("SELECT pg_stat_statements_reset(); SELECT pg_stat_kcache_reset();")).toBeInTheDocument();
    unmount2();
    // kcache без stats_since: время неизвестно, подвал говорит об отдельном сбросе.
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data({ pgss: { ...pgss, kcacheSinceMs: null, kcacheSeparate: true }, rows }), { member: "pg-1" }));
    renderWithClient(view());
    await screen.findByText("select 1");
    expect(screen.getByText(/накоплены с его собственного сброса/)).toBeInTheDocument();
  });

  it("«между снимками»: пара из панели снимков; подпись местным временем, покрытие, без stddev и без запроса текстов; удаление снимка пары и выход", async () => {
    const a = { file: "pg-1/2026-10-29T10-00-00.json", member: "pg-1", takenAtMs: 1_761_732_000_000, rows: 120, bytes: 40_000 };
    const b = { file: "pg-1/2026-10-29T11-00-00.json", member: "pg-1", takenAtMs: 1_761_735_600_000, rows: 130, bytes: 42_000 };
    vi.mocked(api.snapshotList).mockResolvedValue([b, a]);
    vi.mocked(api.snapshotDelete).mockResolvedValue(true);
    vi.mocked(api.snapshotCompare).mockResolvedValue({ new: [], gone: [], newMore: 0, goneMore: 0, degraded: [], improved: [], totalsFrom: { calls: 1, totalExecMs: 1, rows: 0 }, totalsTo: { calls: 2, totalExecMs: 2, rows: 0 }, reset: false });
    const between = diagReport(
      data({ mode: { kind: "between", from: a.file, to: b.file }, coverageS: 3600, rows: [row("1", { own: true, text: "/* glafira */ select 1", stddevExecMs: null, maxExecMs: null, minExecMs: null }), row("2")] }),
      { member: "pg-1", followsLeader: false, unavailable: [{ part: "between", reason: { key: "diag.unavailable.betweenReset", params: { part: "between" }, text: "x" } }] },
    );
    vi.mocked(api.runDiagnostic).mockImplementation(async (_id, req) => (req.queries?.mode.kind === "between" ? between : diagReport(data(), { member: "pg-1" })));
    const user = userEvent.setup();
    renderWithClient(view());
    await screen.findByText("select 2");
    vi.mocked(api.queryTexts).mockClear();
    const toggle = screen.getByRole("button", { name: "Снимки" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await user.click(toggle);
    const panel = screen.getByRole("region", { name: "Снимки" });
    const rows = await within(panel).findAllByRole("row");
    await user.click(within(rows[1]).getByRole("checkbox"));
    await user.click(within(rows[2]).getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "Сравнить" }));
    await user.click(await screen.findByRole("button", { name: "Топ между снимками" }));
    await waitFor(() =>
      expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", { tool: "queries", member: null, queries: { mode: { kind: "between", from: a.file, to: b.file }, sort: "totalTime", limit: 50, database: null, toplevelOnly: true } }),
    );
    expect(screen.queryByRole("region", { name: "Снимки" })).not.toBeInTheDocument();
    expect(screen.getByText(`Между снимками узла pg-1: ${formatTimestamp(a.takenAtMs)} → ${formatTimestamp(b.takenAtMs)}`)).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("1 ч между снимками"));
    expect(screen.getByRole("combobox", { name: "Режим" })).toHaveValue("between");
    expect(screen.getByText("Сравнение: статистика сбрасывалась между снимками")).toBeInTheDocument();
    expect(screen.getByText("Глафира")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stddev" })).toBeDisabled();
    expect(api.queryTexts).not.toHaveBeenCalled();
    // Выход кнопкой — снова «с момента сброса».
    await user.click(screen.getByRole("button", { name: "Выйти из режима снимков" }));
    expect(screen.getByRole("combobox", { name: "Режим" })).toHaveValue("sinceReset");
    expect(screen.queryByText(/Между снимками узла/)).not.toBeInTheDocument();
    // Удаление снимка активной пары выводит из режима «между снимками».
    await user.click(screen.getByRole("button", { name: "Снимки" }));
    const fresh = await within(screen.getByRole("region", { name: "Снимки" })).findAllByRole("row");
    await user.click(within(fresh[1]).getByRole("checkbox"));
    await user.click(within(fresh[2]).getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "Сравнить" }));
    await user.click(await screen.findByRole("button", { name: "Топ между снимками" }));
    expect(await screen.findByText(/Между снимками узла/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Снимки" }));
    const again = await within(screen.getByRole("region", { name: "Снимки" })).findAllByRole("row");
    await user.click(within(again[1]).getByRole("button", { name: "Удалить" }));
    await user.click(screen.getByRole("button", { name: "Удалить снимок" }));
    await waitFor(() => expect(api.snapshotDelete).toHaveBeenCalledWith("c", b.file));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Режим" })).toHaveValue("sinceReset"));
    expect(screen.queryByText(/Между снимками узла/)).not.toBeInTheDocument();
  });

  it("покрытие «между снимками» короче минуты — в секундах", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data({ mode: { kind: "between", from: "pg-1/x.json", to: "pg-1/y.json" }, coverageS: 45 }), { member: "pg-1" }));
    renderWithClient(view());
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("45 с между снимками"));
  });
});
