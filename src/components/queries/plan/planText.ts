import type { PlanNode, PlanTree } from "../../../api/types";

/** Строковое поле из `extra` (ключи, которые модель плана не разбирает: `Partial Mode`, `Operation`, `CTE Name` …). */
function extraStr(n: PlanNode, key: string): string | null {
  const v = n.extra[key];
  return typeof v === "string" ? v : null;
}

/** Имя соединения как в `ExplainNode`: тип вставляется перед «Join» (`Hash Left Join`), у внутреннего Nested Loop слова «Join» нет. */
function joinName(n: PlanNode): string {
  const base = n.nodeType === "Hash Join" ? "Hash" : n.nodeType === "Merge Join" ? "Merge" : "Nested Loop";
  if (n.joinType !== null && n.joinType !== "Inner") return `${base} ${n.joinType} Join`;
  return base !== "Nested Loop" ? `${base} Join` : base;
}

/** Имя узла, как его печатает psql: стратегия агрегата и SetOp, `Partial` / `Finalize`, `Async` / `Parallel`, тип соединения, операция ModifyTable. */
function nodeName(n: PlanNode): string {
  let name: string;
  switch (n.nodeType) {
    case "Aggregate":
      name = n.strategy === "Sorted" ? "GroupAggregate" : n.strategy === "Hashed" ? "HashAggregate" : n.strategy === "Mixed" ? "MixedAggregate" : "Aggregate";
      break;
    case "SetOp":
      name = n.strategy === "Hashed" ? "HashSetOp" : "SetOp";
      break;
    case "ModifyTable":
      name = extraStr(n, "Operation") ?? "ModifyTable";
      break;
    case "Hash Join":
    case "Merge Join":
    case "Nested Loop":
      name = joinName(n);
      break;
    default:
      name = n.nodeType;
  }
  const mode = extraStr(n, "Partial Mode");
  if (n.nodeType === "Aggregate" && (mode === "Partial" || mode === "Finalize")) name = `${mode} ${name}`;
  if (n.extra["Async Capable"] === true) name = `Async ${name}`;
  if (n.parallelAware) name = `Parallel ${name}`;
  return name;
}

/** Типы узлов, у которых имя берётся из `Alias`, если отношения нет. */
const ALIAS_TARGETS: ReadonlySet<string> = new Set(["Subquery Scan", "Values Scan", "Table Function Scan", "Named Tuplestore Scan"]);

/**
 * Цель узла: индекс (` using idx`, ` Backward using idx`, у Bitmap Index Scan — ` on idx`) и отношение (` on schema.rel alias`);
 * у CTE, подзапроса и функции — их имя из `extra`.
 */
function target(n: PlanNode): string {
  let s = "";
  if (n.indexName !== null) {
    if (n.nodeType === "Bitmap Index Scan") s += ` on ${n.indexName}`;
    else {
      if (extraStr(n, "Scan Direction") === "Backward") s += " Backward";
      s += ` using ${n.indexName}`;
    }
  }
  if (n.relation) {
    const { schema, name, alias } = n.relation;
    s += ` on ${schema !== null ? `${schema}.` : ""}${name}`;
    if (alias !== null && alias !== name) s += ` ${alias}`;
    return s;
  }
  const alias = extraStr(n, "Alias");
  const object = n.nodeType === "CTE Scan" || n.nodeType === "WorkTable Scan" ? extraStr(n, "CTE Name") : n.nodeType === "Function Scan" ? extraStr(n, "Function Name") : null;
  if (object !== null && alias !== null && object !== alias) s += ` on ${object} ${alias}`;
  else if (object !== null) s += ` on ${object}`;
  else if (alias !== null && ALIAS_TARGETS.has(n.nodeType)) s += ` on ${alias}`;
  return s;
}

/** Заголовок узла без стоимости — как в psql: имя (`Hash Left Join`, `Parallel Seq Scan`), индекс и отношение с псевдонимом. */
export function nodeLabel(n: PlanNode): string {
  return nodeName(n) + target(n);
}

/** Строки дерева в порядке обхода с глубиной; дети свёрнутых узлов пропускаются. */
export function flatten(root: PlanNode, collapsed: ReadonlySet<number>): { node: PlanNode; depth: number }[] {
  const out: { node: PlanNode; depth: number }[] = [];
  const walk = (n: PlanNode, depth: number) => {
    out.push({ node: n, depth });
    if (!collapsed.has(n.id)) n.children.forEach((c) => walk(c, depth + 1));
  };
  walk(root, 0);
  return out;
}

