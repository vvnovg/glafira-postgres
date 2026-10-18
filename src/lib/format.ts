import { currentLocale } from "../i18n";

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
  if (n < 1024 * 1024) return `${num(n / 1024, 1)} KB`;
  return `${num(n / (1024 * 1024), 1)} MB`;
}

/** undefined — ещё загружается, null — недоступно. */
export function formatCount(n?: number | null): string {
  if (n === undefined) return "…";
  if (n === null) return "—";
  return num(n);
}
