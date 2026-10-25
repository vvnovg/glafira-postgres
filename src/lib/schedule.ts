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
