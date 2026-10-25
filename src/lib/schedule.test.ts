import { describe, expect, it } from "vitest";
import { defaultScheduleLocal, secondsUntil, toLocalInput, toRfc3339 } from "./schedule";

describe("toRfc3339", () => {
  it("местное время со смещением", () => {
    expect(toRfc3339("2026-10-25T14:30", 180)).toBe("2026-10-25T14:30:00+03:00");
    expect(toRfc3339("2026-10-25T14:30", -330)).toBe("2026-10-25T14:30:00-05:30");
    expect(toRfc3339("2026-10-25T14:30:15", 0)).toBe("2026-10-25T14:30:15+00:00");
  });

  it("без смещения берётся часовой пояс машины на эту дату", () => {
    const local = "2026-07-01T09:05";
    const off = -new Date(`${local}:00`).getTimezoneOffset();
    const sign = off >= 0 ? "+" : "-";
    const hh = String(Math.trunc(Math.abs(off) / 60)).padStart(2, "0");
    const mm = String(Math.abs(off) % 60).padStart(2, "0");
    expect(toRfc3339(local)).toBe(`${local}:00${sign}${hh}:${mm}`);
  });

  it("пустое или некорректное значение — ошибка, а не NaN", () => {
    for (const bad of ["", "abc", "2026-10-25", "2026-10-25T", "2026-10-25T14", "2026-13-01T10:00", "2026-02-31T10:00", "2026-10-25T24:00", "2026-10-25T14:60", "2026-10-25T14:30:61", "2026-10-25T14:30:00.500"]) {
      expect(() => toRfc3339(bad, 180), bad).toThrow(/Invalid local date-time/);
      expect(() => toRfc3339(bad), bad).toThrow(/Invalid local date-time/);
    }
  });

  it("високосный день и граничные значения принимаются", () => {
    expect(toRfc3339("2028-02-29T23:59:59", 0)).toBe("2028-02-29T23:59:59+00:00");
    expect(toRfc3339("2026-01-01T00:00", 60)).toBe("2026-01-01T00:00:00+01:00");
  });
});

describe("время расписания по умолчанию", () => {
  // Местное время машины: момент собирается из полей Date, а не из строки с часовым поясом.
  const at = (h: number, m: number, s = 0) => new Date(2026, 9, 25, h, m, s).getTime();

  it("через 15 минут, округлённое вверх до 5 минут", () => {
    expect(defaultScheduleLocal(at(21, 57, 55))).toBe("2026-10-25T22:15");
    expect(defaultScheduleLocal(at(22, 0))).toBe("2026-10-25T22:15");
    expect(defaultScheduleLocal(at(23, 50))).toBe("2026-10-26T00:05");
  });

  it("значение поля по местным часам", () => {
    expect(toLocalInput(at(9, 5))).toBe("2026-10-25T09:05");
  });

  it("секунд до срока; пустое и неразборчивое — null", () => {
    expect(secondsUntil("2026-10-25T22:00", at(21, 59))).toBe(60);
    expect(secondsUntil("2026-10-25T21:58", at(21, 59))).toBe(-60);
    expect(secondsUntil("", at(21, 59))).toBeNull();
    expect(secondsUntil("завтра", at(21, 59))).toBeNull();
  });
});
