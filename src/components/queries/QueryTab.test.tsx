import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { QueryDetail } from "../../api/types";
import { useWorkspace } from "../../state/workspace";
import { connection, member, topology, topologyView } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { queryDetail: vi.fn(), explain: vi.fn(), queryLlmBundle: vi.fn(), startStatementsSampler: vi.fn(), stopSession: vi.fn(), getTopology: vi.fn(), listConnections: vi.fn() } };
});

import { api } from "../../api/client";
import { QueryTab } from "./QueryTab";

const stat = { key: { queryId: "7", dbid: 1, userid: 1, toplevel: true }, datname: "perf", rolname: "app", calls: 1200, totalExecMs: 600, meanExecMs: 0.5, minExecMs: 0.1, maxExecMs: 4, stddevExecMs: 0.3, rows: 1200, rowsPerCall: 1, plans: 0, totalPlanMs: 0, sharedHit: 5000, sharedRead: 0, sharedDirtied: 0, sharedWritten: 0, localHit: 0, localRead: 0, localDirtied: 0, localWritten: 0, tempRead: 0, tempWritten: 0, ioReadMs: 0, ioWriteMs: 0, tempIoMs: null, walRecords: 0, walFpi: 0, walBytes: 0, jitFunctions: null, jitMs: null, parallelPlanned: null, parallelLaunched: null, sharePct: 30, hitRatio: 1, text: null, textTruncated: false, own: false };

const detail = (patch: Partial<QueryDetail> = {}): QueryDetail => ({
  text: "SELECT * FROM shop.customers WHERE id = $1", textTruncated: false, sinceReset: stat,
  windows: [{ windowS: 300, stat: null }, { windowS: 900, stat: null }, { windowS: 3600, stat: { ...stat, calls: 60 } }],
  totals: { calls: 4000, totalExecMs: 2000, rows: 0 }, byUser: [{ rolname: "app", toplevel: true, stat }],
  history: [{ atMs: 1, calls: 10, totalExecMs: 5, meanExecMs: 0.5 }, { atMs: 60_001, calls: 20, totalExecMs: 10, meanExecMs: 0.5 }], gaps: [],
  running: [{ pid: 42, backendStartUs: 1, state: "active", startedMs: 1, durationS: 3.5, waitEventType: "IO", waitEvent: "DataFileRead", usename: "app", applicationName: "api", clientAddr: "10.0.0.5", query: "SELECT * FROM shop.customers WHERE id = 15", truncated: false }],
  waits: { source: "sampler", sinceMs: 0, total: 10, items: [{ eventType: "IO", event: "DataFileRead", count: 7, sharePct: 70 }, { eventType: "CPU", event: null, count: 3, sharePct: 30 }] },
  findings: [], unavailable: [], serverVersionNum: 170_000, inRecovery: false, pgssVersion: "1.11", trackPlanning: true, ...patch,
});

