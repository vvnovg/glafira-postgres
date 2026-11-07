import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { openExplainWithPrefill, useWorkspace } from "../../state/workspace";
import { resetEventBus } from "../../test/events";
import { connection, member, topology, topologyView } from "../../test/fixtures";
import { llmProposal, optimizationRequest } from "../../test/llm";
import { mcpStatus } from "../../test/mcp";
import { planNode, planTree } from "../../test/plans";
import { renderWithClient } from "../../test/render";

const mcp = vi.hoisted(() => ({ value: { status: undefined as unknown, loading: false, error: null as unknown, refetch: () => {} } }));
vi.mock("../../lib/useMcpStatus", () => ({ useMcpStatus: () => mcp.value }));
vi.mock("@tauri-apps/api/event", async () => (await import("../../test/events")).tauriEventMock());
vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { explain: vi.fn(), parsePlan: vi.fn(), listDatabases: vi.fn(), getTopology: vi.fn(), listConnections: vi.fn(), queryLlmBundle: vi.fn(), listOptimizationRequests: vi.fn(), listLlmProposals: vi.fn(), createOptimizationRequest: vi.fn(), cancelOptimizationRequest: vi.fn() } };
});

import { api } from "../../api/client";
import type { ParsedPlan, PlanTree, Recommendation } from "../../api/types";
import { ExplainTab } from "./ExplainTab";

const pasted = (plan: PlanTree, recommendations: Recommendation[] = []): ParsedPlan => ({ plan, recommendations, recommendationsSkipped: [] });