/** Ключи пометок узла (`diag.plan.marks.*`). */
export function marks(n: PlanNode): string[] {
  const out: string[] = [];
  if (n.hot) out.push("hot");
  if (n.neverExecuted) out.push("neverExecuted");
  if (n.sortSpaceType === "Disk" || (n.hashBatches ?? 0) > 1) out.push("disk");
  if ((n.lossyHeapBlocks ?? 0) > 0) out.push("lossy");
  if ((n.heapFetches ?? 0) > 0) out.push("heapFetches");
  if (n.workersPlanned !== null && n.workersLaunched !== null && n.workersLaunched < n.workersPlanned) out.push("workersShort");
  if (n.parallelAware) out.push("parallel");
  return out;
}

/**
 * Во сколько раз факт отличается от оценки строк в любую сторону (округлено до целого); `Infinity` — отношение бесконечно,
 * `null` — ошибки оценки нет. Ошибку размечает флаг `misestimated`, а не число: `estimateRatio` при нём может быть `null`
 * (оценка 0 строк, факт больше — отношение бесконечно) или 0 (факт 0 при большой оценке) — оба случая бесконечные.
 */
export function estimateFactor(n: PlanNode): number | null {
  if (!n.misestimated) return null;
  const r = n.estimateRatio;
  if (r === null || !Number.isFinite(r) || r <= 0) return Number.POSITIVE_INFINITY;
  const k = Math.round(r >= 1 ? r : 1 / r);
  return Number.isFinite(k) ? k : Number.POSITIVE_INFINITY;
}

/** Ошибка оценки строк как «×N» (направление видно из «оценка → факт»); `×∞` — бесконечная; `null` — ошибки нет. */
export function estimateLabel(n: PlanNode): string | null {
  const k = estimateFactor(n);
  if (k === null) return null;
  return Number.isFinite(k) ? `×${k}` : "×∞";
}

/** Порог «красной» ошибки оценки: ×10 — оранжевая (порог `misestimated` в `pg-advisor`), ×100 и больше — красная (спец. P3 §8.6). */
const SEVERE_ESTIMATE_FACTOR = 100;

/** Степень ошибки оценки: `severe` — ×100 и больше (и бесконечная), `high` — от ×10 до ×100, `null` — ошибки нет. */
export function estimateSeverity(n: PlanNode): "high" | "severe" | null {
  const k = estimateFactor(n);
  if (k === null) return null;
  return k >= SEVERE_ESTIMATE_FACTOR ? "severe" : "high";
}

const f2 = (x: number) => x.toFixed(2);
const f3 = (x: number) => x.toFixed(3);
/** Отступ в парах пробелов, как `es->indent` в `ExplainNode`. */
const pad = (indent: number) => "  ".repeat(indent);

/** Заголовок узла с оценкой и фактом: `Seq Scan on t  (cost=… rows=… width=…) (actual time=… rows=… loops=…)`. */
function head(n: PlanNode): string {
  let s = `${nodeLabel(n)}  (cost=${f2(n.startupCost)}..${f2(n.totalCost)} rows=${Math.round(n.planRows)} width=${n.planWidth})`;
  if (n.actual) {
    const { startupMs, totalMs, rows, loops } = n.actual;
    if (n.neverExecuted) s += " (never executed)";
    // `TIMING OFF`: времён нет, psql печатает только строки и циклы.
    else if (startupMs !== null && totalMs !== null) s += ` (actual time=${f3(startupMs)}..${f3(totalMs)} rows=${Math.round(rows)} loops=${Math.round(loops)})`;
    else s += ` (actual rows=${Math.round(rows)} loops=${Math.round(loops)})`;
  }
  return s;
}

/** Группа счётчиков буферов (`shared hit=… read=…`); нулевые счётчики и пустая группа опускаются. */
function bufferGroup(name: string, counters: [string, number][]): string | null {
  const parts = counters.filter(([, v]) => v > 0).map(([k, v]) => `${k}=${v}`);
  return parts.length ? `${name} ${parts.join(" ")}` : null;
}

