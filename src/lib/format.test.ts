import { afterEach, describe, expect, it } from "vitest";
import { i18n } from "../i18n";
import { formatAas, formatBytes, formatCount, formatDuration, formatInt64, formatPercent, formatScheduled, formatTimestamp } from "./format";

describe("format", () => {
  it("timestamp в UTC с миллисекундами", () => {
    expect(formatTimestamp(Date.UTC(2026, 0, 2, 3, 4, 5, 6), true)).toBe("2026-01-02 03:04:05.006");
    expect(formatTimestamp(null)).toBe("—");
  });

  it("bytes", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2,0 KB");
    expect(formatBytes(3 * 1024 * 1024)).toBe("3,0 MB");
  });

  it("гигабайты и терабайты", () => {
    expect(formatBytes(3 * 1024 ** 3)).toMatch(/^3[.,]0 GB$/);
    expect(formatBytes(2 * 1024 ** 4)).toMatch(/^2[.,]0 TB$/);
  });

  it("границы единиц: округление до 1024 переводит в следующую единицу", () => {
    const re = (n: number, unit: string) => new RegExp(`^${n}[.,]0 ${unit}$`);
    expect(formatBytes(1023)).toBe("1023 B");
    expect(formatBytes(1024)).toMatch(re(1, "KB"));
    // KB -> MB
    expect(formatBytes(1024 ** 2 - 1)).toMatch(re(1, "MB"));
    expect(formatBytes(1024 ** 2)).toMatch(re(1, "MB"));
    // MB -> GB
    expect(formatBytes(1024 ** 3 - 1)).toMatch(re(1, "GB"));
    expect(formatBytes(1024 ** 3)).toMatch(re(1, "GB"));
    // GB -> TB
    expect(formatBytes(1024 ** 4 - 1)).toMatch(re(1, "TB"));
    expect(formatBytes(1024 ** 4)).toMatch(re(1, "TB"));
  });

  it("у границы единица меняется только когда округление доходит до 1024", () => {
    // 1048524 B = 1023,949 KB — ещё KB; 1048525 B = 1023,950 KB — округляется до 1024,0, значит MB.
    expect(formatBytes(1048524)).toMatch(/^1[\s,]?023[.,]9 KB$/);
    expect(formatBytes(1048525)).toMatch(/^1[.,]0 MB$/);
  });

  it("терабайты — последняя единица: дальше растёт число", () => {
    expect(formatBytes(1024 ** 5)).toMatch(/^1[\s,]?024[.,]0 TB$/);
  });

  it("count: загрузка и недоступно", () => {
    expect(formatCount(undefined)).toBe("…");
    expect(formatCount(null)).toBe("—");
  });

  it("count: знаки после запятой задаются вторым аргументом (TPS — один знак), по умолчанию целое", () => {
    expect(formatCount(1234.5, 1)).toBe("1 234,5".replace(/ /g, "\u00a0"));
    expect(formatCount(3, 1)).toBe("3,0");
    expect(formatCount(1234.56)).toBe("1 235".replace(/ /g, "\u00a0"));
    expect(formatCount(null, 1)).toBe("—");
    expect(formatCount(undefined, 1)).toBe("…");
  });

  it("percent: один знак после запятой, прочерк для null", () => {
    expect(formatPercent(49.494949494949495)).toBe("49,5 %");
    expect(formatPercent(0)).toBe("0 %");
    expect(formatPercent(100)).toBe("100 %");
    expect(formatPercent(null)).toBe("—");
  });

  it("AAS: целое без дробной части, один знак, у малых значений два", () => {
    expect(formatAas(0)).toBe("0");
    expect(formatAas(1)).toBe("1");
    expect(formatAas(1.4)).toBe("1,4");
    expect(formatAas(12.34)).toBe("12,3");
    expect(formatAas(0.04)).toBe("0,04");
    expect(formatAas(0.1)).toBe("0,1");
  });

  it("int64: границы bigint не округляются вверх до 2^63", () => {
    // i64::MAX из JSON приходит числом 2^63 (9223372036854775808): показывается настоящая граница.
    expect(formatInt64(2 ** 63)).toBe("9 223 372 036 854 775 807".replace(/ /g, "\u00a0"));
    // Знак минуса зависит от версии ICU (U+002D или U+2212).
    expect(formatInt64(-(2 ** 63))).toMatch(new RegExp(`^[-\u2212]${"9 223 372 036 854 775 808".replace(/ /g, "\u00a0")}$`));
    expect(formatInt64(2147483647)).toBe("2 147 483 647".replace(/ /g, "\u00a0"));
    expect(formatInt64(-1)).toMatch(/^[-\u2212]1$/);
    expect(formatInt64(null)).toBe("—");
  });
});

describe("форматы по языку", () => {
  afterEach(() => i18n.changeLanguage("ru"));
  it("числа и размеры", async () => {
    await i18n.changeLanguage("en");
    expect(formatCount(1234567)).toBe("1,234,567");
    expect(formatBytes(1536)).toBe("1.5 KB");
    await i18n.changeLanguage("ru");
    expect(formatCount(1234567)).toBe("1 234 567".replace(/ /g, "\u00a0"));
    expect(formatBytes(1536)).toBe("1,5 KB");
  });

  it("проценты и int64 по языку", async () => {
    await i18n.changeLanguage("en");
    expect(formatPercent(49.5)).toBe("49.5 %");
    expect(formatInt64(1234567)).toBe("1,234,567");
    expect(formatInt64(2 ** 63)).toBe("9,223,372,036,854,775,807");
  });

  it("длительность — старшая единица и только смежная за ней", async () => {
    await i18n.changeLanguage("ru");
    expect([formatDuration(null), formatDuration(0.4), formatDuration(42), formatDuration(3725), formatDuration(90061)]).toEqual(["—", "0 с", "42 с", "1 ч 2 мин", "1 д 1 ч"]);
    await i18n.changeLanguage("en");
    expect(formatDuration(125)).toBe("2 min 5 s");
  });

  it("длительность: нулевая смежная единица опускается, несмежные не показываются", async () => {
    await i18n.changeLanguage("ru");
    // 1 д 5 с — это «1 д»: секунды не смежны с днями, а часы нулевые.
    expect([formatDuration(86_405), formatDuration(3_605), formatDuration(3_600), formatDuration(86_400)]).toEqual(["1 д", "1 ч", "1 ч", "1 д"]);
    expect([formatDuration(60), formatDuration(65), formatDuration(59), formatDuration(3_599), formatDuration(172_800 + 3_600 * 23)]).toEqual([
      "1 мин",
      "1 мин 5 с",
      "59 с",
      "59 мин 59 с",
      "2 д 23 ч",
    ]);
    await i18n.changeLanguage("en");
    expect(formatDuration(86_405)).toBe("1 d");
  });

  it("длительность: отрицательное значение сервера (−0,001 с) показывается как 0", async () => {
    await i18n.changeLanguage("ru");
    expect([formatDuration(-0.001), formatDuration(-5), formatDuration(-86_400)]).toEqual(["0 с", "0 с", "0 с"]);
  });

  it("запланированное время: ISO 8601 со смещением — местным временем тем же форматом, что и метки; неразборчивое — как есть", () => {
    expect(formatScheduled("2026-10-25T02:00:00+00:00")).toBe(formatTimestamp(Date.parse("2026-10-25T02:00:00+00:00")));
    expect(formatScheduled("2026-10-25T05:00:00+03:00")).toBe(formatScheduled("2026-10-25T02:00:00Z"));
    expect(formatScheduled("завтра")).toBe("завтра");
    expect(formatScheduled("")).toBe("");
  });
});