describe("QueryTab", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    useWorkspace.setState({ tabs: [], activeId: null });
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2", { role: "replica" })] }) }));
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c" })]);
    vi.mocked(api.startStatementsSampler).mockResolvedValue("s-1");
    vi.mocked(api.stopSession).mockResolvedValue(true);
  });

  it("обзор: текст, метрики в две колонки, разбивка, экземпляры, ожидания; таблицы — после плана", async () => {
    vi.mocked(api.queryDetail).mockResolvedValue(detail());
    const user = userEvent.setup();
    renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    expect(await screen.findByText("SELECT * FROM shop.customers WHERE id = $1")).toBeInTheDocument();
    expect(api.queryDetail).toHaveBeenCalledWith("c", "pg-1", "perf", "7");
    expect(screen.getByRole("table", { name: "Метрики" })).toHaveTextContent("1 200");
    expect(screen.getByRole("table", { name: "Метрики" })).toHaveTextContent("60");
    expect(screen.getByRole("table", { name: "Выполняющиеся экземпляры" })).toHaveTextContent("42");
    expect(screen.getByRole("table", { name: "Ожидания" })).toHaveTextContent("IO:DataFileRead");
    expect(screen.getByText("Таблицы появятся после первого плана")).toBeInTheDocument();
    expect(api.startStatementsSampler).toHaveBeenCalledWith("c", "pg-1", expect.any(Function));
    // «Открыть в консоли» подставляет текст в консоль подключения.
    await user.click(screen.getByRole("button", { name: "Открыть в консоли" }));
    const tabs = useWorkspace.getState().tabs;
    expect(tabs[tabs.length - 1]).toMatchObject({ kind: "explain", connectionId: "c", member: "pg-1", database: "perf", sql: "SELECT * FROM shop.customers WHERE id = $1" });
  });

  it("подвкладка «План»: источник — текст экземпляра, пробный план даёт таблицы в обзоре", async () => {
    vi.mocked(api.queryDetail).mockResolvedValue(detail());
    const plan = (await import("../../test/plans")).planTree();
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "read", plan, raw: [], relations: [{ schema: "shop", name: "customers" }], elapsedMs: 5, warnings: [] });
    const user = userEvent.setup();
    renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    await screen.findByText("SELECT * FROM shop.customers WHERE id = $1");
    await user.click(screen.getByRole("tab", { name: "План" }));
    await user.selectOptions(screen.getByRole("combobox", { name: "Текст запроса" }), "running:42");
    await user.click(screen.getByRole("button", { name: "План" }));
    await waitFor(() => expect(api.explain).toHaveBeenCalledWith("c", expect.objectContaining({ sql: "SELECT * FROM shop.customers WHERE id = 15", params: { kind: "none" } })));
    await user.click(screen.getByRole("tab", { name: "Обзор" }));
    expect(await screen.findByRole("button", { name: "shop.customers" })).toBeInTheDocument();
    // Подвкладка «План» хранит результат и выбор источника, пока вкладка открыта.
    await user.click(screen.getByRole("tab", { name: "План" }));
    expect(screen.getByRole("combobox", { name: "Текст запроса" })).toHaveValue("running:42");
    expect(screen.getByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    expect(api.explain).toHaveBeenCalledTimes(1);
    // «Скопировать для LLM» — в ряду кнопок обзора; передаёт последний план вкладки; markdown собирается на языке интерфейса.
    vi.mocked(api.queryLlmBundle).mockResolvedValue({ serverVersionNum: 170_004, pgssVersion: "1.11", inRecovery: false, member: "pg-1", database: "perf", text: "SELECT 1", textTruncated: false, stats: null, planText: "Result\n", tables: [], tablesOmitted: 0, warnings: [] });
    await user.click(screen.getByRole("tab", { name: "Обзор" }));
    await user.click(screen.getByRole("button", { name: "Скопировать для LLM" }));
    expect(await screen.findByText("Скопировано")).toBeInTheDocument();
    expect(api.queryLlmBundle).toHaveBeenCalledWith("c", "pg-1", "perf", { kind: "query", queryId: "7" }, plan);
    expect(await navigator.clipboard.readText()).toContain("## Среда\n\n- PostgreSQL 17.4");
  });

  it("«Скопировать для LLM»: ошибка сборки показывается текстом словаря, без плана уходит null", async () => {
    vi.mocked(api.queryDetail).mockResolvedValue(detail());
    vi.mocked(api.queryLlmBundle).mockRejectedValue({ code: "NOT_AUTHORIZED", key: "errors.explain.notAuthorized", message: "Not enough privileges", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    await screen.findByText("SELECT * FROM shop.customers WHERE id = $1");
    await user.click(screen.getByRole("button", { name: "Скопировать для LLM" }));
    expect(await screen.findByText(/Не хватает прав на таблицы запроса/)).toBeInTheDocument();
    expect(api.queryLlmBundle).toHaveBeenCalledWith("c", "pg-1", "perf", { kind: "query", queryId: "7" }, null);
  });

  it("запрос исчез из pgss — пометка без ошибки; на PG 13 экземпляры недоступны", async () => {
    vi.mocked(api.queryDetail).mockResolvedValue(detail({ text: null, sinceReset: null, byUser: [], running: [], serverVersionNum: 130_000, unavailable: [{ part: "running", reason: { key: "diag.unavailable.version", params: { part: "running", version: "14" }, text: "running: PostgreSQL 14+" } }] }));
    renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    expect(await screen.findByText("Запроса больше нет в pg_stat_statements")).toBeInTheDocument();
    expect(screen.getByText("Выполняющиеся экземпляры (query_id): только с PostgreSQL 14")).toBeInTheDocument();
    expect(screen.queryByText("Сейчас не выполняется")).not.toBeInTheDocument();
    // Блок экземпляров скрыт целиком: ни заголовка, ни ссылки на «Сессии».
    expect(screen.queryByRole("heading", { name: "Выполняющиеся экземпляры" })).toBeNull();
    expect(screen.queryByRole("button", { name: /Открыть «/ })).toBeNull();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("вкладка, открытая без текста, получает заголовок по тексту; свой заголовок не трогается", async () => {
    vi.mocked(api.queryDetail).mockResolvedValue(detail());
    useWorkspace.getState().open({ kind: "query", connectionId: "c", member: "pg-1", database: "perf", queryId: "7" });
    useWorkspace.getState().open({ kind: "query", connectionId: "c", member: "pg-2", database: "perf", queryId: "7", title: "мой заголовок" });
    const first = renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    await waitFor(() => expect(useWorkspace.getState().tabs[0]).toMatchObject({ title: "SELECT * FROM shop.customers WHERE id = $1" }));
    first.unmount();
    renderWithClient(<QueryTab connectionId="c" member="pg-2" database="perf" queryId="7" active />);
    await screen.findByText("SELECT * FROM shop.customers WHERE id = $1");
    expect(useWorkspace.getState().tabs[1]).toMatchObject({ title: "мой заголовок" });
  });

  it("узла больше нет в топологии — заглушка с выбором другого узла, карточка не читается", async () => {
    vi.mocked(api.queryDetail).mockResolvedValue(detail());
    const user = userEvent.setup();
    renderWithClient(<QueryTab connectionId="c" member="pg-9" database="perf" queryId="7" active />);
    expect(await screen.findByText("Узла pg-9 больше нет в топологии подключения")).toBeInTheDocument();
    expect(api.queryDetail).not.toHaveBeenCalled();
    expect(api.startStatementsSampler).not.toHaveBeenCalled();
    await user.selectOptions(screen.getByRole("combobox", { name: "Открыть на узле" }), "pg-2");
    const tabs = useWorkspace.getState().tabs;
    expect(tabs[tabs.length - 1]).toMatchObject({ kind: "query", connectionId: "c", member: "pg-2", database: "perf", queryId: "7" });
  });

  it("«Копировать текст запроса», «Обновить» и «Получить план», который сразу строит пробный план", async () => {
    vi.mocked(api.queryDetail).mockResolvedValue(detail());
    const plan = (await import("../../test/plans")).planTree();
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "read", plan, raw: [], relations: [{ schema: "shop", name: "customers" }], elapsedMs: 5, warnings: [] });
    const user = userEvent.setup();
    renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    await screen.findByText("SELECT * FROM shop.customers WHERE id = $1");
    await user.click(screen.getByRole("button", { name: "Копировать текст запроса" }));
    expect(await navigator.clipboard.readText()).toBe("SELECT * FROM shop.customers WHERE id = $1");
    const reads = vi.mocked(api.queryDetail).mock.calls.length;
    await user.click(screen.getByRole("button", { name: "Обновить" }));
    await waitFor(() => expect(vi.mocked(api.queryDetail).mock.calls.length).toBeGreaterThan(reads));
    await user.click(screen.getByRole("button", { name: "Получить план" }));
    await waitFor(() => expect(api.explain).toHaveBeenCalledWith("c", expect.objectContaining({ mode: "plan", sql: "SELECT * FROM shop.customers WHERE id = $1", member: "pg-1" })));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeVisible();
    expect(api.explain).toHaveBeenCalledTimes(1);
    // Таблицы из пробы — в обзоре; кнопка «Получить план» под текстом остаётся, а второй (внизу) нет.
    await user.click(screen.getByRole("tab", { name: "Обзор" }));
    expect(await screen.findByRole("button", { name: "shop.customers" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Получить план" })).toHaveLength(1);
  });

  it("«Скопировать для LLM» берёт план, только если он получен на узле вкладки: ANALYZE чтения на реплике — без плана", async () => {
    vi.mocked(api.queryDetail).mockResolvedValue(detail());
    const plans = await import("../../test/plans");
    const onReplica = plans.planTree();
    const onLeader = plans.planTree();
    vi.mocked(api.explain)
      .mockResolvedValueOnce({ mode: "plan", tier: "read", plan: plans.planTree(), raw: [], relations: [], elapsedMs: 5, warnings: [] })
      .mockResolvedValueOnce({ mode: "analyze", tier: "read", plan: onReplica, raw: [], relations: [], elapsedMs: 9, warnings: [] });
    vi.mocked(api.queryLlmBundle).mockResolvedValue({ serverVersionNum: 170_004, pgssVersion: "1.11", inRecovery: false, member: "pg-1", database: "perf", text: "SELECT 1", textTruncated: false, stats: null, planText: null, tables: [], tablesOmitted: 0, warnings: [] });
    const user = userEvent.setup();
    renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    await screen.findByText("SELECT * FROM shop.customers WHERE id = $1");
    await user.click(screen.getByRole("tab", { name: "План" }));
    // Текст экземпляра (без $N): «План с выполнением» сразу; лидер с доступной репликой — чтение уходит на pg-2.
    await user.selectOptions(screen.getByRole("combobox", { name: "Текст запроса" }), "running:42");
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", member: "pg-2" })));
    expect(await screen.findByText("Узел: pg-2")).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Обзор" }));
    await user.click(screen.getByRole("button", { name: "Скопировать для LLM" }));
    await waitFor(() => expect(api.queryLlmBundle).toHaveBeenLastCalledWith("c", "pg-1", "perf", { kind: "query", queryId: "7" }, null));
    // План на узле вкладки — уходит в копию.
    vi.mocked(api.explain).mockResolvedValueOnce({ mode: "plan", tier: "read", plan: onLeader, raw: [], relations: [], elapsedMs: 4, warnings: [] });
    await user.click(screen.getByRole("tab", { name: "План" }));
    await user.click(screen.getByRole("button", { name: "План" }));
    await waitFor(() => expect(screen.queryByText("Узел: pg-2")).toBeNull());
    await user.click(screen.getByRole("tab", { name: "Обзор" }));
    await user.click(screen.getByRole("button", { name: "Скопировать для LLM" }));
    await waitFor(() => expect(api.queryLlmBundle).toHaveBeenLastCalledWith("c", "pg-1", "perf", { kind: "query", queryId: "7" }, onLeader));
  });

  it("без текста и с обрезанным нормализованным текстом «Получить план» нет; обрезанный pgss — пункт серый, вместо запуска пояснение", async () => {
    vi.mocked(api.queryDetail).mockResolvedValue(detail({ text: null, sinceReset: null, byUser: [] }));
    const first = renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    expect(await screen.findByText("Таблицы появятся после первого плана")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Получить план" })).toBeNull();
    first.unmount();
    vi.mocked(api.queryDetail).mockResolvedValue(detail({ textTruncated: true }));
    const user = userEvent.setup();
    renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    expect(await screen.findByText("Таблицы появятся после первого плана")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Получить план" })).toBeNull();
    await user.click(screen.getByRole("tab", { name: "План" }));
    expect(screen.getByRole("option", { name: "Нормализованный текст (pg_stat_statements) — обрезан" })).toBeDisabled();
    expect(screen.getByText(/Нормализованный текст обрезан: план по нему не построить/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "План" })).toBeNull();
    // Экземпляр с полным текстом — запускается.
    await user.selectOptions(screen.getByRole("combobox", { name: "Текст запроса" }), "running:42");
    expect(screen.getByRole("button", { name: "План" })).toBeInTheDocument();
    expect(api.explain).not.toHaveBeenCalled();
  });

  it("ряд кнопок под текстом: Получить план, Открыть в консоли, Копировать текст запроса, Скопировать для LLM; в шапке LLM нет", async () => {
    vi.mocked(api.queryDetail).mockResolvedValue(detail());
    renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    await screen.findByText("SELECT * FROM shop.customers WHERE id = $1");
    const text = screen.getByRole("group", { name: "Текст запроса" });
    const row = text.nextElementSibling as HTMLElement;
    expect(within(row).getAllByRole("button").map((b) => b.textContent)).toEqual(["Получить план", "Открыть в консоли", "Копировать", "Скопировать для LLM"]);
    expect(within(text.closest(".query-tab")!.querySelector(".toolbar") as HTMLElement).queryByRole("button", { name: "Скопировать для LLM" })).toBeNull();
    // Старой кнопки «План» под текстом нет: «План» — только подвкладка.
    expect(screen.queryByRole("button", { name: "План" })).toBeNull();
  });

  it("track_planning выключен — «Планирование» в карточке «—»", async () => {
    vi.mocked(api.queryDetail).mockResolvedValue(detail({ trackPlanning: false, sinceReset: { ...stat, totalPlanMs: 12 } }));
    renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    const row = (await screen.findByRole("rowheader", { name: "Планирование" })).closest("tr")!;
    expect(within(row).getAllByRole("cell").map((c) => c.textContent)).toEqual(["—", "—"]);
  });

  it("подключение закрыто — карточка не читает сервер", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "disconnected", topology: null }));
    renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    expect(await screen.findByText("Подключение закрыто")).toBeInTheDocument();
    expect(api.queryDetail).not.toHaveBeenCalled();
    expect(api.startStatementsSampler).not.toHaveBeenCalled();
  });
});
