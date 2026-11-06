import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { ProposalComparison, Recommendation } from "../../../api/types";
import { planNode, planTree } from "../../../test/plans";
import { ProposalCompare } from "./ProposalCompare";

const rec: Recommendation = {
  id: "plan_seq_scan_filter",
  severity: "warning",
  nodeIds: [1],
  params: { table: "shop.orders", rowsRead: 500_000, rowsReturned: 500, loops: 1, columns: "status" },
  command: null,
  hypo: null,
};
const DDL = "CREATE INDEX ON shop.orders (status)";

function comparison(patch: Partial<ProposalComparison> = {}): ProposalComparison {
  const after = planTree({ root: planNode({ id: 0, nodeType: "Index Scan", indexName: "<13456>btree_orders_status", totalCost: 8.5, hot: true, children: [] }) });
  return {
    costBefore: 120,
    costAfter: 8.5,
    indexesUsed: [{ ddl: DDL, used: true }],
    member: "pg-1",
    atMs: 1_700_000_000_000,
    planBefore: planTree(),
    planAfter: after,
    recommendationsBefore: [rec],
    recommendationsAfter: [],
    withoutHypopg: false,
    ...patch,
  };
}

describe("ProposalCompare", () => {
  it("стоимость была / стала, узел, индекс использован; горячие узлы и рекомендации обоих планов рядом", () => {
    render(<ProposalCompare comparison={comparison()} />);
    const block = screen.getByRole("region", { name: "Сравнение планов" });
    expect(within(block).getByText(/^Стоимость: была 120,00, стала 8,50/)).toBeInTheDocument();
    expect(within(block).getByText(/^узел pg-1, /)).toBeInTheDocument();
    expect(within(block).getByText(DDL).parentElement).toHaveTextContent("Индекс использован");
    const before = within(block).getByRole("region", { name: "Исходный план" });
    expect(within(before).getByText("Hash Join")).toBeInTheDocument();
    expect(within(before).getByText("Seq Scan on shop.orders o")).toBeInTheDocument();
    // Не горячий узел в список не попадает.
    expect(within(before).queryByText("Index Scan using customers_pkey on shop.customers c")).toBeNull();
    expect(within(before).getByText("Индекс по колонкам фильтра: shop.orders")).toBeInTheDocument();
    const after = within(block).getByRole("region", { name: "Предложенный план" });
    expect(within(after).getByText("Index Scan using <13456>btree_orders_status on shop.orders o")).toBeInTheDocument();
    expect(within(after).getByText("Рекомендаций нет")).toBeInTheDocument();
    expect(screen.queryByText(/нет hypopg/)).toBeNull();
  });

  it("Z3: стоимости — как в таблице плана (два знака), перед «узел …» — разделитель «·»", () => {
    render(<ProposalCompare comparison={comparison({ costBefore: 8908.9, costAfter: 1905.98 })} />);
    const block = screen.getByRole("region", { name: "Сравнение планов" });
    const line = within(block).getByText(/^Стоимость: /).closest("p")!;
    expect(line.textContent).toMatch(/^Стоимость: была 8\u00a0908,90, стала 1\u00a0905,98 · узел pg-1, \d\d:\d\d:\d\d$/);
    // Стоимость горячего узла — тем же форматом.
    expect(within(block).getByRole("region", { name: "Предложенный план" })).toHaveTextContent("стоимость 8,50");
  });

  it("без hypopg — пометка, индекс не использован", () => {
    render(<ProposalCompare comparison={comparison({ withoutHypopg: true, indexesUsed: [{ ddl: DDL, used: false }] })} />);
    expect(screen.getByText("В этой базе нет hypopg: предложенный запрос спланирован без своих индексов")).toBeInTheDocument();
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.getByText("Индекс не использован")).toBeInTheDocument();
  });

  it("неизвестная рекомендация — запасной заголовок, не сырой ключ; повтор индекса не ломает список", () => {
    const unknown: Recommendation = { ...rec, id: "future_rule_x" };
    render(<ProposalCompare comparison={comparison({ recommendationsAfter: [unknown], indexesUsed: [{ ddl: DDL, used: true }, { ddl: DDL, used: false }] })} />);
    const after = screen.getByRole("region", { name: "Предложенный план" });
    expect(within(after).getByText("Рекомендация future_rule_x")).toBeInTheDocument();
    expect(screen.queryByText(/recommend\./)).toBeNull();
    expect(screen.getAllByText(DDL)).toHaveLength(2);
  });
});
