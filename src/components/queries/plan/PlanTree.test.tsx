import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { planNode, planTree } from "../../../test/plans";
import type { PlanNode, PlanTree } from "../../../api/types";
import { PlanNodeDetails } from "./PlanNodeDetails";
import { PlanSummary } from "./PlanSummary";
import { PlanTreeView } from "./PlanTree";
import { flatten, marks, nodeLabel, renderText } from "./planText";

/** Узел без условий, буферов и факта — для проверки заголовков. */
const bare = (patch: Partial<PlanNode> = {}) =>
  planNode({ actual: null, filter: null, rowsRemovedByFilter: null, buffers: null, relation: null, startupCost: 1, totalCost: 2, planRows: 3, planWidth: 4, ...patch });

/** Заголовки строк текста плана без хвоста «(cost=…)»: так сверяются имена узлов с Rust-рендером. */
const heads = (root: PlanNode) =>
  renderText({ root, planningMs: null, executionMs: null, triggers: [], jit: null, settings: [], totalMs: null })
    .trimEnd()
    .split("\n")
    .map((l) => l.split("  (cost=")[0]);

/** План `ANALYZE, TIMING OFF`: строки и циклы есть, времён и долей нет, горячие узлы — по стоимости. */
function timingOffTree(): PlanTree {
  const noTime = (n: PlanNode): PlanNode => ({
    ...n,
    actual: n.actual && { ...n.actual, startupMs: null, totalMs: null },
    inclusiveMs: null,
    exclusiveMs: null,
    sharePct: null,
    children: n.children.map(noTime),
  });
  return { ...planTree({ planningMs: null, executionMs: null, totalMs: null }), root: noTime(planTree().root) };
}

