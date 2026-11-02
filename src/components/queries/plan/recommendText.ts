import type { Recommendation } from "../../../api/types";
import { i18n } from "../../../i18n";
import { formatBytes, formatCount, formatMs, formatPercent } from "../../../lib/format";

/**
 * Параметры текста рекомендации: числа по суффиксам (как у находок: `…Bytes`, `…Pct`, `…Ms`), `…Kb` — килобайты как байты,
 * `ratio` — с одним знаком до 10, остальные числа — с разделителями; строки (`table`, `tables`, `tablesHead`, `columns`, `keys`, `index`, `condition`, `rewrite`,
 * `name`) — как есть. Добавляются подписи `directionLabel` (ошибка оценки) и `kindLabel` (что уходит на диск).
 */
export function recommendParams(r: Recommendation): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(r.params)) {
    if (v === null) continue;
    if (typeof v === "number" && k.endsWith("Bytes")) out[k] = formatBytes(v);
    else if (typeof v === "number" && k.endsWith("Kb")) out[k] = formatBytes(v * 1024);
    else if (typeof v === "number" && k.endsWith("Pct")) out[k] = formatPercent(v);
    else if (typeof v === "number" && k.endsWith("Ms")) out[k] = formatMs(v);
    else if (typeof v === "number" && k === "ratio") out[k] = formatCount(v, v < 10 ? 1 : 0);
    else if (typeof v === "number") out[k] = formatCount(v);
    else out[k] = String(v);
  }
  if (typeof r.params.direction === "string") out.directionLabel = i18n.t(`recommend.direction.${r.params.direction}`);
  if (typeof r.params.kind === "string") out.kindLabel = i18n.t(`recommend.spillKinds.${r.params.kind}`);
  return out;
}

type Field = "title" | "evidence" | "effect" | "risks";

/**
 * Вариант текста по параметрам правила: у сгруппированного по партициям `plan_seq_scan_filter` (`partitions`) — свои
 * заголовок и доказательство (при `tablesMore` > 0 — первые партиции и «и ещё N»); у `plan_spill` по bitmap (`kind: bitmap`, без размеров) — свои заголовок и доказательство, а
 * эффект с индексом — только у сортировки с ключами; `plan_no_pruning` без ANALYZE (важность `info`) — доказательство без
 * пустых партиций, с условием-выражением по колонке (`rewrite`) — заголовок и эффект про диапазон; `plan_jit_heavy` в параллельном плане (`processes` > 1) — доказательство с числом процессов.
 */
function variant(r: Recommendation, field: Field): string {
  const p = r.params;
  switch (r.id) {
    case "plan_seq_scan_filter":
      if (typeof p.partitions === "number" && field === "title") return "titlePartitions";
      if (typeof p.partitions === "number" && field === "evidence") return typeof p.tablesMore === "number" && p.tablesMore > 0 ? "evidencePartitionsMore" : "evidencePartitions";
      break;
    case "plan_spill":
      if (p.kind === "bitmap" && field === "title") return "titleBitmap";
      if (p.kind === "bitmap" && field === "evidence") return "evidenceBitmap";
      if (field === "effect" && !(p.kind === "sort" && typeof p.keys === "string" && p.keys !== "")) return "effectMemory";
      break;
    case "plan_no_pruning":
      if (r.severity === "info" && field === "evidence") return "evidencePlan";
      if (typeof p.rewrite === "string" && field === "title") return "titleCast";
      if (typeof p.rewrite === "string" && field === "effect") return "effectCast";
      break;
    case "plan_jit_heavy":
      if (typeof p.processes === "number" && p.processes > 1 && field === "evidence") return "evidenceParallel";
      break;
  }
  return field;
}

const text = (r: Recommendation, field: Field) => i18n.t(`recommend.${r.id}.${variant(r, field)}`, recommendParams(r));

export const recommendTitle = (r: Recommendation) => text(r, "title");
export const recommendEvidence = (r: Recommendation) => text(r, "evidence");
export const recommendEffect = (r: Recommendation) => text(r, "effect");
export const recommendRisks = (r: Recommendation) => text(r, "risks");

/** Места для значений пользователя в командах `pg_advisor::plan_rules` (колонки индекса, размер work_mem). */
export const TEMPLATE_TOKENS = ["<columns>", "<work_mem>"] as const;

/** Команда — шаблон: в ней есть место для значения пользователя. */
export const isTemplate = (command: string) => TEMPLATE_TOKENS.some((tok) => command.includes(tok));

/** У рекомендации есть тексты в словаре; правило из более новой версии бэкенда — нет (карточка показывает id и команду). */
export const isKnownRecommendation = (r: Recommendation) =>
  (["title", "evidence", "effect", "risks"] as const).every((f) => i18n.exists(`recommend.${r.id}.${f}`));
