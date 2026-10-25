import { describe, expect, it } from "vitest";
import { toRfc3339 } from "./schedule";

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
});
