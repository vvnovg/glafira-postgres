import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ExplainResult, Recommendation } from "../../../api/types";
import { planTree } from "../../../test/plans";
import { renderWithClient } from "../../../test/render";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return { ...orig, api: { hypoCheck: vi.fn() } };
});

import { api } from "../../../api/client";
import { Recommendations } from "./Recommendations";

const rec: Recommendation = {
  id: "plan_seq_scan_filter", severity: "warning", nodeIds: [1],
  params: { table: "shop.orders", rowsRead: 500_000, rowsReturned: 500, loops: 1, columns: "status" },
  command: "CREATE INDEX CONCURRENTLY ON \"shop\".\"orders\" (\"status\");", hypo: { indexDdl: "CREATE INDEX ON \"shop\".\"orders\" (\"status\")" },
};
const result = (patch: Partial<ExplainResult> = {}): ExplainResult => ({ mode: "analyze", tier: "read", plan: planTree(), raw: [], relations: [], elapsedMs: 1, warnings: [], recommendations: [rec], recommendationsSkipped: [], ...patch });

describe("Recommendations", () => {
  beforeEach(() => vi.resetAllMocks());

  it("карточка рекомендации: заголовок с цифрами, доказательство, команда, эффект и риски, «Показать узел»", async () => {
    const onShowNode = vi.fn();
    const user = userEvent.setup();
    renderWithClient(<Recommendations connectionId="c" member="pg-1" database="perf" sql="SELECT 1" params={{ kind: "none" }} searchPath={null} result={result()} onShowNode={onShowNode} />);
    expect(screen.getByRole("heading", { name: "Индекс по колонкам фильтра: shop.orders" })).toBeInTheDocument();
    expect(screen.getByText(/Seq Scan прочитал 500 000 строк, вернул 500/)).toBeInTheDocument();
    expect(screen.getByText(rec.command!)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Копировать команду" })).toBeInTheDocument();
    expect(screen.getByText(/Риски/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Показать узел" }));
    expect(onShowNode).toHaveBeenCalledWith(1);
  });

  it("«Проверить гипотезу»: запрос с DDL, стоимость была/стала и использование индекса", async () => {
    vi.mocked(api.hypoCheck).mockResolvedValue({ indexName: "<1>btree_orders_status", estimatedBytes: 11_000_000, costBefore: 12_000, costAfter: 120.5, used: true, planAfter: planTree() });
    const user = userEvent.setup();
    renderWithClient(<Recommendations connectionId="c" member="pg-1" database="perf" sql="SELECT 1" params={{ kind: "none" }} searchPath={null} result={result()} onShowNode={() => {}} />);
    await user.click(screen.getByRole("button", { name: "Проверить гипотезу" }));
    await waitFor(() => expect(api.hypoCheck).toHaveBeenCalledWith("c", { member: "pg-1", database: "perf", sql: "SELECT 1", params: { kind: "none" }, searchPath: null, indexDdl: rec.hypo!.indexDdl }));
    const dialog = await screen.findByRole("dialog", { name: "Проверка гипотезы" });
    expect(within(dialog).getByText(/Стоимость: 12 000 → 120,5/)).toBeInTheDocument();
    expect(within(dialog).getByText("Индекс использован")).toBeInTheDocument();
    expect(within(dialog).getByText(/10,5 MB/)).toBeInTheDocument();
    expect(within(dialog).getByRole("treegrid")).toBeInTheDocument();
  });

  it("без ANALYZE — подсказка о пропущенных правилах; без рекомендаций — «Советов нет»", () => {
    renderWithClient(<Recommendations connectionId="c" member="pg-1" database="perf" sql="SELECT 1" params={{ kind: "none" }} searchPath={null} result={result({ recommendations: [], recommendationsSkipped: ["plan_spill", "plan_jit_heavy"] })} onShowNode={() => {}} />);
    expect(screen.getByText("Советов нет")).toBeInTheDocument();
    expect(screen.getByText("Выполните план с выполнением, чтобы проверить ещё 2 правила")).toBeInTheDocument();
  });
});
