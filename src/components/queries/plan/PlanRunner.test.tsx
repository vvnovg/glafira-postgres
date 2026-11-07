import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ExplainResult, Recommendation } from "../../../api/types";
import { connection, member } from "../../../test/fixtures";
import { planTree } from "../../../test/plans";
import { renderWithClient } from "../../../test/render";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return { ...orig, api: { explain: vi.fn(), listConnections: vi.fn(), hypoCheck: vi.fn() } };
});

import { api } from "../../../api/client";
import { countParams } from "../../../lib/useExplain";
import { PlanRunner } from "./PlanRunner";

const result = (patch: Partial<ExplainResult> = {}): ExplainResult => ({ mode: "plan", tier: "read", plan: planTree(), raw: [], relations: [{ schema: "shop", name: "orders" }], elapsedMs: 12, warnings: [], recommendations: [], recommendationsSkipped: [], ...patch });
/** Подтверждение яруса записи — `ConfirmDialog` P2 (группа с именем-заголовком). */
const CONFIRM = { name: "Выполнить запрос и откатить" };

describe("PlanRunner", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", environment: "dev", readOnly: false })]);
  });

  it("форма из $N, GENERIC_PLAN на 16+ только для «Плана», «План» шлёт значения; ANALYZE чтения — без подтверждения", async () => {
    vi.mocked(api.explain).mockResolvedValue(result());
    const user = userEvent.setup();
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="SELECT * FROM t WHERE a = $1 AND b = $2" serverVersionNum={170_000} inRecovery={false} />);
    const generic = screen.getByRole("checkbox", { name: "GENERIC_PLAN (без значений)" });
    expect(generic).toBeChecked();
    // Поля значений видны и при GENERIC_PLAN: «План с выполнением» берёт их без снятия флажка.
    expect(screen.getByRole("textbox", { name: "$1" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "$2" })).toBeInTheDocument();
    expect(screen.getByText(/«План с выполнением» берёт значения из полей ниже/)).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("button", { name: "План с выполнением" })).toBeEnabled());
    // Кнопка называет режим: при поставленном флажке «План (общий)».
    expect(screen.queryByRole("button", { name: "План" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "План (общий)" }));
    await waitFor(() => expect(api.explain).toHaveBeenCalledWith("c", expect.objectContaining({ sql: "SELECT * FROM t WHERE a = $1 AND b = $2", mode: "plan", params: { kind: "generic" }, member: "pg-1", database: "perf" })));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    // ANALYZE прямо при поставленном флажке: значения из полей, пустое поле — NULL.
    await user.type(screen.getByRole("textbox", { name: "$1" }), "7");
    vi.mocked(api.explain).mockResolvedValue(result({ mode: "analyze" }));
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", params: { kind: "values", values: ["7", null] }, timeoutS: 30, forceWrite: false, writeConfirmed: false })));
    // Проба яруса перед ANALYZE — «План» по флажку (GENERIC_PLAN, без значений); значения уходят в само выполнение.
    const calls = vi.mocked(api.explain).mock.calls;
    expect(calls[calls.length - 2][1]).toMatchObject({ mode: "plan", params: { kind: "generic" } });
    // Флажок снят — «План» тоже со значениями.
    await user.click(generic);
    await user.click(screen.getByRole("button", { name: "План" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "plan", params: { kind: "values", values: ["7", null] } })));
    expect(screen.queryByText(/«План с выполнением» берёт значения из полей ниже/)).toBeNull();
    expect(screen.queryByRole("group", CONFIRM)).not.toBeInTheDocument();
    expect(screen.getByText(/История/)).toBeInTheDocument();
  });

  it("административная функция в плане пробы: ярус записи, окно подтверждения называет причину", async () => {
    vi.mocked(api.explain).mockResolvedValueOnce(result({ tier: "write", warnings: ["adminFunction"] }));
    const user = userEvent.setup();
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="SELECT pg_terminate_backend(pid) FROM pg_stat_activity" serverVersionNum={170_000} inRecovery={false} />);
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    const dialog = await screen.findByRole("group", CONFIRM);
    expect(within(dialog).getByText(/Запрос вызывает административную функцию/)).toBeInTheDocument();
    expect(api.explain).toHaveBeenCalledTimes(1);
  });

  it("ярус записи без административной функции: строки о ней нет", async () => {
    vi.mocked(api.explain).mockResolvedValueOnce(result({ tier: "write" }));
    const user = userEvent.setup();
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="UPDATE t SET a = 1" serverVersionNum={170_000} inRecovery={false} />);
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    const dialog = await screen.findByRole("group", CONFIRM);
    expect(within(dialog).queryByText(/административную функцию/)).not.toBeInTheDocument();
  });

  it("ярус записи: подтверждение со списком таблиц, затем ANALYZE; EXPLAIN_WRITES предлагает выполнить как запись", async () => {
    vi.mocked(api.explain).mockResolvedValueOnce(result({ tier: "write", relations: [{ schema: "shop", name: "balances" }] })).mockResolvedValueOnce(result({ mode: "analyze", tier: "write" }));
    const user = userEvent.setup();
    const { client } = renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="UPDATE shop.balances SET balance = 0" serverVersionNum={170_000} inRecovery={false} />);
    const invalidate = vi.spyOn(client, "invalidateQueries");
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    const dialog = await screen.findByRole("group", CONFIRM);
    expect(within(dialog).getByText("shop.balances")).toBeInTheDocument();
    expect(within(dialog).getByText(/Строки изменятся и откатятся/)).toBeInTheDocument();
    expect(within(dialog).getByText(/NOTIFY не доставляется/)).toBeInTheDocument();
    expect(invalidate).not.toHaveBeenCalledWith({ queryKey: ["actions", "c"] });
    await user.click(within(dialog).getByRole("button", { name: "Выполнить и откатить" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", confirmation: { confirm: null, typedName: null }, writeConfirmed: true })));
    await waitFor(() => expect(screen.queryByRole("group", CONFIRM)).not.toBeInTheDocument());
    // Ярус записи пишет журнал действий — его список перечитывается, как после диалогов P2.
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["actions", "c"] });
    // Запрос-чтение с побочным эффектом: сервер отвечает EXPLAIN_WRITES → кнопка «Выполнить как запись».
    vi.mocked(api.explain).mockResolvedValueOnce(result()).mockRejectedValueOnce({ code: "EXPLAIN_WRITES", message: "writes", retryable: false, key: "errors.explain.writes" });
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    const asWrite = await screen.findByRole("button", { name: "Выполнить как запись" });
    vi.mocked(api.explain).mockResolvedValueOnce(result({ mode: "analyze", tier: "write" }));
    await user.click(asWrite);
    // Таблицы — из пробного плана перед отказом.
    const again = await screen.findByRole("group", CONFIRM);
    expect(within(again).getByText("shop.orders")).toBeInTheDocument();
    await user.click(within(again).getByRole("button", { name: "Выполнить и откатить" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", forceWrite: true, confirmation: { confirm: null, typedName: null }, writeConfirmed: true })));
    expect(api.explain).toHaveBeenCalledTimes(5);
  });

  it("prod: подтверждение требует имени подключения и передаёт его", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", environment: "prod", name: "billing" })]);
    vi.mocked(api.explain).mockResolvedValueOnce(result({ tier: "write" })).mockResolvedValueOnce(result({ mode: "analyze", tier: "write" }));
    const user = userEvent.setup();
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="DELETE FROM shop.orders" serverVersionNum={170_000} inRecovery={false} />);
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    const dialog = await screen.findByRole("group", CONFIRM);
    const run = within(dialog).getByRole("button", { name: "Выполнить и откатить" });
    expect(run).toBeDisabled();
    await user.type(within(dialog).getByLabelText("Имя подключения"), "billing");
    await user.click(run);
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", confirmation: { confirm: "billing", typedName: null } })));
  });

  it("на реплике ярус записи закрыт подсказкой; read-only профиль — то же", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", readOnly: true })]);
    vi.mocked(api.explain).mockResolvedValue(result({ tier: "write" }));
    const user = userEvent.setup();
    const first = renderWithClient(<PlanRunner connectionId="c" member="pg-2" database="perf" sql="UPDATE t SET a = 1" serverVersionNum={170_000} inRecovery />);
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    expect(await screen.findByText("Запись недоступна: подключение только для чтения")).toBeInTheDocument();
    expect(screen.queryByRole("group", CONFIRM)).not.toBeInTheDocument();
    first.unmount();
    // Профиль с правом записи, узел — реплика: подсказка выбрать primary, ANALYZE не уходит.
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", readOnly: false })]);
    renderWithClient(<PlanRunner connectionId="c" member="pg-2" database="perf" sql="UPDATE t SET a = 1" serverVersionNum={170_000} inRecovery />);
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    expect(await screen.findByText("Запись на реплике невозможна: выберите primary")).toBeInTheDocument();
    expect(screen.queryByRole("group", CONFIRM)).not.toBeInTheDocument();
    expect(vi.mocked(api.explain).mock.calls.every(([, r]) => r.mode === "plan")).toBe(true);
  });

  it("ошибка разбора показывает место в тексте; READ_ONLY не предлагает выполнить как запись", async () => {
    vi.mocked(api.explain).mockRejectedValueOnce({ code: "INVALID_INPUT", message: "The server rejected the query: syntax error", retryable: false, key: "errors.explain.server", params: { message: "syntax error at or near \"FORM\"" }, details: "position 10" });
    const user = userEvent.setup();
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql={"SELECT 1\nFORM t"} serverVersionNum={170_000} inRecovery={false} />);
    await user.click(screen.getByRole("button", { name: "План" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("syntax error at or near");
    expect(screen.getByText("Ошибка в строке 2, позиция 1")).toBeInTheDocument();
    expect(screen.getByText("F", { selector: "mark" })).toBeInTheDocument();
    vi.mocked(api.explain).mockResolvedValueOnce(result({ tier: "write" })).mockRejectedValueOnce({ code: "READ_ONLY", message: "read-only", retryable: false, key: "errors.explain.serverReadOnly" });
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    await user.click(within(await screen.findByRole("group", CONFIRM)).getByRole("button", { name: "Выполнить и откатить" }));
    expect(await screen.findByText(/Сервер не принимает запись/)).toBeInTheDocument();
    expect(screen.queryByText("Ошибка в строке 2, позиция 1")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Выполнить как запись" })).not.toBeInTheDocument();
  });
});

describe("PlanRunner: вход пробы, реплика, загрузка профиля", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", environment: "dev", readOnly: false })]);
  });

  const runner = (sql: string, extra: Partial<Parameters<typeof PlanRunner>[0]> = {}) => (
    <PlanRunner connectionId="c" member="pg-1" database="perf" sql={sql} serverVersionNum={170_000} inRecovery={false} {...extra} />
  );

  it("правка SQL при открытом подтверждении закрывает его: ANALYZE с подтверждением не уходит", async () => {
    vi.mocked(api.explain).mockResolvedValue(result({ tier: "write", relations: [{ schema: "shop", name: "balances" }] }));
    const user = userEvent.setup();
    const { rerender } = renderWithClient(runner("UPDATE shop.balances SET balance = 0"));
    await user.click(await screen.findByRole("button", { name: "План с выполнением" }));
    expect(await screen.findByRole("group", CONFIRM)).toBeInTheDocument();
    rerender(runner("UPDATE shop.balances SET balance = 1"));
    await waitFor(() => expect(screen.queryByRole("group", CONFIRM)).toBeNull());
    // Возврат к прежнему тексту диалог не воскрешает: его нужно получить новой пробой.
    rerender(runner("UPDATE shop.balances SET balance = 0"));
    expect(screen.queryByRole("group", CONFIRM)).toBeNull();
    expect(vi.mocked(api.explain).mock.calls.every(([, r]) => r.mode === "plan")).toBe(true);
  });

  it("проба, вернувшаяся после правки текста, диалог не открывает; правка значения снимает «Выполнить как запись»", async () => {
    let probe: (v: ExplainResult) => void = () => {};
    vi.mocked(api.explain).mockReturnValueOnce(new Promise((r) => { probe = r; }));
    const user = userEvent.setup();
    const { rerender } = renderWithClient(runner("UPDATE t SET a = $1"));
    await user.click(await screen.findByRole("button", { name: "План с выполнением" }));
    rerender(runner("UPDATE t SET a = $1 WHERE false"));
    await act(async () => probe(result({ tier: "write" })));
    expect(screen.queryByRole("group", CONFIRM)).toBeNull();
    expect(api.explain).toHaveBeenCalledTimes(1);
    // Чтение записало → подсказка «как запись»; новое значение $1 — уже другой запрос, подсказка снята.
    vi.mocked(api.explain).mockResolvedValueOnce(result()).mockRejectedValueOnce({ code: "EXPLAIN_WRITES", message: "writes", retryable: false, key: "errors.explain.writes" });
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    expect(await screen.findByRole("button", { name: "Выполнить как запись" })).toBeInTheDocument();
    await user.type(screen.getByRole("textbox", { name: "$1" }), "5");
    expect(screen.queryByRole("button", { name: "Выполнить как запись" })).toBeNull();
  });

  it("EXPLAIN_WRITES уже от пробы — «Выполнить как запись» без строки «Таблицы:», ANALYZE с forceWrite", async () => {
    vi.mocked(api.explain).mockRejectedValueOnce({ code: "EXPLAIN_WRITES", message: "writes", retryable: false, key: "errors.explain.writes" });
    const user = userEvent.setup();
    renderWithClient(runner("SELECT f()"));
    await user.click(await screen.findByRole("button", { name: "План с выполнением" }));
    await user.click(await screen.findByRole("button", { name: "Выполнить как запись" }));
    const dialog = await screen.findByRole("group", CONFIRM);
    expect(within(dialog).queryByText("Таблицы:")).toBeNull();
    expect(within(dialog).getByText("Данные изменятся и откатятся.")).toBeInTheDocument();
    vi.mocked(api.explain).mockResolvedValueOnce(result({ mode: "analyze", tier: "write", relations: [] }));
    await user.click(within(dialog).getByRole("button", { name: "Выполнить и откатить" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", forceWrite: true, sql: "SELECT f()", writeConfirmed: true })));
  });

  it("профиль ещё читается: кнопки выключены, подсказки read-only нет", async () => {
    vi.mocked(api.listConnections).mockReturnValue(new Promise(() => {}));
    renderWithClient(runner("UPDATE t SET a = 1"));
    expect(screen.getByRole("button", { name: "План" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "План с выполнением" })).toBeDisabled();
    expect(screen.getByText("Загрузка…")).toBeInTheDocument();
    expect(screen.queryByText("Запись недоступна: подключение только для чтения")).toBeNull();
  });

  it("лидер и ярус чтения: ANALYZE по умолчанию на первой доступной реплике, лидера можно вернуть", async () => {
    const members = [member("pg-1", { role: "primary" }), member("pg-3", { role: "replica", reachable: false }), member("pg-2", { role: "replica" })];
    vi.mocked(api.explain).mockResolvedValue(result());
    const user = userEvent.setup();
    renderWithClient(runner("SELECT 1", { members }));
    expect(screen.queryByRole("combobox", { name: "Выполнить на узле" })).toBeNull();
    await user.click(await screen.findByRole("button", { name: "План" }));
    const on = await screen.findByRole("combobox", { name: "Выполнить на узле" });
    // Недоступная реплика не предлагается; по умолчанию — первая доступная.
    expect(within(on).getAllByRole("option").map((o) => o.textContent)).toEqual(["pg-1", "pg-2"]);
    expect(on).toHaveValue("pg-2");
    expect(screen.getByText("на реплике план может отличаться")).toBeInTheDocument();
    vi.mocked(api.explain).mockResolvedValueOnce(result()).mockResolvedValueOnce(result({ mode: "analyze" }));
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", member: "pg-2" })));
    // Проба — на узле вкладки.
    const calls = vi.mocked(api.explain).mock.calls;
    expect(calls[calls.length - 2][1]).toMatchObject({ mode: "plan", member: "pg-1" });
    // Вернули лидера — ANALYZE на нём, пометки нет.
    await user.selectOptions(screen.getByRole("combobox", { name: "Выполнить на узле" }), "pg-1");
    expect(screen.queryByText("на реплике план может отличаться")).toBeNull();
    vi.mocked(api.explain).mockResolvedValueOnce(result()).mockResolvedValueOnce(result({ mode: "analyze" }));
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", member: "pg-1" })));
  });

  it("без доступных реплик ANALYZE чтения — на узле вкладки, выбора нет", async () => {
    const members = [member("pg-1", { role: "primary" }), member("pg-2", { role: "replica", reachable: false })];
    vi.mocked(api.explain).mockResolvedValueOnce(result()).mockResolvedValueOnce(result({ mode: "analyze" }));
    const user = userEvent.setup();
    renderWithClient(runner("SELECT 1", { members }));
    await user.click(await screen.findByRole("button", { name: "План с выполнением" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", member: "pg-1" })));
    expect(screen.queryByRole("combobox", { name: "Выполнить на узле" })).toBeNull();
  });

  it("реплику не предлагают: узел — сама реплика или проба показала запись", async () => {
    const members = [member("pg-1", { role: "primary" }), member("pg-2", { role: "replica" })];
    vi.mocked(api.explain).mockResolvedValue(result());
    const user = userEvent.setup();
    const first = renderWithClient(<PlanRunner connectionId="c" member="pg-2" database="perf" sql="SELECT 1" serverVersionNum={170_000} inRecovery members={members} />);
    await user.click(await screen.findByRole("button", { name: "План" }));
    await screen.findByRole("treegrid", { name: "Дерево плана" });
    expect(screen.queryByRole("combobox", { name: "Выполнить на узле" })).toBeNull();
    first.unmount();
    vi.mocked(api.explain).mockResolvedValue(result({ tier: "write" }));
    renderWithClient(runner("UPDATE t SET a = 1", { members }));
    await user.click(await screen.findByRole("button", { name: "План" }));
    await screen.findByRole("treegrid", { name: "Дерево плана" });
    expect(screen.queryByRole("combobox", { name: "Выполнить на узле" })).toBeNull();
  });

  it("ANALYZE чтения с лидера на реплике: узел — в сводке и истории, onResult получает узел запуска", async () => {
    const members = [member("pg-1", { role: "primary" }), member("pg-2", { role: "replica" })];
    vi.mocked(api.explain).mockResolvedValueOnce(result()).mockResolvedValueOnce(result({ mode: "analyze" }));
    const onResult = vi.fn();
    const user = userEvent.setup();
    renderWithClient(runner("SELECT 1", { members, onResult }));
    // Сразу «План с выполнением», без «Плана»: чтение уходит на первую доступную реплику.
    await user.click(await screen.findByRole("button", { name: "План с выполнением" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", member: "pg-2" })));
    expect(await screen.findByText("Узел: pg-2")).toBeInTheDocument();
    expect(onResult).toHaveBeenLastCalledWith(expect.objectContaining({ mode: "analyze" }), "pg-2", expect.objectContaining({ mode: "analyze", member: "pg-2" }));
    // «План» — на узле вкладки: сводка без узла; в истории узел назван только у запуска на реплике.
    vi.mocked(api.explain).mockResolvedValueOnce(result());
    await user.click(screen.getByRole("button", { name: "План" }));
    await waitFor(() => expect(onResult).toHaveBeenLastCalledWith(expect.objectContaining({ mode: "plan" }), "pg-1", expect.objectContaining({ mode: "plan", member: "pg-1" })));
    expect(screen.queryByText("Узел: pg-2")).toBeNull();
    expect(screen.getByRole("button", { name: /^#1 с выполнением, чтение, на pg-2, / })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^#2 план, чтение, \d/ })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^#1 / }));
    expect(screen.getByText("Узел: pg-2")).toBeInTheDocument();
  });

  it("место ошибки — в тексте упавшего запуска, даже если ответ пришёл после правки; следующая правка снимает ошибку", async () => {
    let fail: (e: unknown) => void = () => {};
    vi.mocked(api.explain).mockReturnValueOnce(new Promise((_, reject) => { fail = reject; }));
    const user = userEvent.setup();
    const { rerender } = renderWithClient(runner("SELECT 1\nFORM t"));
    await user.click(await screen.findByRole("button", { name: "План" }));
    rerender(runner("SELECT 12345678\nFROM t"));
    await act(async () => fail({ code: "INVALID_INPUT", message: "syntax error", retryable: false, key: "errors.explain.server", params: { message: "syntax error at or near \"FORM\"" }, details: "position 10" }));
    expect(await screen.findByText("Ошибка в строке 2, позиция 1")).toBeInTheDocument();
    expect(screen.getByText("F", { selector: "mark" })).toBeInTheDocument();
    rerender(runner("SELECT 12345678\nFROM t2"));
    await waitFor(() => expect(screen.queryByText("Ошибка в строке 2, позиция 1")).toBeNull());
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("параметров больше 100: вместо полей — сообщение; на 16+ «План» идёт с GENERIC_PLAN, запуски со значениями закрыты", async () => {
    const sql = `SELECT ${Array.from({ length: 101 }, (_, i) => `$${i + 1}`).join(", ")}`;
    vi.mocked(api.explain).mockResolvedValue(result());
    const user = userEvent.setup();
    const first = renderWithClient(runner(sql));
    expect(screen.getByText(/Параметров больше 100 \(всего: 101\)/)).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "$1" })).toBeNull();
    const plan = await screen.findByRole("button", { name: "План (общий)" });
    await waitFor(() => expect(plan).toBeEnabled());
    expect(screen.getByRole("button", { name: "План с выполнением" })).toBeDisabled();
    await user.click(plan);
    await waitFor(() => expect(api.explain).toHaveBeenCalledWith("c", expect.objectContaining({ mode: "plan", params: { kind: "generic" } })));
    // Без GENERIC_PLAN «Плану» тоже нужны значения — закрыт и он.
    await user.click(screen.getByRole("checkbox", { name: "GENERIC_PLAN (без значений)" }));
    expect(screen.getByRole("button", { name: "План" })).toBeDisabled();
    expect(screen.queryByRole("textbox", { name: "$1" })).toBeNull();
    first.unmount();
    // PostgreSQL 15 и номер за пределом протокола: GENERIC_PLAN нет — закрыты оба запуска, «Получить план» не ждёт.
    vi.mocked(api.explain).mockClear();
    const onAutoRun = vi.fn();
    renderWithClient(runner("SELECT $70000", { serverVersionNum: 150_000, autoRun: true, onAutoRun }));
    expect(screen.getByText(/Параметров больше 100 \(всего: 65535\)/)).toBeInTheDocument();
    await waitFor(() => expect(onAutoRun).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("button", { name: "План" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "План с выполнением" })).toBeDisabled();
    expect(api.explain).not.toHaveBeenCalled();
  });

  it("ответ после размонтирования отбрасывается; autoRun запускает одну пробу и сообщает об этом", async () => {
    let finish: (v: ExplainResult) => void = () => {};
    vi.mocked(api.explain).mockReturnValueOnce(new Promise((r) => { finish = r; }));
    const onResult = vi.fn();
    const onAutoRun = vi.fn();
    const view = renderWithClient(runner("SELECT 1", { onResult, autoRun: true, onAutoRun }));
    await waitFor(() => expect(api.explain).toHaveBeenCalledWith("c", expect.objectContaining({ mode: "plan", sql: "SELECT 1" })));
    expect(onAutoRun).toHaveBeenCalledTimes(1);
    view.unmount();
    await act(async () => finish(result()));
    expect(onResult).not.toHaveBeenCalled();
    expect(api.explain).toHaveBeenCalledTimes(1);
  });
});

describe("PlanRunner: отказы и режим", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", environment: "dev", readOnly: false })]);
  });

  it("«План» — главная кнопка запуска с ⌘⏎ (спец. переноса U1 §1), «План с выполнением» — обычная", async () => {
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="SELECT 1" serverVersionNum={170_000} inRecovery={false} />);
    const plan = screen.getByRole("button", { name: "План" });
    expect(plan).toHaveClass("primary");
    expect(plan).toHaveAttribute("data-action", "run");
    expect(plan.querySelector('svg[data-icon="play"]')).not.toBeNull();
    expect(plan.querySelector(".kbd")).not.toBeNull();
    const analyze = screen.getByRole("button", { name: "План с выполнением" });
    expect(analyze).not.toHaveClass("primary");
    expect(analyze).not.toHaveAttribute("data-action");
  });

  it("ошибка запуска убирает прошлый результат с экрана до нового успешного; история остаётся", async () => {
    vi.mocked(api.explain).mockResolvedValueOnce(result()).mockResolvedValueOnce(result());
    const user = userEvent.setup();
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="SELECT 1" serverVersionNum={170_000} inRecovery={false} />);
    await user.click(screen.getByRole("button", { name: "План" }));
    await user.click(screen.getByRole("button", { name: "План" }));
    expect(await screen.findByText(/История/)).toBeInTheDocument();
    expect(screen.getByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    vi.mocked(api.explain).mockRejectedValueOnce({ code: "INVALID_INPUT", message: "x", retryable: false, key: "errors.explain.oneStatement" });
    await user.click(screen.getByRole("button", { name: "План" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Введите ровно одну инструкцию");
    expect(screen.queryByRole("treegrid", { name: "Дерево плана" })).toBeNull();
    expect(screen.getByText(/История/)).toBeInTheDocument();
    // Выбор запуска из истории снимает отказ.
    await user.click(screen.getAllByRole("button", { name: /^#\d/ })[0]);
    expect(screen.getByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("после ошибки правка текста или значений не возвращает прошлый результат; новый успешный запуск и история — возвращают", async () => {
    vi.mocked(api.explain).mockResolvedValueOnce(result()).mockResolvedValueOnce(result());
    const user = userEvent.setup();
    const { rerender } = renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="SELECT $1" serverVersionNum={150_000} inRecovery={false} />);
    await user.type(screen.getByRole("textbox", { name: "$1" }), "1");
    await user.click(screen.getByRole("button", { name: "План" }));
    await user.click(screen.getByRole("button", { name: "План" }));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    vi.mocked(api.explain).mockRejectedValueOnce({ code: "INVALID_INPUT", message: "x", retryable: false, key: "errors.explain.oneStatement" });
    await user.click(screen.getByRole("button", { name: "План" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    // Правка значения и текста снимает баннер, но прошлый результат к новому входу не относится.
    await user.type(screen.getByRole("textbox", { name: "$1" }), "2");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByRole("treegrid", { name: "Дерево плана" })).toBeNull();
    rerender(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="SELECT $1 + 1" serverVersionNum={150_000} inRecovery={false} />);
    expect(screen.queryByRole("treegrid", { name: "Дерево плана" })).toBeNull();
    expect(screen.getByText(/История/)).toBeInTheDocument();
    await user.click(screen.getAllByRole("button", { name: /^#\d/ })[0]);
    expect(screen.getByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
  });

  it("после отказа новый успешный запуск показывает результат", async () => {
    vi.mocked(api.explain).mockRejectedValueOnce({ code: "INVALID_INPUT", message: "x", retryable: false, key: "errors.explain.oneStatement" }).mockResolvedValueOnce(result());
    const user = userEvent.setup();
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="SELECT 1" serverVersionNum={170_000} inRecovery={false} />);
    await user.click(screen.getByRole("button", { name: "План" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "План" }));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
  });

  it("отказ в записи — красным баннером, прошлый результат скрыт", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", readOnly: true })]);
    vi.mocked(api.explain).mockResolvedValueOnce(result()).mockResolvedValueOnce(result({ tier: "write" }));
    const user = userEvent.setup();
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="UPDATE t SET a = 1" serverVersionNum={170_000} inRecovery={false} />);
    await user.click(screen.getByRole("button", { name: "План" }));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    expect(await screen.findByText("Запись недоступна: подключение только для чтения")).toHaveClass("error-banner");
    expect(screen.queryByRole("treegrid", { name: "Дерево плана" })).toBeNull();
  });

  it("параметров больше 100: сообщение красным", async () => {
    const sql = `SELECT ${Array.from({ length: 101 }, (_, i) => `$${i + 1}`).join(", ")}`;
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql={sql} serverVersionNum={170_000} inRecovery={false} />);
    expect(screen.getByText(/Параметров больше 100 \(всего: 101\)/)).toHaveClass("error-banner");
  });

  it("подпись флажка GENERIC_PLAN не приглушена", () => {
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="SELECT $1" serverVersionNum={170_000} inRecovery={false} />);
    expect(screen.getByRole("checkbox", { name: "GENERIC_PLAN (без значений)" }).closest("label")).toHaveClass("generic");
  });

  it("сводка результата начинается с режима запуска", async () => {
    vi.mocked(api.explain).mockResolvedValue(result({ mode: "plan" }));
    const user = userEvent.setup();
    const { container } = renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="SELECT 1" serverVersionNum={170_000} inRecovery={false} />);
    await user.click(screen.getByRole("button", { name: "План" }));
    await screen.findByRole("treegrid", { name: "Дерево плана" });
    expect(container.querySelector(".plan-summary-part")).toHaveTextContent("План");
  });
});

describe("PlanRunner: кнопки запроса в ряду формы и подстановка текста (смоук P4b R1b, R6, R11)", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", environment: "dev", readOnly: false })]);
  });

  const runner = (sql: string, extra: Partial<Parameters<typeof PlanRunner>[0]> = {}) => (
    <PlanRunner connectionId="c" member="pg-1" database="perf" sql={sql} serverVersionNum={170_000} inRecovery={false} {...extra} />
  );
  const tree = () => screen.queryByRole("treegrid", { name: "Дерево плана" });
  /** Ответ, который приходит, когда тест скажет. */
  const pending = () => {
    let finish: (v: ExplainResult) => void = () => {};
    vi.mocked(api.explain).mockReturnValueOnce(new Promise((r) => { finish = r; }));
    return (v = result()) => act(async () => finish(v));
  };

  it("`extraActions` — в ряду формы рядом с «План» и до, и после результата; в ряду копирования плана их нет", async () => {
    vi.mocked(api.explain).mockResolvedValue(result());
    const user = userEvent.setup();
    renderWithClient(runner("SELECT 1", { extraActions: <button type="button">LLM</button> }));
    const row = () => screen.getByRole("button", { name: "План" }).closest(".toolbar");
    expect(row()).toContainElement(screen.getByRole("button", { name: "LLM" }));
    await user.click(screen.getByRole("button", { name: "План" }));
    await screen.findByRole("treegrid", { name: "Дерево плана" });
    expect(screen.getAllByRole("button", { name: "LLM" })).toHaveLength(1);
    expect(row()).toContainElement(screen.getByRole("button", { name: "LLM" }));
    expect(screen.getByRole("button", { name: "Копировать как текст" }).closest(".toolbar")).not.toContainElement(screen.getByRole("button", { name: "LLM" }));
  });

  it("подстановка сразу убирает результат и строит «План» по новому тексту, как кнопка; родитель узнаёт, что она принята", async () => {
    vi.mocked(api.explain).mockResolvedValueOnce(result());
    const user = userEvent.setup();
    const onSubstitution = vi.fn();
    const { rerender } = renderWithClient(runner("SELECT 1"));
    await user.click(screen.getByRole("button", { name: "План" }));
    await screen.findByRole("treegrid", { name: "Дерево плана" });
    const finish = pending();
    rerender(runner("SELECT 2", { substitution: { nonce: 1, plan: true }, onSubstitution }));
    // Прежний план к новому тексту не относится — его нет сразу, ещё до ответа.
    expect(tree()).toBeNull();
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ sql: "SELECT 2", mode: "plan", params: { kind: "none" }, member: "pg-1", database: "perf" })));
    expect(onSubstitution).toHaveBeenCalledTimes(1);
    await finish();
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    // Та же подстановка (снята родителем или нет) второй раз не запускается; «План с выполнением» — никогда.
    rerender(runner("SELECT 2", { substitution: { nonce: 1, plan: true }, onSubstitution }));
    rerender(runner("SELECT 2", { substitution: null, onSubstitution }));
    expect(api.explain).toHaveBeenCalledTimes(2);
    expect(api.explain).not.toHaveBeenCalledWith("c", expect.objectContaining({ mode: "analyze" }));
  });

  it("смонтированный заново с уже принятой подстановкой (родитель снял её) — без запуска", async () => {
    vi.mocked(api.explain).mockResolvedValue(result());
    renderWithClient(runner("SELECT 2", { substitution: null }));
    await act(() => new Promise((r) => setTimeout(r, 0)));
    expect(api.explain).not.toHaveBeenCalled();
  });

  it("подстановка без «Плана» (без autoPlan) только убирает результат; запуск, начатый до неё, уходит в историю, а не на экран", async () => {
    vi.mocked(api.explain).mockResolvedValueOnce(result());
    const user = userEvent.setup();
    const onSubstitution = vi.fn();
    const { rerender } = renderWithClient(runner("SELECT 1"));
    await user.click(screen.getByRole("button", { name: "План" }));
    await screen.findByRole("treegrid", { name: "Дерево плана" });
    const finish = pending();
    await user.click(screen.getByRole("button", { name: "План" }));
    rerender(runner("SELECT 2", { substitution: { nonce: 7, plan: false }, onSubstitution }));
    expect(tree()).toBeNull();
    await waitFor(() => expect(onSubstitution).toHaveBeenCalledTimes(1));
    await finish();
    expect(await screen.findByText(/История/)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^#\d/ })).toHaveLength(2);
    expect(tree()).toBeNull();
    expect(api.explain).toHaveBeenCalledTimes(2);
    // Из истории запуск прежнего текста можно открыть.
    await user.click(screen.getAllByRole("button", { name: /^#\d/ })[0]);
    expect(tree()).not.toBeNull();
  });

  it("подстановка во время запуска: «План» нового текста — после него; ответ прежнего на экран не попадает", async () => {
    const user = userEvent.setup();
    const { rerender } = renderWithClient(runner("SELECT 1"));
    const finishOld = pending();
    await user.click(screen.getByRole("button", { name: "План" }));
    const finishNew = pending();
    rerender(runner("SELECT 2", { substitution: { nonce: 1, plan: true } }));
    expect(api.explain).toHaveBeenCalledTimes(1);
    await finishOld();
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ sql: "SELECT 2", mode: "plan" })));
    expect(tree()).toBeNull();
    await finishNew();
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
  });

  it("ошибка запуска, начатого до подстановки, баннером под новым текстом не встаёт", async () => {
    let fail: (e: unknown) => void = () => {};
    vi.mocked(api.explain).mockReturnValueOnce(new Promise((_r, j) => { fail = j; }));
    const user = userEvent.setup();
    const { rerender } = renderWithClient(runner("SELECT 1"));
    await user.click(screen.getByRole("button", { name: "План" }));
    rerender(runner("SELECT 2", { substitution: { nonce: 1, plan: false } }));
    await act(async () => fail({ code: "INVALID_INPUT", message: "x", retryable: false, key: "errors.explain.oneStatement" }));
    expect(screen.queryByRole("alert")).toBeNull();
    await waitFor(() => expect(screen.getByRole("button", { name: "План" })).toBeEnabled());
  });

  it("текст с $N: на 16+ — общий план, GENERIC_PLAN снова включается; без него (15) — только очистка", async () => {
    vi.mocked(api.explain).mockResolvedValue(result());
    const user = userEvent.setup();
    const onSubstitution = vi.fn();
    const first = renderWithClient(runner("SELECT $1"));
    await user.click(screen.getByRole("checkbox", { name: "GENERIC_PLAN (без значений)" }));
    first.rerender(runner("SELECT $1 + $2", { substitution: { nonce: 1, plan: true }, onSubstitution }));
    await waitFor(() => expect(api.explain).toHaveBeenCalledWith("c", expect.objectContaining({ sql: "SELECT $1 + $2", mode: "plan", params: { kind: "generic" } })));
    expect(screen.getByRole("checkbox", { name: "GENERIC_PLAN (без значений)" })).toBeChecked();
    first.unmount();
    vi.mocked(api.explain).mockClear();
    const second = renderWithClient(runner("SELECT 1", { serverVersionNum: 150_000 }));
    await user.click(screen.getByRole("button", { name: "План" }));
    await screen.findByRole("treegrid", { name: "Дерево плана" });
    second.rerender(runner("SELECT $1", { serverVersionNum: 150_000, substitution: { nonce: 2, plan: true }, onSubstitution }));
    expect(tree()).toBeNull();
    await waitFor(() => expect(onSubstitution).toHaveBeenCalledTimes(2));
    expect(api.explain).toHaveBeenCalledTimes(1);
  });
});

describe("countParams", () => {
  it("наибольший номер $N, как в Rust", () => {
    expect(countParams("SELECT 1")).toBe(0);
    expect(countParams("SELECT $2, $1, $10")).toBe(10);
    expect(countParams("SELECT $1::int, ($2)")).toBe(2);
    expect(countParams("SELECT $")).toBe(0);
  });

  // Те же случаи, что у `explain::params::count_params` в Rust.
  it("пропускает строки, идентификаторы, комментарии и долларовые кавычки", () => {
    expect(countParams("SELECT '$5', $1")).toBe(1);
    expect(countParams("SELECT 'it''s $5', $1")).toBe(1);
    expect(countParams(String.raw`SELECT E'\'$5', $1`)).toBe(1);
    expect(countParams(String.raw`SELECT some_e'\', $2`)).toBe(2);
    expect(countParams('SELECT "col$5", $1')).toBe(1);
    expect(countParams('SELECT "a""$5", $1')).toBe(1);
    expect(countParams("SELECT a$1, b$2x, $1")).toBe(1);
    expect(countParams("SELECT x_$3, y9$4")).toBe(0);
    expect(countParams("SELECT $1 -- $5")).toBe(1);
    expect(countParams("SELECT 1 -- $5\n + $2")).toBe(2);
    expect(countParams("SELECT /* $5 /* $6 */ $7 */ $1")).toBe(1);
    expect(countParams("DO $$ BEGIN END $$")).toBe(0);
    expect(countParams("SELECT $$text $5$$, $1")).toBe(1);
    expect(countParams("SELECT $f$ $5 $$ $6 $f$, $2")).toBe(2);
    expect(countParams("SELECT $_x1$ $5 $_x1$, $1")).toBe(1);
    expect(countParams("SELECT $1$, $2")).toBe(2);
    expect(countParams("SELECT 'open $5")).toBe(0);
    expect(countParams("SELECT $q$ open $5")).toBe(0);
    expect(countParams("SELECT /* open $5")).toBe(0);
    expect(countParams("SELECT 'é$5', таблица$6, $1")).toBe(1);
  });

  it("номер ограничен пределом протокола 65 535", () => {
    expect(countParams("SELECT $65535")).toBe(65_535);
    expect(countParams("SELECT $70000")).toBe(65_535);
    expect(countParams("SELECT $1234567")).toBe(65_535);
    expect(countParams("SELECT $99999999999999999999999, $1")).toBe(65_535);
  });

  it("рекомендации под сводкой: «Показать узел» раскрывает свёрнутого предка и выбирает узел; гипотеза — на входе запуска, не текущих полях", async () => {
    const rec: Recommendation = {
      id: "plan_seq_scan_filter", severity: "warning", nodeIds: [1, 2], params: { table: "shop.orders", rowsRead: 500_000, rowsReturned: 500, loops: 1, columns: "status" },
      command: "CREATE INDEX CONCURRENTLY ON \"shop\".\"orders\" (\"status\");", hypo: { indexDdl: "CREATE INDEX ON \"shop\".\"orders\" (\"status\")" },
    };
    vi.mocked(api.explain).mockResolvedValue(result({ recommendations: [rec], recommendationsSkipped: ["plan_spill"] }));
    vi.mocked(api.hypoCheck).mockResolvedValue({ indexName: "<1>btree_orders_status", estimatedBytes: 8192, costBefore: 100, costAfter: 10, used: true, planAfter: planTree() });
    const user = userEvent.setup();
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="SELECT * FROM orders" serverVersionNum={170_000} inRecovery={false} />);
    await user.click(screen.getByText("search_path"));
    await user.type(screen.getByRole("textbox", { name: "search_path" }), "shop");
    await waitFor(() => expect(screen.getByRole("button", { name: "План" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "План" }));
    const block = await screen.findByRole("region", { name: "Рекомендации" });
    expect(within(block).getByRole("heading", { name: "Индекс по колонкам фильтра: shop.orders" })).toBeInTheDocument();
    expect(within(block).getByText("Выполните план с выполнением, чтобы проверить ещё 1 правило")).toBeInTheDocument();
    // Свернуть корень: узел 1 скрыт; «Показать узел» раскрывает и выбирает его.
    const tree = screen.getByRole("treegrid", { name: "Дерево плана" });
    await user.click(within(tree).getAllByRole("button", { name: "Свернуть" })[0]);
    expect(within(tree).getAllByRole("row")).toHaveLength(2);
    await user.click(within(block).getByRole("button", { name: "Показать узел" }));
    const selected = within(tree).getAllByRole("row").find((r) => r.getAttribute("aria-selected") === "true");
    expect(selected).toHaveAttribute("data-node-id", "1");
    expect(within(tree).getAllByRole("row")).toHaveLength(4);
    // Узлы рекомендации помечены; клик по строке пометку снимает.
    expect(within(selected!).getByText("рекомендация")).toBeInTheDocument();
    expect(within(tree).getAllByText("рекомендация")).toHaveLength(2);
    const second = within(tree).getAllByRole("row").find((r) => r.getAttribute("data-node-id") === "2")!;
    await user.click(second.querySelector<HTMLButtonElement>("button.link")!);
    expect(second).toHaveAttribute("aria-selected", "true");
    expect(within(tree).queryAllByText("рекомендация")).toHaveLength(0);
    // Поле search_path правится после запуска — гипотеза идёт с тем, что было у запуска.
    await user.clear(screen.getByRole("textbox", { name: "search_path" }));
    await user.click(within(block).getByRole("button", { name: "Проверить гипотезу" }));
    await waitFor(() =>
      expect(api.hypoCheck).toHaveBeenCalledWith("c", { member: "pg-1", database: "perf", sql: "SELECT * FROM orders", params: { kind: "none" }, searchPath: "shop", indexDdl: rec.hypo!.indexDdl }),
    );
    const dialog = await screen.findByRole("dialog", { name: "Проверка гипотезы" });
    expect(await within(dialog).findByText("Индекс использован")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
