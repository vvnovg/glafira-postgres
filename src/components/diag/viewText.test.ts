import { describe, expect, it } from "vitest";
import { bytesText, metaText, uniqueKeys } from "./viewText";

describe("viewText", () => {
  it("bytesText: байты в единицах, null — прочерк", () => {
    expect(bytesText(2 * 1024 ** 3)).toBe("2,0 GB");
    expect(bytesText(0)).toBe("0 B");
    expect(bytesText(null)).toBe("—");
  });

  it("metaText: непустые части через « · »", () => {
    expect(metaText(["app", null, "", "orders", undefined])).toBe("app · orders");
    expect(metaText([null])).toBe("");
  });

  it("uniqueKeys: счётчик только у повторов, порядок остальных не важен", () => {
    expect(uniqueKeys(["a", "b", "a", "a"])).toEqual(["a", "b", "a#1", "a#2"]);
    expect(uniqueKeys(["b", "a"])).toEqual(["b", "a"]);
  });
});
