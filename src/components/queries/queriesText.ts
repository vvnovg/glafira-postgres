import type { PgssMeta, QueryKey, QuerySort } from "../../api/types";

/** Заголовок вкладки и первая строка таблицы: начало текста одной строкой; без текста — queryId. */
export function queryTitle(text: string | null, queryId: string, max = 60): string {
  if (!text) return queryId;
  const line = text.replace(/\s+/g, " ").trim();
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

/** Версия pgss как пара чисел; `null` — не разобралась. */
export function pgssVersion(v: string | null | undefined): [number, number] | null {
  const m = /^(\d+)\.(\d+)/.exec(v ?? "");
  return m ? [Number(m[1]), Number(m[2])] : null;
}

/** Зеркало `QuerySort::available`: min/max/stddev нет в окне, JIT — только с pgss 1.10. */
export function sortAvailable(sort: QuerySort, pgss: PgssMeta | null, window: boolean): boolean {
  if (sort === "maxTime" || sort === "stddevTime") return !window;
  if (sort === "jitTime") {
    const v = pgssVersion(pgss?.version);
    return v !== null && (v[0] > 1 || v[1] >= 10);
  }
  return true;
}

/** Ключ строки pgss одной строкой (для `Map`, `Set` и `key` списка). */
export const QUERY_KEY_ID = (k: QueryKey) => `${k.queryId}:${k.dbid}:${k.userid}:${k.toplevel ? 1 : 0}`;
