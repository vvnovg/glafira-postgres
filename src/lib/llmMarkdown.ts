import type { TFunction } from "i18next";
import type { LlmBundle, QueryStat } from "../api/types";
import { BLOCK_BYTES } from "../components/queries/queriesText";
import { formatBytes, formatCount, formatMs, formatPercent } from "./format";

/** Ключи предупреждений сборки — литералами (правило словарей); неизвестный ключ попадает в текст как есть. */
const WARNING_KEYS: Record<string, string> = {
  paramsNeeded: "llm.warnings.paramsNeeded",
  queryGone: "llm.warnings.queryGone",
  planFailed: "llm.warnings.planFailed",
  ddlFailed: "llm.warnings.ddlFailed",
};

/** `170004` → `17.4`; `130012` → `13.12`. */
export const versionText = (num: number) => `${Math.floor(num / 10_000)}.${num % 10_000}`;

/** Неразрывные пробелы чисел (`1 000` в ru) — обычными: markdown вставляют в чаты и редакторы, где они мешают. */
const plain = (s: string) => s.replace(/[  ]/g, " ");

/** Блок кода: ограждение длиннее любой цепочки обратных кавычек в тексте, чтобы текст не закрыл блок раньше времени. */
function fence(lang: string, body: string): string {
  const longest = Math.max(0, ...(body.match(/`+/g) ?? []).map((m) => m.length));
  const ticks = "`".repeat(Math.max(3, longest + 1));
  return `${ticks}${lang}\n${body}\n${ticks}`;
}

/**
 * Таблица метрик. Строка планирования — только если планирование учитывалось: при `track_planning = off` счётчики нулевые,
 * и «0 мс» читалось бы как факт.
 */
function statsTable(s: QueryStat, t: TFunction): string {
  const rows: [string, string][] = [
    [t("llm.stats.calls"), formatCount(s.calls)],
    [t("llm.stats.total"), formatMs(s.totalExecMs)],
    [t("llm.stats.mean"), formatMs(s.meanExecMs)],
    [t("llm.stats.minMax"), s.minExecMs === null || s.maxExecMs === null ? "—" : `${formatMs(s.minExecMs)} / ${formatMs(s.maxExecMs)}`],
    [t("llm.stats.stddev"), s.stddevExecMs === null ? "—" : formatMs(s.stddevExecMs)],
    [t("llm.stats.rowsPerCall"), formatCount(s.rowsPerCall, 1)],
    [t("llm.stats.hitRatio"), s.hitRatio === null ? "—" : formatPercent(s.hitRatio * 100)],
    [t("llm.stats.read"), formatBytes(s.sharedRead * BLOCK_BYTES)],
    [t("llm.stats.temp"), formatBytes(s.tempWritten * BLOCK_BYTES)],
    [t("llm.stats.wal"), formatBytes(s.walBytes)],
  ];
  if (s.plans !== 0 || s.totalPlanMs !== 0) rows.push([t("llm.stats.planning"), formatMs(s.totalPlanMs)]);
  return [`| ${t("llm.stats.metric")} | ${t("llm.stats.value")} |`, "|---|---|", ...rows.map(([k, v]) => `| ${k} | ${plain(v)} |`)].join("\n");
}

/**
 * Markdown для LLM (спец. P3 §7.4): заголовки — на языке интерфейса; данные (текст запроса, план, DDL) — как есть;
 * промптов нет. Блоки разделяются пустой строкой; содержимое блоков не переформатируется.
 */
export function llmMarkdown(b: LlmBundle, t: TFunction): string {
  const blocks: string[] = [`# ${t("llm.title")}`, `## ${t("llm.env")}`];
  const env = [`- PostgreSQL ${versionText(b.serverVersionNum)}${b.inRecovery ? ` (${t("llm.replica")})` : ""}`];
  if (b.pgssVersion) env.push(`- pg_stat_statements ${b.pgssVersion}`);
  env.push(`- ${t("llm.node")}: ${b.member}, ${t("llm.database")}: ${b.database}`);
  blocks.push(env.join("\n"));
  if (b.text) {
    blocks.push(`## ${t("llm.query")}`, fence("sql", b.text));
    // Усечённый текст сервер не планирует: пометка объясняет и отсутствие раздела «План».
    if (b.textTruncated) blocks.push(`_${t("llm.truncated")}_`);
  }
  if (b.stats && (b.stats.sinceReset || b.stats.window3600)) {
    blocks.push(`## ${t("llm.statistics")}`);
    if (b.stats.sinceReset) blocks.push(`### ${t("llm.sinceReset")}`, statsTable(b.stats.sinceReset, t));
    if (b.stats.window3600) blocks.push(`### ${t("llm.lastHour")}`, statsTable(b.stats.window3600, t));
  }
  if (b.planText) blocks.push(`## ${t("llm.plan")}`, fence("", b.planText.trimEnd()));
  if (b.tables.length > 0 || b.tablesOmitted > 0) {
    blocks.push(`## ${t("llm.tables")}`);
    for (const tb of b.tables) blocks.push(`### ${tb.schema}.${tb.name}`, fence("sql", [tb.ddl.trimEnd(), ...tb.indexes].join("\n")));
    // Не вошедшие таблицы: сверх лимита или с непрочитанным DDL — подпись покрывает обе причины.
    if (b.tablesOmitted > 0) blocks.push(`_${t("llm.moreTables", { count: b.tablesOmitted })}_`);
  }
  if (b.warnings.length > 0) blocks.push(`## ${t("llm.notes")}`, b.warnings.map((w) => `- ${WARNING_KEYS[w] ? t(WARNING_KEYS[w]) : w}`).join("\n"));
  return `${blocks.join("\n\n")}\n`;
}
