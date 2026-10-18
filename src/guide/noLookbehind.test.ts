import { describe, expect, it } from "vitest";

// WebKit на macOS 11–12 без Safari 16.4 не разбирает lookbehind: чанк с ним не загружается целиком.
const sources = import.meta.glob<string>(["../**/*.{ts,tsx}", "!../**/*.test.{ts,tsx}"], {
  query: "?raw",
  import: "default",
  eager: true,
});

describe("regular expressions", () => {
  it("use no lookbehind in app code", () => {
    expect(Object.keys(sources).length).toBeGreaterThan(20);
    const offenders = Object.entries(sources)
      .filter(([, text]) => /\(\?<[=!]/.test(text))
      .map(([path]) => path);
    expect(offenders).toEqual([]);
  });
});
