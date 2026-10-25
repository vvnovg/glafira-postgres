const pad = (n: number) => String(Math.abs(n)).padStart(2, "0");

const LOCAL = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?$/;

/**
 * Значение `<input type="datetime-local">` («2026-10-25T14:30») в RFC 3339 с местным смещением
 * («2026-10-25T14:30:00+03:00»): Patroni принимает время со смещением. `offsetMin` — для тестов
 * (минуты к востоку от UTC); по умолчанию — смещение этой даты в часовом поясе машины.
 * Пустое или некорректное значение (не «ГГГГ-ММ-ДДTЧЧ:ММ[:СС]», несуществующая дата) — `Error`:
 * вызывающий проверяет ввод до вызова, а не отправляет в Patroni время вида `NaN`.
 */
export function toRfc3339(local: string, offsetMin?: number): string {
  const m = LOCAL.exec(local);
  if (!m) throw new Error(`Invalid local date-time: "${local}"`);
  const [, y, mo, d, h, mi, s = "00"] = m;
  const t = new Date(Date.UTC(+y, +mo - 1, +d, +h, +mi, +s));
  // Несуществующие даты и время (31 февраля, 24:00) Date не отвергает, а сдвигает: сверяем обратно.
  if (t.getUTCFullYear() !== +y || t.getUTCMonth() !== +mo - 1 || t.getUTCDate() !== +d || t.getUTCHours() !== +h || t.getUTCMinutes() !== +mi || t.getUTCSeconds() !== +s) {
    throw new Error(`Invalid local date-time: "${local}"`);
  }
  const date = `${y}-${mo}-${d}`;
  const hms = `${h}:${mi}:${s}`;
  const off = offsetMin ?? -new Date(`${date}T${hms}`).getTimezoneOffset();
  const sign = off >= 0 ? "+" : "-";
  return `${date}T${hms}${sign}${pad(Math.trunc(off / 60))}:${pad(off % 60)}`;
}

/** Через сколько минут по умолчанию назначать действие по расписанию: успеть передумать и отменить. */
export const DEFAULT_LEAD_MIN = 15;
const STEP_MS = 5 * 60_000;

/** Значение `<input type="datetime-local">` («ГГГГ-ММ-ДДTЧЧ:ММ») для момента `ms` по местным часам. */
export function toLocalInput(ms: number): string {
  const d = new Date(ms);
  const p2 = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p2(d.getMonth() + 1)}-${p2(d.getDate())}T${p2(d.getHours())}:${p2(d.getMinutes())}`;
}

/** Время по умолчанию для действия по расписанию: через `DEFAULT_LEAD_MIN` минут, округлённое вверх до 5 минут. */
export function defaultScheduleLocal(nowMs: number = Date.now()): string {
  return toLocalInput(Math.ceil((nowMs + DEFAULT_LEAD_MIN * 60_000) / STEP_MS) * STEP_MS);
}

/** Секунд от `nowMs` до момента `local` (`datetime-local`); пустое или неразборчивое значение — `null`. */
export function secondsUntil(local: string, nowMs: number = Date.now()): number | null {
  try {
    return (Date.parse(toRfc3339(local)) - nowMs) / 1000;
  } catch {
    return null;
  }
}
