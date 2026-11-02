import type { TFunction } from "i18next";
import type { PgssMeta, QueryKey, QuerySort, QueryStat } from "../../api/types";
import { currentLocale } from "../../i18n";

/** Размер блока PostgreSQL, байты: счётчики блоков pgss переводятся в байты по нему. */
export const BLOCK_BYTES = 8192;

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

/** Зеркало `QuerySort::available`: min/max/stddev нет в окне, JIT — только с pgss 1.10, CPU — только с `pg_stat_kcache`. */
export function sortAvailable(sort: QuerySort, pgss: PgssMeta | null, window: boolean): boolean {
  if (sort === "maxTime" || sort === "stddevTime") return !window;
  if (sort === "cpuTime") return pgss?.kcache === true;
  if (sort === "jitTime") {
    const v = pgssVersion(pgss?.version);
    return v !== null && (v[0] > 1 || v[1] >= 10);
  }
  return true;
}

/** CPU строки (user + sys), мс; `null` — колонок `pg_stat_kcache` нет. */
export function cpuMs(s: Pick<QueryStat, "cpuUserMs" | "cpuSysMs">): number | null {
  return s.cpuUserMs === null || s.cpuSysMs === null ? null : s.cpuUserMs + s.cpuSysMs;
}

/** Ключ строки pgss одной строкой (для `Map`, `Set` и `key` списка). */
export const QUERY_KEY_ID = (k: QueryKey) => `${k.queryId}:${k.dbid}:${k.userid}:${k.toplevel ? 1 : 0}`;

/**
 * Узел и время по адресу файла снимка `<каталог узла>/<время>.json` (зеркало `member_of_dir`): каталог — имя узла в
 * `%XX`-форме; не код (старый каталог, `%` без двух цифр) — как есть. Для подписи, когда сведений о снимке нет.
 */
export function snapshotFileParts(file: string): { member: string; stamp: string } {
  const slash = file.indexOf("/");
  const dir = slash < 0 ? "" : file.slice(0, slash);
  const stamp = file.slice(slash + 1).replace(/\.json$/, "");
  let member = dir;
  try {
    member = decodeURIComponent(dir);
  } catch {
    // Не код — имя каталога как есть.
  }
  return { member, stamp };
}

/**
 * Изменение средней между снимками (`ratio` = после / до) словами с направлением: замедление — «в 3 раза медленнее»,
 * ускорение — обратным коэффициентом «в 2,5 раза быстрее» (а не «×0,4»). Коэффициент — до одного знака; склонение
 * («раза»/«раз») — по нему же. Нулевое, бесконечное и нечисловое отношение — прочерк.
 */
export function changeRatioText(ratio: number, t: TFunction): string {
  if (!Number.isFinite(ratio) || ratio <= 0) return "—";
  const slower = ratio >= 1;
  const factor = Math.round((slower ? ratio : 1 / ratio) * 10) / 10;
  const x = factor.toLocaleString(currentLocale(), { maximumFractionDigits: 1 });
  return t(slower ? "diag.snapshots.slower" : "diag.snapshots.faster", { count: factor, x });
}
