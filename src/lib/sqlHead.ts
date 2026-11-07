/** Подпись текста — не длиннее (полный текст — в подсказке). */
export const HEAD_MAX = 80;

/** Хвостовые `;` и пробелы снимаются, как у Rust (`pg_advisor::sqltext::normalize`): так хранится текст запроса на оптимизацию. */
export function normalizeSql(sql: string): string {
  let s = sql.trim();
  while (s.endsWith(";")) s = s.slice(0, -1).trimEnd();
  return s;
}

/** Тот же текст запроса — с точностью до хвостовых `;` и пробелов (`normalizeSql`). */
export function sameSql(a: string, b: string): boolean {
  return normalizeSql(a) === normalizeSql(b);
}

/** Непустые строки текста без отступов. */
const lines = (sql: string) =>
  sql
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l !== "");

/**
 * Начало текста рядом с другим (смоук P4b, R4; P4c, R23): первая непустая строка; если она та же, что у `current` (предложение
 * модели обычно начинается так же), — первая строка, которой тексты расходятся, с «…» впереди. Не длиннее `HEAD_MAX`.
 */
export function differingHead(text: string, current: string): string {
  const own = lines(text);
  const now = lines(current);
  if (own.length === 0) return "";
  if (own[0] !== now[0]) return own[0].slice(0, HEAD_MAX);
  const i = own.findIndex((l, k) => l !== now[k]);
  return i < 0 ? own[0].slice(0, HEAD_MAX) : `… ${own[i]}`.slice(0, HEAD_MAX);
}