describe("ExplainTab", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    useWorkspace.setState({ tabs: [], activeId: null });
    mcp.value = { status: mcpStatus(), loading: false, error: null, refetch: vi.fn() };
    resetEventBus();
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([]);
    vi.mocked(api.listLlmProposals).mockResolvedValue([]);
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2", { role: "replica" })] }) }));
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c" })]);
    vi.mocked(api.listDatabases).mockResolvedValue([{ name: "postgres", sizeBytes: 1 }, { name: "perf", sizeBytes: 1 }]);
  });

  it("узел и база из спеки, SQL подставлен, «План» уходит на выбранный узел и базу", async () => {
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "read", plan: planTree(), raw: [], relations: [], elapsedMs: 3, warnings: [], recommendations: [], recommendationsSkipped: [] });
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-2" database="perf" sql="SELECT 1" active />);
    expect(await screen.findByRole("combobox", { name: "База" })).toHaveValue("perf");
    expect(screen.getByRole("combobox", { name: "Узел" })).toHaveValue("pg-2");
    expect(screen.getByRole("textbox", { name: "SQL" })).toHaveValue("SELECT 1");
    await user.click(screen.getByRole("button", { name: "План" }));
    await waitFor(() => expect(api.explain).toHaveBeenCalledWith("c", expect.objectContaining({ member: "pg-2", database: "perf", sql: "SELECT 1", mode: "plan" })));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
  });

  it("без узла в спеке берётся лидер; «Вставить план» разбирает JSON без сервера", async () => {
    vi.mocked(api.parsePlan).mockResolvedValue(pasted(planTree()));
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member={null} database={undefined} sql={undefined} active />);
    expect(await screen.findByRole("combobox", { name: "Узел" })).toHaveValue("pg-1");
    // База по умолчанию — «БД по умолчанию» подключения (у фикстуры postgres).
    await waitFor(() => expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("postgres"));
    await user.click(screen.getByRole("tab", { name: "Вставить план" }));
    await user.type(screen.getByRole("textbox", { name: "План (текст или JSON)" }), "[[{{]");
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    // В user.type «[[» и «{{» — экранированные «[» и «{»: в поле попадает «[{]».
    await waitFor(() => expect(api.parsePlan).toHaveBeenCalledWith("[{]"));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    expect(api.explain).not.toHaveBeenCalled();
  });

  it("без базы в спеке — «БД по умолчанию» подключения, а не первая в списке сервера; ошибка «нет отношения» называет базу", async () => {
    const base = connection({ id: "c" });
    vi.mocked(api.listConnections).mockResolvedValue([{ ...base, pg: { ...base.pg, database: "perf" } }]);
    vi.mocked(api.explain).mockRejectedValue({
      code: "INVALID_INPUT",
      key: "errors.explain.serverMissing",
      params: { message: 'relation "st_boarding.boarding_file" does not exist' },
      message: "The server rejected the query",
      retryable: false,
    });
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member={null} database={undefined} sql="SELECT 1 FROM st_boarding.boarding_file" active />);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("perf"));
    await user.click(screen.getByRole("button", { name: "План" }));
    await waitFor(() => expect(api.explain).toHaveBeenCalledWith("c", expect.objectContaining({ database: "perf" })));
    expect(await screen.findByText("Запрос выполнялся в базе «perf»: проверьте, что объект в этой базе")).toBeInTheDocument();
  });

  it("вставленный план получает рекомендации; «Проверить гипотезу» (нужен сервер) скрыта", async () => {
    const rec: Recommendation = {
      id: "plan_seq_scan_filter",
      severity: "warning",
      nodeIds: [0],
      params: { table: "shop.orders", rowsRead: 500_000, rowsReturned: 500, loops: 1, columns: "status" },
      command: 'CREATE INDEX CONCURRENTLY ON "shop"."orders" ("status");',
      hypo: { indexDdl: 'CREATE INDEX ON "shop"."orders" ("status")' },
    };
    vi.mocked(api.parsePlan).mockResolvedValue(pasted(planTree(), [rec]));
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql={undefined} active />);
    await screen.findByRole("combobox", { name: "Узел" });
    await user.click(screen.getByRole("tab", { name: "Вставить план" }));
    await user.type(screen.getByRole("textbox", { name: "План (текст или JSON)" }), "x");
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    const block = await screen.findByRole("region", { name: "Рекомендации" });
    expect(within(block).getByRole("heading", { name: "Индекс по колонкам фильтра: shop.orders" })).toBeInTheDocument();
    expect(within(block).getByText(rec.command!)).toBeInTheDocument();
    expect(within(block).queryByRole("button", { name: "Проверить гипотезу" })).toBeNull();
  });

  it("ошибка разбора показывается баннером", async () => {
    vi.mocked(api.parsePlan).mockRejectedValue({ code: "INVALID_INPUT", key: "errors.plan.unparsed", params: { line: "1", message: "x" }, message: "The plan could not be parsed (line 1): x", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql={undefined} active />);
    await screen.findByRole("combobox", { name: "Узел" });
    await user.click(screen.getByRole("tab", { name: "Вставить план" }));
    await user.type(screen.getByRole("textbox", { name: "План (текст или JSON)" }), "oops");
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("План не разобран (строка 1): x");
  });

  it("«Разобрать» недоступна, пока вставлено только пустое; ошибка прошлого разбора снимается удачным", async () => {
    vi.mocked(api.parsePlan).mockRejectedValueOnce({ code: "INVALID_INPUT", key: "errors.plan.unparsed", params: { line: "1", message: "x" }, message: "m", retryable: false });
    vi.mocked(api.parsePlan).mockResolvedValueOnce(pasted(planTree()));
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql={undefined} active />);
    await screen.findByRole("combobox", { name: "Узел" });
    await user.click(screen.getByRole("tab", { name: "Вставить план" }));
    expect(screen.getByRole("button", { name: "Разобрать" })).toBeDisabled();
    await user.type(screen.getByRole("textbox", { name: "План (текст или JSON)" }), "bad");
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("«Вставить план» объясняет назначение: текст или JSON, сервер не используется", async () => {
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql={undefined} active />);
    await screen.findByRole("combobox", { name: "Узел" });
    await user.click(screen.getByRole("tab", { name: "Вставить план" }));
    expect(screen.getByText(/План, полученный вне Глафиры.*Сервер не используется/)).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "План (текст или JSON)" })).toHaveAttribute("placeholder", "Вставьте вывод EXPLAIN: текст или FORMAT JSON, можно вместе с заголовком psql");
  });

  it("вставленный план без времени (TIMING OFF) показывается; выбор узла открывает панель узла", async () => {
    const root = planNode({ id: 0, nodeType: "Seq Scan", actual: { startupMs: null, totalMs: null, rows: 5, loops: 1 }, inclusiveMs: null, exclusiveMs: null, sharePct: null, hot: false, misestimated: false, children: [] });
    vi.mocked(api.parsePlan).mockResolvedValue(pasted(planTree({ root, executionMs: null, totalMs: null })));
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql={undefined} active />);
    await screen.findByRole("combobox", { name: "Узел" });
    await user.click(screen.getByRole("tab", { name: "Вставить план" }));
    await user.type(screen.getByRole("textbox", { name: "План (текст или JSON)" }), "x");
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    const tree = await screen.findByRole("treegrid", { name: "Дерево плана" });
    await user.click(within(tree).getByRole("button", { name: /Seq Scan/ }));
    expect(screen.getByText("строк: 5, циклов: 1")).toBeInTheDocument();
    expect(tree).not.toHaveTextContent("NaN");
  });

  it("повторное открытие подставляет новые узел, база и SQL; открытие без SQL оставляет набранное", async () => {
    const { rerender } = renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql="SELECT 1" active />);
    expect(await screen.findByRole("textbox", { name: "SQL" })).toHaveValue("SELECT 1");
    rerender(<ExplainTab connectionId="c" member="pg-2" database="postgres" sql="SELECT 2" active />);
    expect(screen.getByRole("textbox", { name: "SQL" })).toHaveValue("SELECT 2");
    expect(screen.getByRole("combobox", { name: "Узел" })).toHaveValue("pg-2");
    expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("postgres");
    // Пункт дерева открывает консоль без спеки подстановки: набранное остаётся.
    rerender(<ExplainTab connectionId="c" member={undefined} database={undefined} sql={undefined} active />);
    expect(screen.getByRole("textbox", { name: "SQL" })).toHaveValue("SELECT 2");
    expect(screen.getByRole("combobox", { name: "Узел" })).toHaveValue("pg-2");
  });

  it("то же открытие из карточки второй раз возвращает исходный текст, даже если его успели изменить", async () => {
    const user = userEvent.setup();
    const ui = () => <ExplainTab connectionId="c" member="pg-1" database="perf" sql="SELECT 1" active />;
    useWorkspace.getState().open({ kind: "explain", connectionId: "c", member: "pg-1", database: "perf", sql: "SELECT 1" });
    const { rerender } = renderWithClient(ui());
    const box = await screen.findByRole("textbox", { name: "SQL" });
    await user.type(box, " -- правка");
    expect(box).toHaveValue("SELECT 1 -- правка");
    act(() => useWorkspace.getState().open({ kind: "explain", connectionId: "c", member: "pg-1", database: "perf", sql: "SELECT 1" }));
    rerender(ui());
    expect(screen.getByRole("textbox", { name: "SQL" })).toHaveValue("SELECT 1");
  });

  it("версия узла 16+ даёт GENERIC_PLAN для запроса с параметрами; на 15 флажка нет", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary", serverVersion: "17.2" }), member("pg-2", { role: "replica", serverVersion: "15.4" })] }) }));
    const { rerender } = renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql="SELECT $1" active />);
    expect(await screen.findByRole("checkbox")).toBeInTheDocument();
    rerender(<ExplainTab connectionId="c" member="pg-2" database="perf" sql="SELECT $1" active />);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Узел" })).toHaveValue("pg-2"));
    expect(screen.queryByRole("checkbox")).toBeNull();
  });

  it("правка SQL при открытом подтверждении яруса записи закрывает его: запуск с подтверждением не уходит", async () => {
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "write", plan: planTree(), raw: [], relations: [{ schema: "shop", name: "balances" }], elapsedMs: 3, warnings: [], recommendations: [], recommendationsSkipped: [] });
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql="UPDATE shop.balances SET balance = 0" active />);
    await screen.findByRole("combobox", { name: "База" });
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    const dialog = await screen.findByRole("group", { name: "Выполнить запрос и откатить" });
    expect(within(dialog).getByText("shop.balances")).toBeInTheDocument();
    await user.type(screen.getByRole("textbox", { name: "SQL" }), " WHERE false");
    expect(screen.queryByRole("group", { name: "Выполнить запрос и откатить" })).toBeNull();
    expect(api.explain).toHaveBeenCalledTimes(1);
    expect(api.explain).not.toHaveBeenCalledWith("c", expect.objectContaining({ mode: "analyze" }));
  });

  it("закрытое подключение — сообщение вместо консоли", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "disconnected", topology: null }));
    renderWithClient(<ExplainTab connectionId="c" member={null} database={undefined} sql={undefined} active />);
    expect(await screen.findByText("Подключение закрыто")).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "SQL" })).toBeNull();
  });

  it("скрытая вкладка список баз не читает; при возвращении читает ровно один раз", async () => {
    const { rerender, client } = renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql={undefined} active={false} />);
    // Топология прочитана и консоль нарисована; даём пройти макрозадаче — запрос баз, будь он разрешён, уже ушёл бы.
    expect(await screen.findByRole("textbox", { name: "SQL" })).toBeInTheDocument();
    await act(() => new Promise((r) => setTimeout(r, 0)));
    expect(api.listDatabases).not.toHaveBeenCalled();
    rerender(<ExplainTab connectionId="c" member="pg-1" database="perf" sql={undefined} active />);
    await waitFor(() => expect(api.listDatabases).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(client.isFetching()).toBe(0));
    // Всё успокоилось — второго чтения так и не было.
    expect(api.listDatabases).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("perf");
  });

  it("«Вставить план» и запрос работают, пока список баз не прочитан: база — из подключения", async () => {
    vi.mocked(api.listDatabases).mockReturnValue(new Promise(() => {}));
    vi.mocked(api.parsePlan).mockResolvedValue(pasted(planTree()));
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member={null} database={undefined} sql={undefined} active />);
    await user.click(await screen.findByRole("tab", { name: "Вставить план" }));
    // Консоль на месте, полноэкранной загрузки нет.
    expect(screen.getByRole("heading", { level: 2, name: "Консоль EXPLAIN" })).toBeInTheDocument();
    await user.type(screen.getByRole("textbox", { name: "План (текст или JSON)" }), "x");
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    // База известна из подключения («БД по умолчанию»), поэтому режим запроса не ждёт список баз сервера.
    await user.click(screen.getByRole("tab", { name: "Запрос" }));
    expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("postgres");
    expect(screen.getByRole("button", { name: "План" })).toBeInTheDocument();
  });

  it("повтор чтения списка баз не размонтирует запуск: показанный план остаётся", async () => {
    vi.mocked(api.listDatabases).mockRejectedValueOnce({ code: "NODE_UNAVAILABLE", message: "down", retryable: true });
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "read", plan: planTree(), raw: [], relations: [], elapsedMs: 3, warnings: [], recommendations: [], recommendationsSkipped: [] });
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql="SELECT 1" active />);
    // База известна из спеки: запуск доступен и при ошибке чтения списка.
    await user.click(await screen.findByRole("button", { name: "План" }));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: "Повторить" }));
    await waitFor(() => expect(api.listDatabases).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(screen.getByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    expect(api.explain).toHaveBeenCalledTimes(1);
  });

  it("переключение на «Вставить план» и обратно не стирает набранный SQL и показанный план запуска", async () => {
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "read", plan: planTree(), raw: [], relations: [], elapsedMs: 3, warnings: [], recommendations: [], recommendationsSkipped: [] });
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql="SELECT 1" active />);
    await screen.findByRole("combobox", { name: "База" });
    await user.click(screen.getByRole("button", { name: "План" }));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Вставить план" }));
    expect(screen.queryByRole("textbox", { name: "SQL" })).toBeNull();
    expect(screen.queryByRole("treegrid", { name: "Дерево плана" })).toBeNull();
    await user.click(screen.getByRole("tab", { name: "Запрос" }));
    expect(screen.getByRole("textbox", { name: "SQL" })).toHaveValue("SELECT 1");
    expect(screen.getByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    expect(api.explain).toHaveBeenCalledTimes(1);
  });

  it("«Скопировать для LLM» берёт SQL консоли, узел и базу; без текста кнопки нет; без запуска плана нет", async () => {
    vi.mocked(api.queryLlmBundle).mockRejectedValue({ code: "NOT_AUTHORIZED", key: "errors.explain.notAuthorized", message: "Not enough privileges", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-2" database="perf" sql="" active />);
    await screen.findByRole("combobox", { name: "База" });
    expect(screen.queryByRole("button", { name: "Скопировать для LLM" })).toBeNull();
    await user.type(screen.getByRole("textbox", { name: "SQL" }), "SELECT 2");
    await user.click(screen.getByRole("button", { name: "Скопировать для LLM" }));
    await waitFor(() => expect(api.queryLlmBundle).toHaveBeenCalledWith("c", "pg-2", "perf", { kind: "sql", sql: "SELECT 2" }, null));
  });

  it("«Скопировать для LLM» получает план последнего запуска; правка SQL, смена узла или базы возвращают null", async () => {
    const plan = planTree();
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "read", plan, raw: [], relations: [], elapsedMs: 3, warnings: [], recommendations: [], recommendationsSkipped: [] });
    vi.mocked(api.queryLlmBundle).mockRejectedValue({ code: "NOT_AUTHORIZED", key: "errors.explain.notAuthorized", message: "Not enough privileges", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql="SELECT 1" active />);
    await screen.findByRole("combobox", { name: "База" });
    await user.click(screen.getByRole("button", { name: "План" }));
    await screen.findByRole("treegrid", { name: "Дерево плана" });
    const copy = () => user.click(screen.getByRole("button", { name: "Скопировать для LLM" }));
    await copy();
    await waitFor(() => expect(api.queryLlmBundle).toHaveBeenLastCalledWith("c", "pg-1", "perf", { kind: "sql", sql: "SELECT 1" }, plan));
    // План относится к тексту запуска: после правки SQL он не уходит.
    await user.type(screen.getByRole("textbox", { name: "SQL" }), " ");
    await copy();
    await waitFor(() => expect(api.queryLlmBundle).toHaveBeenLastCalledWith("c", "pg-1", "perf", { kind: "sql", sql: "SELECT 1 " }, null));
    // Вернули текст — запуск снова актуален.
    await user.type(screen.getByRole("textbox", { name: "SQL" }), "{Backspace}");
    await copy();
    await waitFor(() => expect(api.queryLlmBundle).toHaveBeenLastCalledWith("c", "pg-1", "perf", { kind: "sql", sql: "SELECT 1" }, plan));
    // Другая база или другой узел — другой запуск.
    await user.selectOptions(screen.getByRole("combobox", { name: "База" }), "postgres");
    await copy();
    await waitFor(() => expect(api.queryLlmBundle).toHaveBeenLastCalledWith("c", "pg-1", "postgres", { kind: "sql", sql: "SELECT 1" }, null));
    // Возврат к прежней базе план не воскрешает: запуск остался в истории размонтированного запуска.
    await user.selectOptions(screen.getByRole("combobox", { name: "База" }), "perf");
    await copy();
    await waitFor(() => expect(api.queryLlmBundle).toHaveBeenLastCalledWith("c", "pg-1", "perf", { kind: "sql", sql: "SELECT 1" }, null));
    expect(screen.queryByRole("treegrid", { name: "Дерево плана" })).toBeNull();
    await user.selectOptions(screen.getByRole("combobox", { name: "Узел" }), "pg-2");
    await copy();
    await waitFor(() => expect(api.queryLlmBundle).toHaveBeenLastCalledWith("c", "pg-2", "perf", { kind: "sql", sql: "SELECT 1" }, null));
  });

  it("запуск, начатый до правки SQL, не подсовывает свой план под новый текст", async () => {
    const plan = planTree();
    let finish: (v: unknown) => void = () => {};
    vi.mocked(api.explain).mockReturnValue(new Promise((r) => { finish = r; }) as never);
    vi.mocked(api.queryLlmBundle).mockRejectedValue({ code: "NOT_AUTHORIZED", key: "errors.explain.notAuthorized", message: "Not enough privileges", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql="SELECT 1" active />);
    await screen.findByRole("combobox", { name: "База" });
    await user.click(screen.getByRole("button", { name: "План" }));
    await user.type(screen.getByRole("textbox", { name: "SQL" }), " ");
    await act(async () => finish({ mode: "plan", tier: "read", plan, raw: [], relations: [], elapsedMs: 3, warnings: [], recommendations: [], recommendationsSkipped: [] }));
    await screen.findByRole("treegrid", { name: "Дерево плана" });
    await user.click(screen.getByRole("button", { name: "Скопировать для LLM" }));
    await waitFor(() => expect(api.queryLlmBundle).toHaveBeenCalledWith("c", "pg-1", "perf", { kind: "sql", sql: "SELECT 1 " }, null));
  });

  it("«Скопировать для LLM» после запуска — в ряду копирования результата, рядом с «JSON» и «Как текст»", async () => {
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "read", plan: planTree(), raw: [], relations: [], elapsedMs: 3, warnings: [], recommendations: [], recommendationsSkipped: [] });
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql="SELECT 1" active />);
    await screen.findByRole("combobox", { name: "База" });
    await user.click(screen.getByRole("button", { name: "План" }));
    await screen.findByRole("treegrid", { name: "Дерево плана" });
    const llm = screen.getByRole("button", { name: "Скопировать для LLM" });
    const row = screen.getByRole("button", { name: "Копировать как текст" }).closest(".toolbar");
    expect(row).not.toBeNull();
    expect(row!.contains(llm)).toBe(true);
    expect(screen.getAllByRole("button", { name: "Скопировать для LLM" })).toHaveLength(1);
  });
  it("подстановка из журнала MCP: SQL, база и узел ставятся один раз на nonce, план не строится", async () => {
    const user = userEvent.setup();
    const prefill = { sql: "SELECT 1 FROM shop.orders", database: "perf", member: "pg-2", nonce: 1 };
    const view = (p: typeof prefill) => <ExplainTab connectionId="c" member={undefined} database={undefined} sql={undefined} prefill={p} active />;
    const { rerender } = renderWithClient(view(prefill));
    expect(await screen.findByRole("textbox", { name: "SQL" })).toHaveValue("SELECT 1 FROM shop.orders");
    expect(screen.getByRole("combobox", { name: "Узел" })).toHaveValue("pg-2");
    await waitFor(() => expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("perf"));
    expect(api.explain).not.toHaveBeenCalled();
    // Правку пользователя перерисовка с той же подстановкой (новый объект, тот же nonce) не затирает…
    await user.clear(screen.getByRole("textbox", { name: "SQL" }));
    await user.type(screen.getByRole("textbox", { name: "SQL" }), "SELECT 2");
    rerender(view({ ...prefill }));
    expect(screen.getByRole("textbox", { name: "SQL" })).toHaveValue("SELECT 2");
    // …а новое открытие (новый nonce) подставляет снова.
    rerender(view({ ...prefill, nonce: 2 }));
    expect(screen.getByRole("textbox", { name: "SQL" })).toHaveValue("SELECT 1 FROM shop.orders");
  });

  it("подстановка без узла — лидер; консоль переходит из «Вставить план» на «Запрос»", async () => {
    const user = userEvent.setup();
    const { rerender } = renderWithClient(<ExplainTab connectionId="c" member="pg-2" database={undefined} sql={undefined} active />);
    await user.click(await screen.findByRole("tab", { name: "Вставить план" }));
    expect(screen.queryByRole("textbox", { name: "SQL" })).not.toBeInTheDocument();
    rerender(<ExplainTab connectionId="c" member={undefined} database={undefined} sql={undefined} prefill={{ sql: "SELECT 3", database: "postgres", nonce: 5 }} active />);
    expect(await screen.findByRole("textbox", { name: "SQL" })).toHaveValue("SELECT 3");
    expect(screen.getByRole("combobox", { name: "Узел" })).toHaveValue("pg-1");
  });

  it("«Передать LLM-ассистенту»: план и значения параметров последнего запуска; после правки текста — без них", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary", serverVersion: "15.4" })] }) }));
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", llmAccess: "diagnostics" })]);
    const plan = planTree();
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "read", plan, raw: [], relations: [], elapsedMs: 3, warnings: [], recommendations: [], recommendationsSkipped: [] });
    vi.mocked(api.createOptimizationRequest).mockResolvedValue(optimizationRequest({ status: "answered" }));
    const sql = "SELECT * FROM shop.orders WHERE status = $1";
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql={sql} active />);
    // Кнопка есть в консоли и до первого запуска.
    expect(await screen.findByRole("button", { name: "Передать LLM-ассистенту" })).toBeInTheDocument();
    await user.type(await screen.findByRole("textbox", { name: "$1" }), "disputed");
    await user.click(screen.getByRole("button", { name: "План" }));
    await screen.findByRole("treegrid", { name: "Дерево плана" });
    const ask = screen.getByRole("button", { name: "Передать LLM-ассистенту" });
    await waitFor(() => expect(ask).toBeEnabled());
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest({ status: "answered" })]);
    await user.click(ask);
    expect(api.createOptimizationRequest).toHaveBeenLastCalledWith("c", { member: "pg-1", database: "perf", source: { kind: "console" }, sql, params: ["disputed"], plan });
    // Список перечитался и знает о созданном запросе (он уже отвечен) — можно отправлять снова.
    await waitFor(() => expect(screen.getByRole("button", { name: "Передать LLM-ассистенту" })).toBeEnabled());
    // Текст правили — план и значения прежнего запуска к нему не относятся.
    await user.type(screen.getByRole("textbox", { name: "SQL" }), " ");
    await user.click(screen.getByRole("button", { name: "Передать LLM-ассистенту" }));
    expect(api.createOptimizationRequest).toHaveBeenLastCalledWith("c", { member: "pg-1", database: "perf", source: { kind: "console" }, sql: `${sql} `, params: [], plan: null });
  });

  it("«Предложения LLM» — сразу под редактором; подстановка по ссылке карточки выбирает её предложение", async () => {
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest({ id: "r9", source: { kind: "query", queryId: "7" }, status: "answered" })]);
    vi.mocked(api.listLlmProposals).mockResolvedValue([llmProposal({ id: "p9", requestId: "r9", rewrittenSql: "SELECT 9 AS from_card" })]);
    renderWithClient(
      <ExplainTab connectionId="c" member="pg-1" database="perf" sql="SELECT 1" active prefill={{ sql: "SELECT 1", database: "perf", member: "pg-1", nonce: 5, proposalId: "p9" }} />,
    );
    const editor = await screen.findByRole("textbox", { name: "SQL" });
    const panel = screen.getByRole("region", { name: "Предложения LLM" });
    // Под редактором и над кнопками запуска.
    expect(editor.parentElement).toContainElement(panel);
    expect(editor.compareDocumentPosition(panel) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(panel.compareDocumentPosition(await screen.findByRole("button", { name: "План" })) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(await within(panel).findByRole("table", { name: "Исходный и предложенный текст" })).toHaveTextContent("SELECT 9 AS from_card");
  });

  it("M6: «Открыть в консоли» не теряет прежний текст — он в списке «Заменённые тексты» и возвращается (тот же узел и другой)", async () => {
    const user = userEvent.setup();
    const view = (p: { sql: string; database?: string; member?: string; nonce: number }) => <ExplainTab connectionId="c" member="pg-1" database="perf" sql={undefined} prefill={p} active />;
    const { rerender } = renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql="SELECT 1 -- mine" active />);
    const editor = await screen.findByRole("textbox", { name: "SQL" });
    expect(screen.queryByRole("region", { name: "Заменённые тексты" })).toBeNull();
    // Тот же узел и база.
    rerender(view({ sql: "SELECT 2 -- rewritten", database: "perf", member: "pg-1", nonce: 1 }));
    expect(editor).toHaveValue("SELECT 2 -- rewritten");
    // Другой узел и база: запуск пересоздаётся, текст всё равно сохранён.
    rerender(view({ sql: "SELECT 3 -- other", database: "postgres", member: "pg-2", nonce: 2 }));
    expect(editor).toHaveValue("SELECT 3 -- other");
    const list = within(screen.getByRole("region", { name: "Заменённые тексты" }));
    expect(list.getAllByRole("button").map((b) => b.textContent)).toEqual(["Вернуть: SELECT 2 -- rewritten", "Вернуть: SELECT 1 -- mine"]);
    await user.click(list.getByRole("button", { name: "Вернуть: SELECT 1 -- mine" }));
    expect(editor).toHaveValue("SELECT 1 -- mine");
  });

  it("Z9: две подстановки подряд (nonce 1, 2), вторая — с другой базой: заменённый текст в списке", async () => {
    const view = (p: { sql: string; database?: string; nonce: number }) => <ExplainTab connectionId="c" member={undefined} database={undefined} sql={undefined} prefill={p} active />;
    const { rerender } = renderWithClient(view({ sql: "SELECT count(*) FROM orders WHERE status = $1", database: "perf", nonce: 1 }));
    const editor = await screen.findByRole("textbox", { name: "SQL" });
    await waitFor(() => expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("perf"));
    rerender(view({ sql: "SELECT current_database()", database: "postgres", nonce: 2 }));
    expect(editor).toHaveValue("SELECT current_database()");
    await waitFor(() => expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("postgres"));
    const list = within(screen.getByRole("region", { name: "Заменённые тексты" }));
    expect(list.getAllByRole("button").map((b) => b.textContent)).toEqual(["Вернуть: SELECT count(*) FROM orders WHERE status = $1"]);
  });

  it("Z9: смоук через вкладку — три «Открыть в консоли EXPLAIN» из журнала (база, та же база, другая база)", async () => {
    // Как TabContent: консоль получает спеку вкладки из рабочей области; вкладка не перемонтируется.
    function Console() {
      const tab = useWorkspace((s) => s.tabs.find((x) => x.id === "explain:c"));
      return tab?.kind === "explain" ? <ExplainTab connectionId="c" member={tab.member} database={tab.database} sql={tab.sql} prefill={tab.prefill} active /> : null;
    }
    act(() => openExplainWithPrefill("c", { sql: "SELECT md5('p4b-smoke')", database: "perf" }));
    renderWithClient(<Console />);
    const editor = await screen.findByRole("textbox", { name: "SQL" });
    await waitFor(() => expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("perf"));
    act(() => openExplainWithPrefill("c", { sql: "SELECT count(*) FROM orders WHERE status = 'new'", database: "perf" }));
    expect(editor).toHaveValue("SELECT count(*) FROM orders WHERE status = 'new'");
    act(() => openExplainWithPrefill("c", { sql: "SELECT current_database()", database: "postgres" }));
    expect(editor).toHaveValue("SELECT current_database()");
    await waitFor(() => expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("postgres"));
    const list = within(screen.getByRole("region", { name: "Заменённые тексты" }));
    expect(list.getAllByRole("button").map((b) => b.textContent)).toEqual(["Вернуть: SELECT count(*) FROM orders WHERE status = 'new'", "Вернуть: SELECT md5('p4b-smoke')"]);
  });
});
