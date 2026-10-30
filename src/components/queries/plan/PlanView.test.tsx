import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { planNode, planTree } from "../../../test/plans";
import { PlanView } from "./PlanView";

describe("PlanView", () => {
  it("клик по узлу открывает панель узла; смена плана сбрасывает выбор, а возврат к прежнему плану его не воскрешает", async () => {
    const user = userEvent.setup();
    const first = planTree();
    const second = planTree({ root: planNode({ id: 0, nodeType: "Result", relation: null, children: [] }) });
    const { rerender } = render(<PlanView tree={first} />);
    await user.click(within(screen.getByRole("treegrid", { name: "Дерево плана" })).getByRole("button", { name: /Seq Scan/ }));
    expect(screen.getByText("Собственное время")).toBeInTheDocument();
    rerender(<PlanView tree={second} />);
    expect(screen.queryByText("Собственное время")).toBeNull();
    rerender(<PlanView tree={first} />);
    expect(screen.queryByText("Собственное время")).toBeNull();
  });

  it("сводка получает ярус, предупреждения и время запуска", () => {
    render(<PlanView tree={planTree()} tier="write" warnings={["genericPlan"]} elapsedMs={7} />);
    expect(screen.getByText("Ярус: запись")).toBeInTheDocument();
    expect(screen.getByText(/Запрос: 7,00 мс/)).toBeInTheDocument();
    expect(screen.getByText("План общий (GENERIC_PLAN): без значений параметров")).toBeInTheDocument();
  });
});
