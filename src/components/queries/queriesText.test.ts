import { describe, expect, it } from "vitest";
import { i18n } from "../../i18n";
import { changeRatioText, snapshotFileParts } from "./queriesText";

describe("snapshotFileParts", () => {
  it("раскодирует каталог узла и отделяет время", () => {
    expect(snapshotFileParts("db1%3A5432/2025-10-29T11-00-00.json")).toEqual({ member: "db1:5432", stamp: "2025-10-29T11-00-00" });
    expect(snapshotFileParts("%5B%3A%3A1%5D%3A5432/x.json")).toEqual({ member: "[::1]:5432", stamp: "x" });
    expect(snapshotFileParts("my node/x.json")).toEqual({ member: "my node", stamp: "x" });
  });

  it("старый каталог и не код — как есть", () => {
    expect(snapshotFileParts("db1:5432/x.json")).toEqual({ member: "db1:5432", stamp: "x" });
    expect(snapshotFileParts("50%zz/x.json")).toEqual({ member: "50%zz", stamp: "x" });
    expect(snapshotFileParts("x.json")).toEqual({ member: "", stamp: "x" });
  });
});

describe("changeRatioText", () => {
  const t = i18n.t.bind(i18n);

  it("замедление — «в N раза медленнее», ускорение — обратным коэффициентом «быстрее»", () => {
    expect(changeRatioText(3, t)).toBe("в 3 раза медленнее");
    expect(changeRatioText(0.4, t)).toBe("в 2,5 раза быстрее");
    expect(changeRatioText(0.2, t)).toBe("в 5 раз быстрее");
    expect(changeRatioText(21, t)).toBe("в 21 раз медленнее");
    expect(changeRatioText(1.5, t)).toBe("в 1,5 раза медленнее");
  });

  it("неопределённое отношение — прочерк", () => {
    expect(changeRatioText(0, t)).toBe("—");
    expect(changeRatioText(Number.POSITIVE_INFINITY, t)).toBe("—");
    expect(changeRatioText(Number.NaN, t)).toBe("—");
  });

  it("по-английски — «2.5× faster»", async () => {
    await i18n.changeLanguage("en");
    try {
      expect(changeRatioText(0.4, t)).toBe("2.5× faster");
      expect(changeRatioText(3, t)).toBe("3× slower");
    } finally {
      await i18n.changeLanguage("ru");
    }
  });
});
