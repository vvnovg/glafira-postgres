import { currentLocale, i18n } from "../i18n";

const pad = (n: number, w = 2) => String(n).padStart(w, "0");

export function formatTimestamp(ms: number | null, utc = false): string {
  if (ms === null) return "—";
  const d = new Date(ms);
  const parts = utc
    ? [d.getUTCFullYear(), d.getUTCMonth() + 1, d.getUTCDate(), d.getUTCHours(), d.getUTCMinutes(), d.getUTCSeconds(), d.getUTCMilliseconds()]
    : [d.getFullYear(), d.getMonth() + 1, d.getDate(), d.getHours(), d.getMinutes(), d.getSeconds(), d.getMilliseconds()];
  const [y, mo, da, h, mi, s, milli] = parts;
  return `${y}-${pad(mo)}-${pad(da)} ${pad(h)}:${pad(mi)}:${pad(s)}.${pad(milli, 3)}`;
}

const num = (n: number, digits = 0) =>
  n.toLocaleString(currentLocale(), { minimumFractionDigits: digits, maximumFractionDigits: digits });

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  // Единицу выбираем по значению после округления до одного знака: 1 048 575 B — это «1,0 MB», а не «1 024,0 KB».
  while (Number(v.toFixed(1)) >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${num(v, 1)} ${units[i]}`;
}

/** undefined — ещё загружается, null — недоступно. `digits` — знаков после запятой (скорости, например TPS, — один). */
export function formatCount(n?: number | null, digits = 0): string {
  if (n === undefined) return "…";
  if (n === null) return "—";
  return num(n, digits);
}

/** Доля в процентах с одним знаком после запятой («49,5 %»); null — нет данных. */
export function formatPercent(n: number | null): string {
  if (n === null) return "—";
  return `${n.toLocaleString(currentLocale(), { maximumFractionDigits: 1 })} %`;
}

/** Предел i64 (9223372036854775807) как BigInt; сам литерал не годится — в `number` он округляется до 2^63. */
const I64_MAX = BigInt("9223372036854775807");

/**
 * Значение bigint из каталога. Rust отдаёт i64 числом JSON, а `number` точен только до 2^53: настоящая
 * граница bigint (9223372036854775807) приходит как 2^63 и без поправки показывалась бы как …808.
 * Остальные значения выше 2^53 приблизительны — точность теряется до интерфейса.
 */
export function formatInt64(n: number | null): string {
  if (n === null) return "—";
  const locale = currentLocale();
  if (n >= 2 ** 63) return I64_MAX.toLocaleString(locale);
  // Intl дописывает нули после 17 значащих цифр (…776 000); через BigInt печатается точное значение числа.
  if (Math.abs(n) > Number.MAX_SAFE_INTEGER) return BigInt(n).toLocaleString(locale);
  return num(n);
}

const DURATION_UNITS: [number, "d" | "h" | "m" | "s"][] = [[86_400, "d"], [3_600, "h"], [60, "m"], [1, "s"]];

/**
 * Длительность в секундах — старшая единица и только смежная за ней, если она не нулевая («1 ч 2 мин», «1 д», «42 с»):
 * 86 405 с — «1 д», а не «1 д 5 с» (секунды при днях — шум). null — «—».
 * Сервер может отдать небольшое отрицательное значение (−0,001 с: `now()` — начало транзакции, а событие позже): оно показывается как «0 с».
 */
export function formatDuration(seconds: number | null): string {
  if (seconds === null) return "—";
  const s = Math.max(0, Math.floor(seconds));
  // Меньше секунды (0) — «0 с»: старшей единицей остаётся последняя.
  const found = DURATION_UNITS.findIndex(([size]) => s >= size);
  const i = found < 0 ? DURATION_UNITS.length - 1 : found;
  const [size, key] = DURATION_UNITS[i];
  const parts = [i18n.t(`diag.duration.${key}`, { n: Math.floor(s / size) })];
  const next = DURATION_UNITS[i + 1];
  if (next) {
    const n = Math.floor((s % size) / next[0]);
    if (n > 0) parts.push(i18n.t(`diag.duration.${next[1]}`, { n }));
  }
  return parts.join(" ");
}
