import { afterEach, describe, expect, it } from "vitest";
import { i18n } from "../i18n";
import { formatBytes, formatCount, formatTimestamp } from "./format";

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
});
