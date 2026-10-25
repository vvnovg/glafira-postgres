const pad = (n: number) => String(Math.abs(n)).padStart(2, "0");

/**
 * Значение `<input type="datetime-local">` («2026-10-25T14:30») в RFC 3339 с местным смещением
 * («2026-10-25T14:30:00+03:00»): Patroni принимает время со смещением. `offsetMin` — для тестов
 * (минуты к востоку от UTC); по умолчанию — смещение этой даты в часовом поясе машины.
 */
export function toRfc3339(local: string, offsetMin?: number): string {
  const [date, time = "00:00"] = local.split("T");
  const hms = time.length === 5 ? `${time}:00` : time;
  const off = offsetMin ?? -new Date(`${date}T${hms}`).getTimezoneOffset();
  const sign = off >= 0 ? "+" : "-";
  return `${date}T${hms}${sign}${pad(Math.trunc(off / 60))}:${pad(off % 60)}`;
}
