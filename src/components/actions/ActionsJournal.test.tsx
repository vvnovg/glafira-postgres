import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../../api/client";
import type { ActionRecord } from "../../api/types";
import { renderWithClient } from "../../test/render";
import { ActionsJournal } from "./ActionsJournal";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { listActions: vi.fn() } };
});

const terminate: ActionRecord = {
  ts: new Date(2026, 9, 25, 12, 0, 5).getTime(),
  action: "terminate",
  connection: { id: "c", name: "billing-prod", environment: "prod" },
  member: "pg-1",
  prodConfirmed: true,
  outcome: "partial",
  code: null,
  ms: 412,
  sessions: [
    { pid: 42, backendStartUs: 1, user: "app", database: "orders", application: "api", client: "10.0.0.5", state: "idle in transaction",
      xactAgeS: 3600, queryId: null, query: "UPDATE shop.orders SET status = 'x'", result: "terminated" },
    { pid: 43, backendStartUs: 2, user: "app", database: "orders", application: "api", client: null, state: null,
      xactAgeS: null, queryId: null, query: null, result: "notAllowed" },
  ],
};
const switchover: ActionRecord = {
  ts: new Date(2026, 9, 25, 12, 1, 0).getTime(),
  action: "switchover",
  connection: { id: "c", name: "billing-prod", environment: "prod" },
  member: "pg-1",
  prodConfirmed: true,
  outcome: "ok",
  code: null,
  ms: 80,
  patroni: { method: "POST", path: "http://pg-1:8008/switchover", body: { leader: "pg-1" }, status: 200, response: "Successfully switched over" },
};
const dropSlot: ActionRecord = {
  ts: new Date(2026, 9, 25, 12, 2, 0).getTime(),
  action: "dropSlot",
  connection: { id: "c", name: "billing-prod", environment: "prod" },
  member: "pg-2",
  prodConfirmed: false,
  outcome: "error",
  code: "NODE_UNAVAILABLE",
  ms: 5,
  slot: { name: "old_slot", slotType: "logical", plugin: "pgoutput", database: "orders", retainedBytes: 1024, walStatus: "reserved" },
};

describe("ActionsJournal", () => {
  beforeEach(() => {
    vi.mocked(api.listActions).mockReset();
    vi.mocked(api.listActions).mockResolvedValue([switchover, terminate]);
  });

  it("таблица: время, действие, узел, объект, итог; новые сверху", async () => {
    renderWithClient(<ActionsJournal connectionId="c" />);
    const table = await screen.findByRole("table", { name: "Журнал действий" });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows[0]).toHaveTextContent("Switchover");
    expect(rows[1]).toHaveTextContent("Завершение сессий");
    expect(rows[1]).toHaveTextContent("2 сессии");
    expect(rows[1]).toHaveTextContent("частично");
    expect(api.listActions).toHaveBeenCalledWith("c", 1000);
  });

  it("столбцы: время, действие, узел, объект, итог; в строке время, узел и итог", async () => {
    renderWithClient(<ActionsJournal connectionId="c" />);
    const table = await screen.findByRole("table", { name: "Журнал действий" });
    expect(within(table).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["Время", "Действие", "Узел", "Объект", "Итог"]);
    const cells = within(within(table).getAllByRole("row")[2]).getAllByRole("cell").map((c) => c.textContent);
    expect(cells).toEqual(["2026-10-25 12:00:05.000", "Завершение сессий", "pg-1", "2 сессии", "частично"]);
  });

  it("строка раскрывается в полную запись", async () => {
    const user = userEvent.setup();
    renderWithClient(<ActionsJournal connectionId="c" />);
    const toggle = await screen.findByRole("button", { name: /Завершение сессий/ });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    // Полная запись — JSON как есть: текст запроса и итоги сессий лежат внутри одного блока.
    expect(screen.getByText(/UPDATE shop\.orders SET status = 'x'/)).toBeInTheDocument();
    expect(screen.getByText(/notAllowed/)).toBeInTheDocument();
    await user.click(toggle);
    expect(screen.queryByText(/notAllowed/)).toBeNull();
  });

  it("в раскрытой записи есть код ошибки; слот — объект строки", async () => {
    const user = userEvent.setup();
    vi.mocked(api.listActions).mockResolvedValue([dropSlot]);
    renderWithClient(<ActionsJournal connectionId="c" />);
    const table = await screen.findByRole("table", { name: "Журнал действий" });
    expect(within(table).getAllByRole("row")[1]).toHaveTextContent("old_slot");
    expect(within(table).getAllByRole("row")[1]).toHaveTextContent("ошибка");
    await user.click(screen.getByRole("button", { name: /Удаление слота/ }));
    expect(screen.getByText(/"code": "NODE_UNAVAILABLE"/)).toBeInTheDocument();
  });

  it("действие без известной подписи показывается как есть; одна сессия — форма «сессия»", async () => {
    vi.mocked(api.listActions).mockResolvedValue([{ ...terminate, action: "futureAction", sessions: [terminate.sessions![0]] }]);
    renderWithClient(<ActionsJournal connectionId="c" />);
    const row = (await screen.findAllByRole("row"))[1];
    expect(row).toHaveTextContent("futureAction");
    expect(row).toHaveTextContent("1 сессия");
  });

  it("раскрытая запись остаётся на своей строке, когда сверху появилась новая", async () => {
    const user = userEvent.setup();
    const { client } = renderWithClient(<ActionsJournal connectionId="c" />);
    await user.click(await screen.findByRole("button", { name: /Завершение сессий/ }));
    vi.mocked(api.listActions).mockResolvedValue([dropSlot, switchover, terminate]);
    await client.invalidateQueries({ queryKey: ["actions", "c"] });
    expect(await screen.findByRole("button", { name: /Удаление слота/ })).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("button", { name: /Завершение сессий/ })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getAllByText(/notAllowed/)).toHaveLength(1);
  });

  it("пустой журнал — сообщение", async () => {
    vi.mocked(api.listActions).mockResolvedValue([]);
    renderWithClient(<ActionsJournal connectionId="c" />);
    expect(await screen.findByText("Действий ещё не было")).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("ошибка чтения — баннер, «Обновить» читает журнал заново", async () => {
    const user = userEvent.setup();
    vi.mocked(api.listActions).mockRejectedValueOnce(new Error("boom"));
    renderWithClient(<ActionsJournal connectionId="c" />);
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Обновить" }));
    expect(await screen.findByRole("table", { name: "Журнал действий" })).toBeInTheDocument();
    expect(api.listActions).toHaveBeenCalledTimes(2);
  });

  it("скрытая вкладка журнал не читает; при возвращении на вкладку читает заново (после действия там появилась запись)", async () => {
    const { rerender } = renderWithClient(<ActionsJournal connectionId="c" active={false} />);
    await Promise.resolve();
    expect(api.listActions).not.toHaveBeenCalled();
    rerender(<ActionsJournal connectionId="c" active />);
    expect(await screen.findByRole("table", { name: "Журнал действий" })).toBeInTheDocument();
    expect(api.listActions).toHaveBeenCalledTimes(1);
    rerender(<ActionsJournal connectionId="c" active={false} />);
    rerender(<ActionsJournal connectionId="c" active />);
    await vi.waitFor(() => expect(api.listActions).toHaveBeenCalledTimes(2));
  });
});
