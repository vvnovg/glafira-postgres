import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ExplainResult } from "../../../api/types";
import { connection, member } from "../../../test/fixtures";
import { planTree } from "../../../test/plans";
import { renderWithClient } from "../../../test/render";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return { ...orig, api: { explain: vi.fn(), listConnections: vi.fn() } };
});

import { api } from "../../../api/client";
import { countParams } from "../../../lib/useExplain";
import { PlanRunner } from "./PlanRunner";

const result = (patch: Partial<ExplainResult> = {}): ExplainResult => ({ mode: "plan", tier: "read", plan: planTree(), raw: [], relations: [{ schema: "shop", name: "orders" }], elapsedMs: 12, warnings: [], ...patch });
/** Подтверждение яруса записи — `ConfirmDialog` P2 (группа с именем-заголовком). */
const CONFIRM = { name: "Выполнить запрос и откатить" };

describe("PlanRunner", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", environment: "dev", readOnly: false })]);
  });

  it("форма из $N, GENERIC_PLAN на 16+, «План» шлёт значения; ANALYZE чтения — без подтверждения", async () => {
    vi.mocked(api.explain).mockResolvedValue(result());
    const user = userEvent.setup();
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="SELECT * FROM t WHERE a = $1 AND b = $2" serverVersionNum={170_000} inRecovery={false} />);
    expect(screen.getByRole("textbox", { name: "$1" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "$2" })).toBeInTheDocument();
    const generic = screen.getByRole("checkbox", { name: "GENERIC_PLAN (без значений)" });
    expect(generic).toBeChecked();
    await user.click(screen.getByRole("button", { name: "План" }));
    await waitFor(() => expect(api.explain).toHaveBeenCalledWith("c", expect.objectContaining({ sql: "SELECT * FROM t WHERE a = $1 AND b = $2", mode: "plan", params: { kind: "generic" }, member: "pg-1", database: "perf" })));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    await user.click(generic);
    await user.type(screen.getByRole("textbox", { name: "$1" }), "7");
    vi.mocked(api.explain).mockResolvedValue(result({ mode: "analyze" }));
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", params: { kind: "values", values: ["7", null] }, timeoutS: 30, forceWrite: false })));
    expect(screen.queryByRole("group", CONFIRM)).not.toBeInTheDocument();
    expect(screen.getByText(/История/)).toBeInTheDocument();
  });

  it("ярус записи: подтверждение со списком таблиц, затем ANALYZE; EXPLAIN_WRITES предлагает выполнить как запись", async () => {
    vi.mocked(api.explain).mockResolvedValueOnce(result({ tier: "write", relations: [{ schema: "shop", name: "balances" }] })).mockResolvedValueOnce(result({ mode: "analyze", tier: "write" }));
    const user = userEvent.setup();
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="UPDATE shop.balances SET balance = 0" serverVersionNum={170_000} inRecovery={false} />);
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    const dialog = await screen.findByRole("group", CONFIRM);
    expect(within(dialog).getByText("shop.balances")).toBeInTheDocument();
    expect(within(dialog).getByText(/Строки изменятся и откатятся/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Выполнить и откатить" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", confirmation: { confirm: null, typedName: null } })));
    await waitFor(() => expect(screen.queryByRole("group", CONFIRM)).not.toBeInTheDocument());
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
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", forceWrite: true, confirmation: { confirm: null, typedName: null } })));
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
    expect(within(dialog).getByText("Строки изменятся и откатятся.")).toBeInTheDocument();
    vi.mocked(api.explain).mockResolvedValueOnce(result({ mode: "analyze", tier: "write", relations: [] }));
    await user.click(within(dialog).getByRole("button", { name: "Выполнить и откатить" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", forceWrite: true, sql: "SELECT f()" })));
  });

  it("профиль ещё читается: кнопки выключены, подсказки read-only нет", async () => {
    vi.mocked(api.listConnections).mockReturnValue(new Promise(() => {}));
    renderWithClient(runner("UPDATE t SET a = 1"));
    expect(screen.getByRole("button", { name: "План" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "План с выполнением" })).toBeDisabled();
    expect(screen.getByText("Загрузка…")).toBeInTheDocument();
    expect(screen.queryByText("Запись недоступна: подключение только для чтения")).toBeNull();
  });

  it("лидер и ярус чтения: ANALYZE можно выполнить на реплике (по умолчанию — текущий узел)", async () => {
    const members = [member("pg-1", { role: "primary" }), member("pg-2", { role: "replica" }), member("pg-3", { role: "replica", reachable: false })];
    vi.mocked(api.explain).mockResolvedValue(result());
    const user = userEvent.setup();
    renderWithClient(runner("SELECT 1", { members }));
    expect(screen.queryByRole("combobox", { name: "Выполнить на узле" })).toBeNull();
    await user.click(await screen.findByRole("button", { name: "План" }));
    const on = await screen.findByRole("combobox", { name: "Выполнить на узле" });
    expect(on).toHaveValue("pg-1");
    expect(within(on).getAllByRole("option").map((o) => o.textContent)).toEqual(["pg-1", "pg-2"]);
    await user.selectOptions(on, "pg-2");
    expect(screen.getByText("на реплике план может отличаться")).toBeInTheDocument();
    vi.mocked(api.explain).mockResolvedValueOnce(result()).mockResolvedValueOnce(result({ mode: "analyze" }));
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", member: "pg-2" })));
    // Проба — на узле вкладки.
    const calls = vi.mocked(api.explain).mock.calls;
    expect(calls[calls.length - 2][1]).toMatchObject({ mode: "plan", member: "pg-1" });
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

describe("countParams", () => {
  it("наибольший номер $N, как в Rust: $$ — не параметр, номер ограничен", () => {
    expect(countParams("SELECT 1")).toBe(0);
    expect(countParams("SELECT $2, $1, $10")).toBe(10);
    expect(countParams("DO $$ BEGIN END $$")).toBe(0);
    expect(countParams("SELECT $1234567")).toBe(123_456);
  });
});
