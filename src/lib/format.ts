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

/**
 * Время запланированного действия Patroni (`at` — ISO 8601 со смещением, как пришло из `/cluster`): в часовом поясе
 * компьютера, тем же форматом, что и остальные метки. Строка, которую не разобрать, показывается как есть.
 */
export function formatScheduled(at: string): string {
  const ms = Date.parse(at);
  return Number.isNaN(ms) ? at : formatTimestamp(ms);
}

/** Время дня «ЧЧ:ММ:СС» по местным часам: в «Обновлено» шапки дата и миллисекунды — шум (полное время — в подсказке). */
export function formatTime(ms: number): string {
  const d = new Date(ms);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/**
 * Короткое время для списков (смоук P4b, R3/R4): сегодня — «12:20», раньше в этом году — «07.11 01:53», в другом году —
 * «07.11.2025 01:53». День и месяц — по правилам языка интерфейса (en — «11/07 01:53»), часы — 24-часовые, как у остальных
 * меток. Полное время показывают в подсказке (`formatTimestamp`). `now` — «сегодня» для тестов.
 */
export function formatShortTimestamp(ms: number, now = Date.now()): string {
  const d = new Date(ms);
  const today = new Date(now);
  const time = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  const sameYear = d.getFullYear() === today.getFullYear();
  if (sameYear && d.getMonth() === today.getMonth() && d.getDate() === today.getDate()) return time;
  const day = new Intl.DateTimeFormat(currentLocale(), sameYear ? { day: "2-digit", month: "2-digit" } : { day: "2-digit", month: "2-digit", year: "numeric" });
  return `${day.format(d)} ${time}`;
}

/** День `ГГГГ-ММ-ДД` (UTC) по правилам языка интерфейса: ru — «07.11.2026», en — «11/07/2026». */
export function formatDay(ymd: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(ymd);
  if (!m) return ymd;
  const day = new Date(Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3])));
  return new Intl.DateTimeFormat(currentLocale(), { day: "2-digit", month: "2-digit", year: "numeric", timeZone: "UTC" }).format(day);
}

/**
 * Доллары по правилам языка интерфейса (`$0.42`, `0,42 $`), два знака:
 * - `estimate` — оценка стоимости: меньше цента — одна значащая цифра (`0,006 $`), а не «0,00 $»;
 * - `price` — цена за 1 млн токенов: до четырёх знаков, чтобы не округлять дешёвые цены (`0,015 $`, `0,435 $`).
 */
export function formatUsd(v: number, kind: "exact" | "estimate" | "price" = "exact"): string {
  const small = kind === "estimate" && v > 0 && v < 0.01;
  const digits = small ? { maximumSignificantDigits: 1 } : kind === "price" ? { minimumFractionDigits: 2, maximumFractionDigits: 4 } : {};
  return new Intl.NumberFormat(currentLocale(), { style: "currency", currency: "USD", ...digits }).format(v);
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

/**
 * Среднее число активных сессий (AAS): до одного знака после запятой («1,4», целое — «1»), а при значении меньше 0,1 —
 * до двух («0,04»), чтобы малая, но не нулевая нагрузка не превращалась в «0».
 */
export function formatAas(n: number): string {
  return n.toLocaleString(currentLocale(), { maximumFractionDigits: n < 0.1 ? 2 : 1 });
}

/** Предел i64 (9223372036854775807) как BigInt; сам литерал не годится — в `number` он округляется до 2^63. */
const I64_MAX = BigInt("9223372036854775807");

/**
 * Значение bigint из каталога. Поля последовательностей Rust отдаёт строкой (`i64_string`): `number` точен только до
 * 2^53, а граница bigint — 9223372036854775807; строка форматируется через BigInt без потери разрядов. Число (старый
 * вид данных, счётчики) поправляется: i64::MAX из JSON-числа приходит как 2^63 и без поправки показывался бы как …808,
 * остальные значения выше 2^53 приблизительны — точность теряется до интерфейса.
 */
export function formatInt64(n: number | string | null): string {
  if (n === null) return "—";
  const locale = currentLocale();
  if (typeof n === "string") return BigInt(n.trim()).toLocaleString(locale);
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

/** Ступени миллисекунд: знаков после запятой и верхняя граница ступени (не включая). */
const MS_TIERS: [number, number][] = [
  [2, 10],
  [1, 100],
  [0, 1000],
];

/**
 * Миллисекунды; точность — по величине (около трёх значащих цифр), одно правило для всех мест:
 * - ровно 0 (и отрицательное — сервер может отдать −0,001) — «0 мс»; больше 0, но меньше 0,01 — «<0,01 мс», а не ноль;
 * - меньше 10 мс — два знака («0,42 мс»), меньше 100 — один («12,3 мс»), меньше 1000 — целые («999 мс»);
 * - от 1000 мс — секунды с двумя знаками («1,00 с» ровно на 1000, «1,25 с»); от минуты — длительность («1 мин 30 с»).
 * Ступень выбирается по уже округлённому значению: число, которое при показе округлилось бы до границы, показывается
 * следующей ступенью (9,996 → «10,0 мс», 999,6 → «1,00 с», 59 999,9 → «1 мин»), а не «10,00 мс», «1 000 мс», «60,00 с».
 */
export function formatMs(ms: number): string {
  const msShort = i18n.t("diag.duration.msShort");
  if (ms <= 0) return `0 ${msShort}`;
  if (ms < 0.01) return `<${num(0.01, 2)} ${msShort}`;
  for (const [digits, below] of MS_TIERS) {
    if (Number(ms.toFixed(digits)) < below) return `${num(ms, digits)} ${msShort}`;
  }
  const s = ms / 1000;
  if (Number(s.toFixed(2)) < 60) return `${num(s, 2)} ${i18n.t("diag.duration.secShort")}`;
  // Длительность считает целые секунды: округление, а не отбрасывание, — как у ступеней выше.
  return formatDuration(Math.round(s));
}
