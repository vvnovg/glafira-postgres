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
    expect(onShowNode).toHaveBeenCalledWith([1]);
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

  const props = { connectionId: "c", member: "pg-1", database: "perf", sql: "SELECT 1", params: { kind: "none" } as const, searchPath: null, onShowNode: () => {} };

  it("шаблон (<columns>), без hypo: пометка о шаблоне, нет «Проверить гипотезу»; без шаблона пометки нет", () => {
    const template: Recommendation = { ...rec, params: { table: "shop.orders", rowsRead: 500_000, rowsReturned: 500, loops: 1 }, command: "CREATE INDEX CONCURRENTLY ON \"shop\".\"orders\" (<columns>);", hypo: null };
    const { unmount } = renderWithClient(<Recommendations {...props} result={result({ recommendations: [template] })} />);
    expect(screen.getByText(/замените <…> своими значениями/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Проверить гипотезу" })).toBeNull();
    unmount();
    // Сравнение в команде (`<`) — не шаблон.
    renderWithClient(<Recommendations {...props} result={result({ recommendations: [{ ...rec, command: "SELECT 1 WHERE 1 < 2;" }] })} />);
    expect(screen.queryByText(/замените <…>/)).toBeNull();
    expect(screen.getByText("Приложение эту команду не выполняет: скопируйте и выполните сами")).toBeInTheDocument();
  });

  it("группа партиций: заголовок и доказательство по партициям, «Показать узел» отдаёт все узлы", async () => {
    const onShowNode = vi.fn();
    const user = userEvent.setup();
    const grouped: Recommendation = {
      id: "plan_seq_scan_filter", severity: "warning", nodeIds: [3, 4, 5],
      params: { partitions: 3, table: "shop.events_p1", tables: "shop.events_p1, shop.events_p2, shop.events_p3", rowsRead: 30_000, rowsReturned: 3, loops: 1 },
      command: "CREATE INDEX CONCURRENTLY ON \"shop\".\"events_p1\" (<columns>);\nCREATE INDEX CONCURRENTLY ON \"shop\".\"events_p2\" (<columns>);", hypo: null,
    };
    renderWithClient(<Recommendations {...props} onShowNode={onShowNode} result={result({ recommendations: [grouped] })} />);
    expect(screen.getByRole("heading", { name: "Индекс по колонкам фильтра в партициях (3): shop.events_p1 и другие" })).toBeInTheDocument();
    expect(screen.getByText(/shop\.events_p3: прочитано строк 30\s000, возвращено 3/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Показать узел" }));
    expect(onShowNode).toHaveBeenCalledWith([3, 4, 5]);
  });

  it("порядок по важности: критичные, предупреждения, сведения", () => {
    const info: Recommendation = { id: "plan_workers_short", severity: "info", nodeIds: [0], params: { planned: 4, launched: 2 }, command: null, hypo: null };
    const critical: Recommendation = { ...rec, id: "plan_subplan_per_row", severity: "critical", params: { loops: 3000, name: "SubPlan 1" }, command: null, hypo: null };
    renderWithClient(<Recommendations {...props} result={result({ recommendations: [info, rec, critical] })} />);
    expect(screen.getAllByRole("heading", { level: 4 }).map((h) => h.textContent)).toEqual([
      "Подзапрос выполняется на каждую строку",
      "Индекс по колонкам фильтра: shop.orders",
      "Воркеров меньше, чем запланировано",
    ]);
  });

  it("правило новее приложения: карточка с id и командой, без сырых ключей", () => {
    const unknown: Recommendation = { id: "plan_future_rule", severity: "warning", nodeIds: [1], params: { table: "shop.orders" }, command: "VACUUM shop.orders;", hypo: null };
    const { container } = renderWithClient(<Recommendations {...props} result={result({ recommendations: [unknown] })} />);
    expect(screen.getByRole("heading", { name: "Рекомендация plan_future_rule" })).toBeInTheDocument();
    expect(screen.getByText(/нет описания этого правила/)).toBeInTheDocument();
    expect(screen.getByText("VACUUM shop.orders;")).toBeInTheDocument();
    expect(container.textContent).not.toMatch(/recommend\./);
    expect(screen.queryByText(/Эффект/)).toBeNull();
  });

  it("HypoCheck: ошибка hypo_check — баннером с текстом errors.hypo.*", async () => {
    vi.mocked(api.hypoCheck).mockRejectedValue({ code: "FEATURE_UNAVAILABLE", key: "errors.hypo.notInstalled", message: "hypopg is not installed in this database", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<Recommendations {...props} result={result()} />);
    await user.click(screen.getByRole("button", { name: "Проверить гипотезу" }));
    const dialog = await screen.findByRole("dialog", { name: "Проверка гипотезы" });
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("hypopg не установлен в этой базе");
    expect(within(dialog).queryByRole("treegrid")).toBeNull();
  });
});
