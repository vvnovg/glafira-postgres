import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useWorkspace } from "../../state/workspace";
import { connection, member, topology, topologyView } from "../../test/fixtures";
import { planNode, planTree } from "../../test/plans";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { explain: vi.fn(), parsePlan: vi.fn(), listDatabases: vi.fn(), getTopology: vi.fn(), listConnections: vi.fn(), queryLlmBundle: vi.fn() } };
});

import { api } from "../../api/client";
import { ExplainTab } from "./ExplainTab";

describe("ExplainTab", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    useWorkspace.setState({ tabs: [], activeId: null });
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2", { role: "replica" })] }) }));
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c" })]);
    vi.mocked(api.listDatabases).mockResolvedValue([{ name: "postgres", sizeBytes: 1 }, { name: "perf", sizeBytes: 1 }]);
  });

  it("узел и база из спеки, SQL подставлен, «План» уходит на выбранный узел и базу", async () => {
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "read", plan: planTree(), raw: [], relations: [], elapsedMs: 3, warnings: [] });
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
    vi.mocked(api.parsePlan).mockResolvedValue(planTree());
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member={null} database={undefined} sql={undefined} active />);
    expect(await screen.findByRole("combobox", { name: "Узел" })).toHaveValue("pg-1");
    // База по умолчанию — первая из списка: известна, когда список прочитан.
    await waitFor(() => expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("postgres"));
    await user.click(screen.getByRole("tab", { name: "Вставить план" }));
    await user.type(screen.getByRole("textbox", { name: "План (JSON)" }), "[[{{]");
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    // В user.type «[[» и «{{» — экранированные «[» и «{»: в поле попадает «[{]».
    await waitFor(() => expect(api.parsePlan).toHaveBeenCalledWith("[{]"));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    expect(api.explain).not.toHaveBeenCalled();
  });

  it("ошибка разбора показывается баннером", async () => {
    vi.mocked(api.parsePlan).mockRejectedValue({ code: "INVALID_INPUT", key: "errors.plan.unparsed", params: { line: "1", message: "x" }, message: "The plan could not be parsed (line 1): x", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql={undefined} active />);
    await screen.findByRole("combobox", { name: "Узел" });
    await user.click(screen.getByRole("tab", { name: "Вставить план" }));
    await user.type(screen.getByRole("textbox", { name: "План (JSON)" }), "oops");
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("План не разобран (строка 1): x");
  });

  it("«Разобрать» недоступна, пока вставлено только пустое; ошибка прошлого разбора снимается удачным", async () => {
    vi.mocked(api.parsePlan).mockRejectedValueOnce({ code: "INVALID_INPUT", key: "errors.plan.unparsed", params: { line: "1", message: "x" }, message: "m", retryable: false });
    vi.mocked(api.parsePlan).mockResolvedValueOnce(planTree());
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql={undefined} active />);
    await screen.findByRole("combobox", { name: "Узел" });
    await user.click(screen.getByRole("tab", { name: "Вставить план" }));
    expect(screen.getByRole("button", { name: "Разобрать" })).toBeDisabled();
    await user.type(screen.getByRole("textbox", { name: "План (JSON)" }), "bad");
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("вставленный план без времени (TIMING OFF) показывается; выбор узла открывает панель узла", async () => {
    const root = planNode({ id: 0, nodeType: "Seq Scan", actual: { startupMs: null, totalMs: null, rows: 5, loops: 1 }, inclusiveMs: null, exclusiveMs: null, sharePct: null, hot: false, misestimated: false, children: [] });
    vi.mocked(api.parsePlan).mockResolvedValue(planTree({ root, executionMs: null, totalMs: null }));
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql={undefined} active />);
    await screen.findByRole("combobox", { name: "Узел" });
    await user.click(screen.getByRole("tab", { name: "Вставить план" }));
    await user.type(screen.getByRole("textbox", { name: "План (JSON)" }), "x");
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    const tree = await screen.findByRole("treegrid", { name: "Дерево плана" });
    await user.click(within(tree).getByRole("button", { name: /Seq Scan/ }));
    expect(screen.getByText("5 строк, циклы 1")).toBeInTheDocument();
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
    expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("perf");
  });

  it("«Вставить план» работает, пока список баз не прочитан: полноэкранной загрузки нет", async () => {
    vi.mocked(api.listDatabases).mockReturnValue(new Promise(() => {}));
    vi.mocked(api.parsePlan).mockResolvedValue(planTree());
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member={null} database={undefined} sql={undefined} active />);
    await user.click(await screen.findByRole("tab", { name: "Вставить план" }));
    // Консоль на месте, загрузка (ждёт база) — только в скрытой панели запроса.
    expect(screen.getByRole("heading", { level: 2, name: "Консоль EXPLAIN" })).toBeInTheDocument();
    expect(screen.getByText("Загрузка…")).not.toBeVisible();
    await user.type(screen.getByRole("textbox", { name: "План (JSON)" }), "x");
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    // Режим запроса ждёт базу: вместо запуска — загрузка, поле SQL доступно.
    await user.click(screen.getByRole("tab", { name: "Запрос" }));
    expect(screen.getByText("Загрузка…")).toBeVisible();
    expect(screen.queryByRole("button", { name: "План" })).toBeNull();
  });

  it("повтор чтения списка баз не размонтирует запуск: показанный план остаётся", async () => {
    vi.mocked(api.listDatabases).mockRejectedValueOnce({ code: "NODE_UNAVAILABLE", message: "down", retryable: true });
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "read", plan: planTree(), raw: [], relations: [], elapsedMs: 3, warnings: [] });
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
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "read", plan: planTree(), raw: [], relations: [], elapsedMs: 3, warnings: [] });
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
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "read", plan, raw: [], relations: [], elapsedMs: 3, warnings: [] });
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
    await user.selectOptions(screen.getByRole("combobox", { name: "База" }), "perf");
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
    await act(async () => finish({ mode: "plan", tier: "read", plan, raw: [], relations: [], elapsedMs: 3, warnings: [] }));
    await screen.findByRole("treegrid", { name: "Дерево плана" });
    await user.click(screen.getByRole("button", { name: "Скопировать для LLM" }));
    await waitFor(() => expect(api.queryLlmBundle).toHaveBeenCalledWith("c", "pg-1", "perf", { kind: "sql", sql: "SELECT 1 " }, null));
  });
});
