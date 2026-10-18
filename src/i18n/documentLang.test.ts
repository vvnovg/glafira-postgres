import { afterEach, describe, expect, it } from "vitest";
import { setLanguage } from "./index";

describe("setLanguage и <html lang>", () => {
  afterEach(async () => {
    await setLanguage("ru");
  });

  it("lang следует за действующим языком, x-pseudo → en, неизвестный → en", async () => {
    await setLanguage("en");
    expect(document.documentElement.lang).toBe("en");
    await setLanguage("ru");
    expect(document.documentElement.lang).toBe("ru");
    await setLanguage("x-pseudo");
    expect(document.documentElement.lang).toBe("en");
    await setLanguage("zz");
    expect(document.documentElement.lang).toBe("en");
  });
});
