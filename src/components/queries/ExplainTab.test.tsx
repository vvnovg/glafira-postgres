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
    expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("postgres");
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

  it("скрытая вкладка список баз не читает; при возвращении читает", async () => {
    const { rerender } = renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql={undefined} active={false} />);
    await waitFor(() => expect(api.getTopology).toHaveBeenCalled());
    await Promise.resolve();
    expect(api.listDatabases).not.toHaveBeenCalled();
    rerender(<ExplainTab connectionId="c" member="pg-1" database="perf" sql={undefined} active />);
    expect(await screen.findByRole("combobox", { name: "База" })).toHaveValue("perf");
    expect(api.listDatabases).toHaveBeenCalledTimes(1);
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

  it("«Скопировать для LLM» берёт SQL консоли, узел и базу; без текста кнопки нет", async () => {
    vi.mocked(api.queryLlmBundle).mockRejectedValue({ code: "NOT_AUTHORIZED", key: "errors.explain.notAuthorized", message: "Not enough privileges", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-2" database="perf" sql="" active />);
    await screen.findByRole("combobox", { name: "База" });
    expect(screen.queryByRole("button", { name: "Скопировать для LLM" })).toBeNull();
    await user.type(screen.getByRole("textbox", { name: "SQL" }), "SELECT 2");
    await user.click(screen.getByRole("button", { name: "Скопировать для LLM" }));
    await waitFor(() => expect(api.queryLlmBundle).toHaveBeenCalledWith("c", "pg-2", "perf", { kind: "sql", sql: "SELECT 2" }, null));
  });
});
