import { useMutation } from "@tanstack/react-query";
import { api } from "../api/client";
import type { ExplainRequest, ExplainResult } from "../api/types";

/** Предел номера параметра: число параметров в сообщении Bind протокола PostgreSQL — 16-битное (как `MAX_PARAMS` в Rust). */
export const MAX_PARAMS = 65_535;

const code = (s: string, i: number) => (i >= 0 && i < s.length ? s.charCodeAt(i) : -1);
const isDigit = (c: number) => c >= 48 && c <= 57;
const isAlpha = (c: number) => (c >= 65 && c <= 90) || (c >= 97 && c <= 122);
/** Символ продолжает идентификатор (`ident_cont` лексера PostgreSQL): латиница, цифры, `_`, `$` и не-ASCII. */
const identCont = (c: number) => isAlpha(c) || isDigit(c) || c === 95 || c === 36 || c >= 0x80;
/** Символ тега долларовых кавычек (`dolq_cont`): как у идентификатора, но без `$`. */
const dolqCont = (c: number) => isAlpha(c) || isDigit(c) || c === 95 || c >= 0x80;

/** Конец строки в кавычках `q`, начатой перед `from`: удвоенная кавычка — символ строки, `escapes` (E-строка) — `\` экранирует. */
function skipQuoted(s: string, from: number, q: string, escapes: boolean): number {
  let j = from;
  while (j < s.length) {
    if (escapes && s[j] === "\\") j += 2;
    else if (s[j] === q) {
      if (s[j + 1] === q) j += 2;
      else return j + 1;
    } else j += 1;
  }
  return s.length;
}

/** Открывающий тег долларовых кавычек в `at` (`$$` или `$tag$`): конец тега; `null` — это не тег. */
function dollarTag(s: string, at: number): number | null {
  let j = at + 1;
  if (dolqCont(code(s, j)) && !isDigit(code(s, j))) while (dolqCont(code(s, j))) j += 1;
  return s[j] === "$" ? j + 1 : null;
}

/**
 * Зеркало `explain::params::count_params`: наибольший номер `$N` (число полей формы). Как лексер PostgreSQL, пропускает
 * строки `'…'` (и `E'…'` с `\`), идентификаторы `"…"`, комментарии `--` и `/* *\/` (вложенные), тела долларовых кавычек;
 * `$` сразу после символа идентификатора (`a$1`) — часть идентификатора. Номер больше `MAX_PARAMS` обрезается до предела.
 * Строка JS — в кодовых единицах UTF-16: не-ASCII символ и в Rust (байты UTF-8), и здесь — символ идентификатора.
 */
export function countParams(sql: string): number {
  let max = 0;
  let i = 0;
  while (i < sql.length) {
    const c = sql[i];
    const next = sql[i + 1];
    if (c === "'") {
      // `E'…'`: буква E не должна быть хвостом идентификатора (`some_e'…'` — обычная строка).
      const escapes = (sql[i - 1] === "e" || sql[i - 1] === "E") && !identCont(code(sql, i - 2));
      i = skipQuoted(sql, i + 1, "'", escapes);
    } else if (c === '"') i = skipQuoted(sql, i + 1, '"', false);
    else if (c === "-" && next === "-") {
      const nl = sql.indexOf("\n", i);
      i = nl === -1 ? sql.length : nl + 1;
    } else if (c === "/" && next === "*") {
      let depth = 1;
      let j = i + 2;
      while (j < sql.length && depth > 0) {
        if (sql[j] === "/" && sql[j + 1] === "*") {
          depth += 1;
          j += 2;
        } else if (sql[j] === "*" && sql[j + 1] === "/") {
          depth -= 1;
          j += 2;
        } else j += 1;
      }
      i = Math.min(j, sql.length);
    } else if (c === "$" && i > 0 && identCont(code(sql, i - 1))) i += 1;
    else if (c === "$" && isDigit(code(sql, i + 1))) {
      let j = i + 1;
      let n = 0;
      while (j < sql.length && isDigit(code(sql, j))) {
        // Насыщение: номер за пределом всё равно обрезается.
        n = Math.min(n * 10 + (code(sql, j) - 48), MAX_PARAMS + 1);
        j += 1;
      }
      max = Math.max(max, Math.min(n, MAX_PARAMS));
      i = j;
    } else if (c === "$") {
      const end = dollarTag(sql, i);
      if (end === null) i += 1;
      else {
        const tag = sql.slice(i, end);
        const close = sql.indexOf(tag, end);
        i = close === -1 ? sql.length : close + tag.length;
      }
    } else i += 1;
  }
  return max;
}

/** Мутация EXPLAIN: один запуск за раз; ошибки — `AppError` из `toAppError` на месте показа. */
export function useExplain(connectionId: string) {
  return useMutation({ mutationFn: (request: ExplainRequest): Promise<ExplainResult> => api.explain(connectionId, request) });
}