function renderNode(n: PlanNode, indent: number, lines: string[]): void {
  if (n.subplanName) {
    // Имя вложенного плана (`SubPlan 1`, `InitPlan 1`, `CTE d`) — отдельной строкой над узлом.
    lines.push(`${pad(indent)}${n.subplanName}`);
    indent += 1;
  }
  let prefix = "";
  if (indent > 0) {
    prefix = `${pad(indent)}->  `;
    indent += 2;
  }
  lines.push(prefix + head(n));
  indent += 1;
  const rest = pad(indent);
  const line = (label: string, v: string) => lines.push(`${rest}${label}: ${v}`);
  if (n.indexCond) line("Index Cond", n.indexCond);
  if (n.recheckCond) line("Recheck Cond", n.recheckCond);
  if (n.hashCond) line("Hash Cond", n.hashCond);
  if (n.mergeCond) line("Merge Cond", n.mergeCond);
  if (n.joinFilter) line("Join Filter", n.joinFilter);
  if (n.filter) line("Filter", n.filter);
  if (n.rowsRemovedByFilter !== null) line("Rows Removed by Filter", String(n.rowsRemovedByFilter));
  if (n.rowsRemovedByIndexRecheck !== null) line("Rows Removed by Index Recheck", String(n.rowsRemovedByIndexRecheck));
  if (n.rowsRemovedByJoinFilter !== null) line("Rows Removed by Join Filter", String(n.rowsRemovedByJoinFilter));
  if (n.sortKey.length) line("Sort Key", n.sortKey.join(", "));
  if (n.groupKey.length) line("Group Key", n.groupKey.join(", "));
  if (n.sortMethod) line("Sort Method", `${n.sortMethod}${n.sortSpaceType !== null && n.sortSpaceKb !== null ? `  ${n.sortSpaceType}: ${n.sortSpaceKb}kB` : ""}`);
  if (n.hashBatches !== null) line("Buckets", `${n.hashBuckets ?? 0}  Batches: ${n.hashBatches}  Memory Usage: ${n.peakMemoryKb ?? 0}kB`);
  if (n.exactHeapBlocks !== null || n.lossyHeapBlocks !== null) line("Heap Blocks", `exact=${n.exactHeapBlocks ?? 0} lossy=${n.lossyHeapBlocks ?? 0}`);
  if (n.heapFetches !== null) line("Heap Fetches", String(n.heapFetches));
  if (n.workersPlanned !== null) line("Workers Planned", String(n.workersPlanned));
  if (n.workersLaunched !== null) line("Workers Launched", String(n.workersLaunched));
  if (n.subplansRemoved !== null) line("Subplans Removed", String(n.subplansRemoved));
  if (n.buffers) {
    const b = n.buffers;
    const parts = [
      bufferGroup("shared", [["hit", b.sharedHit], ["read", b.sharedRead], ["dirtied", b.sharedDirtied], ["written", b.sharedWritten]]),
      bufferGroup("local", [["hit", b.localHit], ["read", b.localRead], ["dirtied", b.localDirtied], ["written", b.localWritten]]),
      b.tempRead > 0 || b.tempWritten > 0 ? `temp read=${b.tempRead} written=${b.tempWritten}` : null,
    ].filter((p): p is string => p !== null);
    if (parts.length) line("Buffers", parts.join(", "));
  }
  if (n.ioReadMs !== null || n.ioWriteMs !== null) line("I/O Timings", `read=${f3(n.ioReadMs ?? 0)} write=${f3(n.ioWriteMs ?? 0)}`);
  n.children.forEach((c) => renderNode(c, indent, lines));
}

/** Зеркало `pg_advisor::plan::to_text`: текст плана в стиле psql для копирования и для LLM. */
export function renderText(t: PlanTree): string {
  const lines: string[] = [];
  renderNode(t.root, 0, lines);
  // `SETTINGS`: параметры, отличные от значений по умолчанию, — одной строкой после дерева (`ExplainPrintSettings`).
  if (t.settings.length) lines.push(`Settings: ${t.settings.map((s) => `${s.name} = '${s.value}'`).join(", ")}`);
  if (t.planningMs !== null) lines.push(`Planning Time: ${f3(t.planningMs)} ms`);
  for (const tr of t.triggers) lines.push(`Trigger ${tr.name}: time=${f3(tr.timeMs)} calls=${tr.calls}`);
  if (t.jit) {
    lines.push("JIT:", `  Functions: ${t.jit.functions}`);
    // Строка времён — только когда они есть (ANALYZE с TIMING), как в psql.
    const j = t.jit;
    if (j.totalMs !== null) {
      const ms = (v: number | null) => f3(v ?? 0);
      lines.push(`  Timing: Generation ${ms(j.generationMs)} ms, Inlining ${ms(j.inliningMs)} ms, Optimization ${ms(j.optimizationMs)} ms, Emission ${ms(j.emissionMs)} ms, Total ${ms(j.totalMs)} ms`);
    }
  }
  if (t.executionMs !== null) lines.push(`Execution Time: ${f3(t.executionMs)} ms`);
  return `${lines.join("\n")}\n`;
}