describe("PlanTreeView", () => {
  it("строки дерева с отступом, горячие и ошибочные оценки помечены, выбор строки", async () => {
    const onSelect = vi.fn();
    const user = userEvent.setup();
    render(<PlanTreeView tree={planTree()} selectedId={null} onSelect={onSelect} />);
    const rows = screen.getAllByRole("row").slice(1);
    expect(rows).toHaveLength(3);
    expect(rows[0]).toHaveTextContent("Hash Join");
    expect(rows[1]).toHaveTextContent("Seq Scan on shop.orders o");
    expect(rows[1]).toHaveAttribute("aria-level", "2");
    expect(rows[1]).toHaveClass("plan-hot");
    expect(within(rows[1]).getByText("×100")).toHaveClass("plan-misestimate");
    expect(rows[1]).toHaveTextContent("500 → 5");
    expect(rows[2]).not.toHaveClass("plan-hot");
    await user.click(within(rows[1]).getByRole("button", { name: /Seq Scan/ }));
    expect(onSelect).toHaveBeenCalledWith(1);
  });

  it("сворачивание скрывает детей; «never executed» и пометки", async () => {
    const user = userEvent.setup();
    const tree = planTree();
    tree.root.children[1] = planNode({ ...tree.root.children[1], neverExecuted: true, sortMethod: "external merge", sortSpaceType: "Disk", sortSpaceKb: 2048 });
    render(<PlanTreeView tree={tree} selectedId={null} onSelect={() => {}} />);
    expect(screen.getAllByRole("row")).toHaveLength(4);
    expect(screen.getByText("не выполнялся")).toBeInTheDocument();
    expect(screen.getByText("диск")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Свернуть" }));
    expect(screen.getAllByRole("row")).toHaveLength(2);
  });

  it("не выполнявшийся узел: в колонке времени прочерк, без «<0,01 мс» и полосы доли", () => {
    const tree = planTree();
    tree.root.children[1] = planNode({ ...tree.root.children[1], neverExecuted: true, actual: { startupMs: 0, totalMs: 0, rows: 0, loops: 0 }, inclusiveMs: 0, exclusiveMs: 0, sharePct: 0 });
    render(<PlanTreeView tree={tree} selectedId={null} onSelect={() => {}} />);
    const time = screen.getAllByRole("row")[3].querySelectorAll("td")[1];
    expect(time).toHaveTextContent(/^—$/);
    expect(within(time).queryByRole("img")).toBeNull();
  });
});

describe("PlanSummary и текст", () => {
  it("сводка: планирование, выполнение, ярус, settings; текст плана в стиле psql", () => {
    render(<PlanSummary tree={planTree()} tier="read" warnings={["genericPlan"]} raw={[{ Plan: {} }]} elapsedMs={70} />);
    expect(screen.getByText(/Планирование: 0,50 мс/)).toBeInTheDocument();
    expect(screen.getByText(/Выполнение: 55,0 мс/)).toBeInTheDocument();
    expect(screen.getByText("Ярус: чтение")).toBeInTheDocument();
    expect(screen.getByText("work_mem = 1MB")).toBeInTheDocument();
    expect(screen.getByText("План общий (GENERIC_PLAN): без значений параметров")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Копировать JSON" })).toBeInTheDocument();
    const text = renderText(planTree());
    expect(text.split("\n")[0]).toBe("Hash Join  (cost=0.00..100.00 rows=1000 width=8) (actual time=1.000..50.000 rows=10 loops=1)");
    expect(text).toContain("  ->  Seq Scan on shop.orders o  (cost=0.00..60.00 rows=500 width=8) (actual time=0.100..30.000 rows=5 loops=1)");
    expect(text).toContain("        Rows Removed by Filter: 499995");
    expect(text).toContain("Execution Time: 55.000 ms");
  });
});

describe("план без времён и без ANALYZE", () => {
  it("TIMING OFF: прочерки вместо времени и долей, без NaN; горячие узлы подсвечены; циклы и строки на месте", () => {
    const { container } = render(<PlanTreeView tree={timingOffTree()} selectedId={null} onSelect={() => {}} />);
    const rows = screen.getAllByRole("row").slice(1);
    expect(screen.getByRole("columnheader", { name: "Время (вкл. / собств.)" })).toBeInTheDocument();
    for (const row of rows) expect(within(row).getAllByRole("cell")[1]).toHaveTextContent(/^—$/);
    expect(rows[1]).toHaveClass("plan-hot");
    expect(rows[1]).toHaveTextContent("500 → 5");
    expect(container.querySelector(".plan-share")).toBeNull();
    expect(container).not.toHaveTextContent("NaN");
    const text = renderText(timingOffTree());
    expect(text.split("\n")[0]).toBe("Hash Join  (cost=0.00..100.00 rows=1000 width=8) (actual rows=10 loops=1)");
    expect(text).not.toContain("NaN");
    expect(text).not.toContain("null");
    expect(text).not.toContain("Planning Time");
    expect(text).not.toContain("Execution Time");
  });

  it("без ANALYZE вместо времени — стоимость узла", () => {
    const tree = planTree();
    const strip = (n: PlanNode): PlanNode => ({ ...n, actual: null, inclusiveMs: null, exclusiveMs: null, sharePct: null, estimateRatio: null, misestimated: false, children: n.children.map(strip) });
    render(<PlanTreeView tree={{ ...tree, root: strip(tree.root), planningMs: null, executionMs: null, totalMs: null }} selectedId={null} onSelect={() => {}} />);
    expect(screen.getByRole("columnheader", { name: "Стоимость" })).toBeInTheDocument();
    const rows = screen.getAllByRole("row").slice(1);
    expect(within(rows[1]).getAllByRole("cell")[1]).toHaveTextContent("0,00..60,00");
    expect(rows[1]).toHaveTextContent("500");
    expect(rows[1]).not.toHaveTextContent("→");
    expect(within(rows[1]).getAllByRole("cell")[3]).toHaveTextContent("—");
  });

  it("ошибка оценки показывается по флагу: бесконечное отношение (null) — «×∞», без флага метки нет", () => {
    const tree = planTree();
    tree.root.children[0] = planNode({ ...tree.root.children[0], planRows: 0, actual: { startupMs: 0, totalMs: 1, rows: 500, loops: 1 }, estimateRatio: null, misestimated: true });
    tree.root.children[1] = planNode({ ...tree.root.children[1], estimateRatio: 3, misestimated: false });
    render(<PlanTreeView tree={tree} selectedId={null} onSelect={() => {}} />);
    const rows = screen.getAllByRole("row").slice(1);
    expect(within(rows[1]).getByText("×∞")).toHaveClass("plan-misestimate");
    expect(rows[2].querySelector(".plan-misestimate")).toBeNull();
  });

  it("степени ошибки оценки: ×10 и больше — оранжевая, ×100 и больше — красная", () => {
    const tree = planTree();
    tree.root.children[0] = planNode({ ...tree.root.children[0], estimateRatio: 0.005, misestimated: true });
    tree.root.children[1] = planNode({ ...tree.root.children[1], estimateRatio: 20, misestimated: true });
    render(<PlanTreeView tree={tree} selectedId={null} onSelect={() => {}} />);
    const rows = screen.getAllByRole("row").slice(1);
    const severe = within(rows[1]).getByText("×200");
    expect(severe).toHaveClass("plan-misestimate", "plan-misestimate-severe");
    const moderate = within(rows[2]).getByText("×20");
    expect(moderate).toHaveClass("plan-misestimate");
    expect(moderate).not.toHaveClass("plan-misestimate-severe");
  });

  it("бесконечная ошибка оценки — красная степень", () => {
    const tree = planTree();
    tree.root.children[0] = planNode({ ...tree.root.children[0], planRows: 0, estimateRatio: null, misestimated: true });
    render(<PlanTreeView tree={tree} selectedId={null} onSelect={() => {}} />);
    expect(within(screen.getAllByRole("row")[2]).getByText("×∞")).toHaveClass("plan-misestimate-severe");
  });

  it("полоса доли времени имеет текстовую подпись с процентом", () => {
    render(<PlanTreeView tree={planTree()} selectedId={null} onSelect={() => {}} />);
    const rows = screen.getAllByRole("row").slice(1);
    expect(within(rows[1]).getByRole("img", { name: "Доля общего времени: 54,5 %" })).toBeInTheDocument();
    expect(within(rows[2]).getByRole("img", { name: "Доля общего времени: 0,7 %" })).toBeInTheDocument();
  });

  it("развёртка сбрасывается при смене плана и сохраняется при перерисовке того же плана", async () => {
    const user = userEvent.setup();
    const tree = planTree();
    const { rerender } = render(<PlanTreeView tree={tree} selectedId={null} onSelect={() => {}} />);
    await user.click(screen.getByRole("button", { name: "Свернуть" }));
    expect(screen.getAllByRole("row")).toHaveLength(2);
    rerender(<PlanTreeView tree={tree} selectedId={1} onSelect={() => {}} />);
    expect(screen.getAllByRole("row")).toHaveLength(2);
    rerender(<PlanTreeView tree={planTree()} selectedId={null} onSelect={() => {}} />);
    expect(screen.getAllByRole("row")).toHaveLength(4);
    expect(screen.getByRole("button", { name: "Свернуть" })).toBeInTheDocument();
  });

  it("выбранная строка помечена aria-selected и классом, разворачивание возвращает детей", async () => {
    const user = userEvent.setup();
    render(<PlanTreeView tree={planTree()} selectedId={2} onSelect={() => {}} />);
    const rows = () => screen.getAllByRole("row").slice(1);
    expect(rows()[2]).toHaveAttribute("aria-selected", "true");
    expect(rows()[2]).toHaveClass("plan-selected");
    expect(rows()[1]).toHaveAttribute("aria-selected", "false");
    expect(rows()[0]).toHaveAttribute("aria-expanded", "true");
    expect(rows()[1]).not.toHaveAttribute("aria-expanded");
    await user.click(screen.getByRole("button", { name: "Свернуть" }));
    expect(rows()).toHaveLength(1);
    expect(rows()[0]).toHaveAttribute("aria-expanded", "false");
    await user.click(screen.getByRole("button", { name: "Развернуть" }));
    expect(rows()).toHaveLength(3);
  });
});

describe("planText: имена узлов как в psql", () => {
  it("соединения, агрегаты, Parallel, Backward, подплан отдельной строкой — как Rust-рендер", () => {
    const joins = bare({
      nodeType: "Hash Join", joinType: "Left",
      children: [
        bare({ nodeType: "Nested Loop", joinType: "Anti", children: [bare({ nodeType: "Nested Loop", joinType: "Inner" })] }),
        bare({ nodeType: "Merge Join", joinType: "Full", children: [bare({ nodeType: "Hash Join", joinType: "Inner" }), bare({ nodeType: "Hash Join", joinType: "Right Anti" })] }),
      ],
    });
    expect(heads(joins)).toEqual([
      "Hash Left Join",
      "  ->  Nested Loop Anti Join",
      "        ->  Nested Loop",
      "  ->  Merge Full Join",
      "        ->  Hash Join",
      "        ->  Hash Right Anti Join",
    ]);
    const aggs = bare({
      nodeType: "Aggregate", strategy: "Plain", extra: { "Partial Mode": "Finalize" },
      children: [
        bare({
          nodeType: "Gather", workersPlanned: 2,
          children: [bare({ nodeType: "Aggregate", strategy: "Hashed", extra: { "Partial Mode": "Partial" }, children: [bare({ nodeType: "Seq Scan", parallelAware: true, relation: { schema: "shop", name: "orders", alias: "o" } })] })],
        }),
        bare({ nodeType: "Aggregate", strategy: "Sorted", extra: { "Partial Mode": "Simple" }, children: [bare({ nodeType: "Aggregate", strategy: "Mixed", children: [bare({ nodeType: "SetOp", strategy: "Hashed", children: [bare({ nodeType: "SetOp", strategy: "Sorted" })] })] })] }),
      ],
    });
    expect(heads(aggs)).toEqual([
      "Finalize Aggregate",
      "  ->  Gather",
      "        Workers Planned: 2",
      "        ->  Partial HashAggregate",
      "              ->  Parallel Seq Scan on shop.orders o",
      "  ->  GroupAggregate",
      "        ->  MixedAggregate",
      "              ->  HashSetOp",
      "                    ->  SetOp",
    ]);
    const scans = bare({
      nodeType: "Index Scan", indexName: "orders_pkey", extra: { "Scan Direction": "Backward" }, relation: { schema: null, name: "orders", alias: "o" },
      children: [
        bare({ nodeType: "Bitmap Index Scan", indexName: "orders_idx" }),
        bare({ nodeType: "Aggregate", strategy: "Plain", parentRelationship: "SubPlan", subplanName: "SubPlan 1", children: [bare({ nodeType: "Index Only Scan", indexName: "i2", relation: { schema: null, name: "t", alias: null } })] }),
      ],
    });
    expect(heads(scans)).toEqual([
      "Index Scan Backward using orders_pkey on orders o",
      "  ->  Bitmap Index Scan on orders_idx",
      "  SubPlan 1",
      "    ->  Aggregate",
      "          ->  Index Only Scan using i2 on t",
    ]);
  });

  it("ModifyTable берёт операцию из extra, CTE Scan и Subquery Scan — имя объекта; nodeLabel без стоимости", () => {
    expect(nodeLabel(bare({ nodeType: "ModifyTable", extra: { Operation: "Update" }, relation: { schema: "shop", name: "balances", alias: "b" } }))).toBe("Update on shop.balances b");
    expect(nodeLabel(bare({ nodeType: "CTE Scan", extra: { "CTE Name": "d", Alias: "x" } }))).toBe("CTE Scan on d x");
    expect(nodeLabel(bare({ nodeType: "Subquery Scan", extra: { Alias: "sq" } }))).toBe("Subquery Scan on sq");
    expect(nodeLabel(bare({ nodeType: "Result" }))).toBe("Result");
  });

  it("условия, буферы и итоги — в порядке Rust-рендера; невыполнявшийся узел и триггеры, JIT", () => {
    const root = planNode({
      nodeType: "Gather", relation: null, workersPlanned: 2, workersLaunched: 1, filter: null, rowsRemovedByFilter: null, sortKey: ["o.id"], groupKey: ["o.status"],
      sortMethod: "external merge", sortSpaceType: "Disk", sortSpaceKb: 2048, hashBatches: 4, hashBuckets: 1024, peakMemoryKb: 64, exactHeapBlocks: 3, lossyHeapBlocks: 1, heapFetches: 7,
      buffers: { sharedHit: 5, sharedRead: 0, sharedDirtied: 2, sharedWritten: 0, localHit: 0, localRead: 0, localDirtied: 0, localWritten: 0, tempRead: 1, tempWritten: 2 },
      ioReadMs: 1.5, ioWriteMs: null, indexCond: "(a = 1)", recheckCond: "(a = 1)", joinFilter: "(b > 2)", mergeCond: "(a = b)",
      children: [planNode({ id: 1, actual: { startupMs: 0, totalMs: 0, rows: 0, loops: 0 }, neverExecuted: true })],
    });
    const tree = planTree({
      root, planningMs: 1, executionMs: 2,
      triggers: [{ name: "audit", relation: "orders", timeMs: 0.25, calls: 3 }],
      jit: { functions: 4, generationMs: 1, inliningMs: 2, optimizationMs: 3, emissionMs: 4, totalMs: 10 },
    });
    const lines = renderText(tree).trimEnd().split("\n");
    expect(lines.slice(1, 15)).toEqual([
      "  Index Cond: (a = 1)",
      "  Recheck Cond: (a = 1)",
      "  Merge Cond: (a = b)",
      "  Join Filter: (b > 2)",
      "  Sort Key: o.id",
      "  Group Key: o.status",
      "  Sort Method: external merge  Disk: 2048kB",
      "  Buckets: 1024  Batches: 4  Memory Usage: 64kB",
      "  Heap Blocks: exact=3 lossy=1",
      "  Heap Fetches: 7",
      "  Workers Planned: 2",
      "  Workers Launched: 1",
      "  Buffers: shared hit=5 dirtied=2, temp read=1 written=2",
      "  I/O Timings: read=1.500 write=0.000",
    ]);
    expect(lines[15]).toBe("  ->  Seq Scan on shop.orders o  (cost=0.00..60.00 rows=500 width=8) (never executed)");
    expect(lines).toContain("Planning Time: 1.000 ms");
    expect(lines.slice(-5)).toEqual([
      "Trigger audit: time=0.250 calls=3",
      "JIT:",
      "  Functions: 4",
      "  Timing: Generation 1.000 ms, Inlining 2.000 ms, Optimization 3.000 ms, Emission 4.000 ms, Total 10.000 ms",
      "Execution Time: 2.000 ms",
    ]);
    expect(renderText(tree).endsWith("Execution Time: 2.000 ms\n")).toBe(true);
  });

  it("flatten пропускает детей свёрнутых узлов, marks собирает ключи пометок", () => {
    const tree = planTree();
    expect(flatten(tree.root, new Set()).map((r) => [r.node.id, r.depth])).toEqual([[0, 0], [1, 1], [2, 1]]);
    expect(flatten(tree.root, new Set([0])).map((r) => r.node.id)).toEqual([0]);
    expect(marks(planNode({ neverExecuted: true, hashBatches: 2, lossyHeapBlocks: 1, heapFetches: 1, workersPlanned: 3, workersLaunched: 2, parallelAware: true }))).toEqual([
      "neverExecuted", "disk", "lossy", "heapFetches", "workersShort", "parallel",
    ]);
    expect(marks(planNode())).toEqual([]);
  });
});

describe("PlanNodeDetails", () => {
  it("поля узла, непустые значения; нераспознанные поля в раскрывающемся блоке", () => {
    render(<PlanNodeDetails node={planNode({ extra: { "Mystery Field": 1 }, output: ["o.id", "o.status"] })} />);
    expect(screen.getByRole("heading", { name: "Узел" })).toBeInTheDocument();
    expect(screen.getByText("Seq Scan on shop.orders o")).toBeInTheDocument();
    expect(screen.getByText("0,10 мс..30,0 мс, 5 строк, циклы 1")).toBeInTheDocument();
    expect(screen.getByText("(o.status = 'disputed'::text)")).toBeInTheDocument();
    expect(screen.getByText("499 995")).toBeInTheDocument();
    expect(screen.getByText("o.id, o.status")).toBeInTheDocument();
    expect(screen.queryByText("Index Cond")).not.toBeInTheDocument();
    expect(screen.getByText("Прочие поля (1)")).toBeInTheDocument();
    expect(screen.getByText(/"Mystery Field": 1/)).toBeInTheDocument();
  });

  it("все распознанные поля: группировка, фильтр соединения, heap-блоки, подпланы, корзины, локальные буферы, включённое время и доля", () => {
    render(
      <PlanNodeDetails
        node={planNode({
          subplanName: "SubPlan 1", groupKey: ["o.status", "o.region"], rowsRemovedByJoinFilter: 1200, exactHeapBlocks: 3, lossyHeapBlocks: 1, subplansRemoved: 2,
          hashBatches: 4, hashBuckets: 1024, peakMemoryKb: 64, startupCost: 1.5, totalCost: 1234.5,
          buffers: { sharedHit: 80, sharedRead: 20, sharedDirtied: 0, sharedWritten: 0, localHit: 7, localRead: 3, localDirtied: 1, localWritten: 0, tempRead: 0, tempWritten: 0 },
          inclusiveMs: 42, exclusiveMs: 30, sharePct: 54.5,
        })}
      />,
    );
    const value = (label: string) => screen.getByText(label).nextElementSibling;
    expect(value("Имя вложенного плана")).toHaveTextContent("SubPlan 1");
    expect(value("Ключ группировки")).toHaveTextContent("o.status, o.region");
    expect(value("Отсеяно фильтром соединения")).toHaveTextContent("1 200");
    expect(value("Блоки кучи (exact / lossy)")).toHaveTextContent("3 / 1");
    expect(value("Исключено подпланов")).toHaveTextContent("2");
    expect(value("Хеш")).toHaveTextContent("батчей 4, корзин 1 024, 64 kB");
    expect(value("Локальные буферы")).toHaveTextContent("local hit 7, read 3, dirtied 1, written 0");
    expect(value("Включённое время")).toHaveTextContent("42,0 мс");
    expect(value("Собственное время")).toHaveTextContent("30,0 мс");
    expect(value("Доля общего времени")).toHaveTextContent("54,5 %");
    expect(value("Стоимость")).toHaveTextContent("1,50..1 234,50");
  });

  it("поля, которых у узла нет, не показываются", () => {
    render(<PlanNodeDetails node={planNode({ inclusiveMs: null, exclusiveMs: null, sharePct: null })} />);
    for (const label of ["Имя вложенного плана", "Ключ группировки", "Отсеяно фильтром соединения", "Блоки кучи (exact / lossy)", "Исключено подпланов", "Хеш", "Локальные буферы", "Включённое время", "Доля общего времени"]) {
      expect(screen.queryByText(label)).not.toBeInTheDocument();
    }
  });

  it("не выполнявшийся узел: «не выполнялся» вместо нулевого факта и времён", () => {
    render(<PlanNodeDetails node={planNode({ neverExecuted: true, actual: { startupMs: 0, totalMs: 0, rows: 0, loops: 0 }, inclusiveMs: 0, exclusiveMs: 0, sharePct: 0 })} />);
    expect(screen.getByText("Факт").nextElementSibling).toHaveTextContent(/^не выполнялся$/);
    expect(screen.queryByText("Включённое время")).not.toBeInTheDocument();
    expect(screen.queryByText("Собственное время")).not.toBeInTheDocument();
    expect(screen.queryByText("Доля общего времени")).not.toBeInTheDocument();
    expect(screen.queryByText(/циклы 0/)).not.toBeInTheDocument();
  });

  it("TIMING OFF: время не печатается, NaN нет", () => {
    const { container } = render(<PlanNodeDetails node={planNode({ actual: { startupMs: null, totalMs: null, rows: 5, loops: 2 }, inclusiveMs: null, exclusiveMs: null, sharePct: null })} />);
    expect(screen.getByText("5 строк, циклы 2")).toBeInTheDocument();
    expect(screen.queryByText("Собственное время")).not.toBeInTheDocument();
    expect(container).not.toHaveTextContent("NaN");
  });
});

describe("PlanSummary: прочее", () => {
  it("JIT, триггеры, неизвестное предупреждение как есть; без raw нет кнопки JSON", () => {
    render(
      <PlanSummary
        tree={planTree({ jit: { functions: 3, generationMs: 1, inliningMs: 0, optimizationMs: 0, emissionMs: 1, totalMs: 2 }, triggers: [{ name: "audit", relation: null, timeMs: 1.5, calls: 4 }] })}
        warnings={["probeWithoutReadOnly", "somethingNew"]}
      />,
    );
    expect(screen.getByText("JIT: функций 3, 2,00 мс")).toBeInTheDocument();
    expect(screen.getByText("audit: 1,50 мс × 4")).toBeInTheDocument();
    expect(screen.getByText("Пробный план построен вне READ ONLY (сервер отверг его там)")).toBeInTheDocument();
    expect(screen.getByText("somethingNew")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Копировать JSON" })).not.toBeInTheDocument();
    expect(screen.queryByText("JSON")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Копировать как текст" })).toBeInTheDocument();
    expect(screen.getByText("Как текст")).toBeInTheDocument();
    expect(screen.queryByText(/Ярус/)).not.toBeInTheDocument();
  });

  it("план без времени планирования и выполнения: частей сводки нет", () => {
    render(<PlanSummary tree={planTree({ planningMs: null, executionMs: null, settings: [] })} tier="write" />);
    expect(screen.queryByText(/Планирование/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Выполнение/)).not.toBeInTheDocument();
    expect(screen.getByText("Ярус: запись")).toBeInTheDocument();
  });
});
