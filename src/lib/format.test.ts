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
