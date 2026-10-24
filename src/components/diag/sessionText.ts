import { i18n } from "../../i18n";

/** Знаков запроса в списке сессий и дереве блокировок; полный текст — в панели сессии. */
export const QUERY_PREVIEW_CHARS = 120;
/** Подсказка (`title`) с текстом запроса не длиннее: подсказка в 16 КБ на каждую строку DOM ни к чему. */
export const QUERY_TITLE_CHARS = 500;

/**
 * Текст запроса одной строкой для списка: пробелы и переводы строк схлопываются, длиннее `max` символов — обрезается
 * с «…». Берётся только начало текста (до `max * 4` знаков): запрос может весить 16 КБ, а в списке их тысяча.
 */
export function oneLine(text: string | null, max = QUERY_PREVIEW_CHARS): string {
  if (!text) return "";
  const chars = Array.from(text.slice(0, max * 4).replace(/\s+/g, " ").trim());
  return chars.length > max || text.length > max * 4 ? `${chars.slice(0, max).join("")}…` : chars.join("");
}

/**
 * pid блокирующих сессий через запятую. `0` — не сессия: `pg_blocking_pids` отдаёт его для подготовленной
 * транзакции (pid у неё нет), а в дереве блокировок — и для сессии, успевшей завершиться между запросами.
 */
export function blockersText(pids: number[]): string {
  return pids.map((p) => (p === 0 ? i18n.t("diag.locks.preparedOrEnded") : String(p))).join(", ");
}
