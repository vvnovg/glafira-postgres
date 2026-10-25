import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { useWorkspace } from "../../state/workspace";
import { diagReport, holder } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";
import { HorizonView } from "./HorizonView";

const horizon = (holders: ReturnType<typeof holder>[], hotStandbyFeedback: boolean | null = false) =>
  diagReport({ kind: "horizon", holders, hotStandbyFeedback });

describe("HorizonView", () => {
  beforeEach(() => useWorkspace.setState({ tabs: [], activeId: null }));

  it("держатели по возрасту, вид держателя, переход к реплике feedback", async () => {
    const user = userEvent.setup();
    useWorkspace.setState({ tabs: [], activeId: null });
    const holders = [
      holder({ kind: "standbyFeedback", name: "walreceiver", member: "pg-2", xminAge: 500 }),
      holder({ kind: "preparedXact", name: "it", gid: "it", xminAge: 100, durationS: 4000 }),
      holder({ kind: "slot", name: "dead", catalog: true, xminAge: 10 }),
    ];
    renderWithClient(<HorizonView connectionId="c" report={diagReport({ kind: "horizon", holders, hotStandbyFeedback: false })} />);
    const rows = screen.getAllByRole("row").slice(1);
    expect(rows.map((r) => (r as HTMLTableRowElement).cells[0].textContent)).toEqual(["feedback реплики", "prepared transaction", "слот (catalog_xmin)"]);
    await user.click(within(rows[0]).getByRole("button", { name: "Открыть горизонт на pg-2" }));
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ tool: "horizon", member: "pg-2" });
    expect(within(rows[1]).getByText("ROLLBACK PREPARED 'it';")).toBeInTheDocument();
  });

  it("ссылка ведёт на вкладку этого же подключения; без участника или на самом себе кнопки нет", async () => {
    const user = userEvent.setup();
    renderWithClient(
      <HorizonView
        connectionId="c1"
        report={horizon([
          holder({ kind: "standbyFeedback", name: "pg-2", member: "pg-2", xminAge: 5 }),
          holder({ kind: "standbyFeedback", name: "unknown", member: null, xminAge: 4 }),
          holder({ kind: "standbyFeedback", name: "pg-1", member: "pg-1", xminAge: 3 }),
        ])}
      />,
    );
    expect(screen.getAllByRole("button", { name: /Открыть горизонт/ })).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "Открыть горизонт на pg-2" }));
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ kind: "diag", connectionId: "c1", tool: "horizon", member: "pg-2" });
  });

  it("слот: имя, тип, активность, приложение и клиент реплики; активный физический слот реплики — со ссылкой", () => {
    renderWithClient(
      <HorizonView
        connectionId="c"
        report={horizon([
          holder({ kind: "slot", name: "pg_2", slotType: "physical", active: true, member: "pg-2", application: "pg-2", clientAddr: "10.0.0.2", xminAge: 900 }),
          holder({ kind: "slot", name: "forgotten", slotType: "logical", active: false, xminAge: 800 }),
        ])}
      />,
    );
    const rows = screen.getAllByRole("row").slice(1);
    expect(rows[0]).toHaveTextContent("pg_2");
    expect(rows[0]).toHaveTextContent("physical");
    expect(rows[0]).toHaveTextContent("активен");
    expect(rows[0]).toHaveTextContent("pg-2");
    expect(rows[0]).toHaveTextContent("10.0.0.2");
    expect(within(rows[0]).getByRole("button", { name: "Открыть горизонт на pg-2" })).toBeInTheDocument();
    expect(rows[1]).toHaveTextContent("forgotten");
    expect(rows[1]).toHaveTextContent("logical");
    expect(rows[1]).toHaveTextContent("неактивен");
    expect(within(rows[1]).queryByRole("button")).not.toBeInTheDocument();
  });

  it("сессия: pid, роль, приложение, БД и состояние; возраст xmin и длительность", () => {
    renderWithClient(
      <HorizonView
        connectionId="c"
        report={horizon([holder({ kind: "session", name: "4242", pid: 4242, user: "app", application: "api", database: "orders", state: "idle in transaction", xminAge: 12_345_678, durationS: 4000 })])}
      />,
    );
    const row = screen.getAllByRole("row")[1];
    expect(row).toHaveTextContent("сессия");
    expect(row).toHaveTextContent("4242");
    expect(row).toHaveTextContent("app");
    expect(row).toHaveTextContent("api");
    expect(row).toHaveTextContent("orders");
    expect(row).toHaveTextContent("idle in transaction");
    expect(row).toHaveTextContent(/12\s345\s678/);
    expect(row).toHaveTextContent("1 ч 6 мин");
  });

  it("prepared transaction: gid, владелец, БД; кавычки в gid удваиваются; команду можно скопировать", () => {
    renderWithClient(<HorizonView connectionId="c" report={horizon([holder({ kind: "preparedXact", name: "it's", gid: "it's", user: "owner", database: "orders", xminAge: 7 })])} />);
    const row = screen.getAllByRole("row")[1];
    expect(row).toHaveTextContent("owner");
    expect(row).toHaveTextContent("orders");
    expect(within(row).getByText("ROLLBACK PREPARED 'it''s';")).toBeInTheDocument();
    expect(within(row).getByRole("button", { name: "Скопировать команду" })).toBeInTheDocument();
  });

  it("hot_standby_feedback узла показан над таблицей; неизвестное значение — без строки", () => {
    const { rerender } = renderWithClient(<HorizonView connectionId="c" report={horizon([holder()], true)} />);
    expect(screen.getByText("hot_standby_feedback = on")).toBeInTheDocument();
    rerender(<HorizonView connectionId="c" report={horizon([holder()], false)} />);
    expect(screen.getByText("hot_standby_feedback = off")).toBeInTheDocument();
    rerender(<HorizonView connectionId="c" report={horizon([holder()], null)} />);
    expect(screen.queryByText(/hot_standby_feedback/)).not.toBeInTheDocument();
  });

  it("держателей нет — сообщение вместо таблицы", () => {
    renderWithClient(<HorizonView connectionId="c" report={horizon([])} />);
    expect(screen.getByText("Горизонт xmin ничто не удерживает")).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("чужой вид отчёта не рисуется", () => {
    const { container } = renderWithClient(<HorizonView connectionId="c" report={diagReport({ kind: "health", endpoints: [], members: [] })} />);
    expect(container).toBeEmptyDOMElement();
  });
});
