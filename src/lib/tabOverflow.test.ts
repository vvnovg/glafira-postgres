import { describe, expect, it } from "vitest";
import { hiddenTabIds, revealScrollLeft } from "./tabOverflow";

describe("hiddenTabIds", () => {
  const strip = { left: 100, right: 400 };

  it("скрыта вкладка, не видная целиком: за правым и за левым краем строки", () => {
    const tabs = [
      { id: "a", left: 60, right: 140 },
      { id: "b", left: 140, right: 300 },
      { id: "c", left: 300, right: 400 },
      { id: "d", left: 400, right: 520 },
      { id: "e", left: 380, right: 460 },
    ];
    expect(hiddenTabIds(strip, tabs)).toEqual(["a", "d", "e"]);
  });

  it("дробные пиксели на краю вкладку не скрывают", () => {
    expect(hiddenTabIds(strip, [{ id: "a", left: 99.6, right: 400.4 }])).toEqual([]);
  });

  it("строка без ширины (ещё не разложена) — скрытых нет", () => {
    expect(hiddenTabIds({ left: 0, right: 0 }, [{ id: "a", left: 0, right: 80 }])).toEqual([]);
  });
});

describe("revealScrollLeft", () => {
  const strip = { left: 100, right: 400 };

  it("вкладка за правым краем — прокрутка вправо до её края с отступом", () => {
    expect(revealScrollLeft(strip, { left: 380, right: 480 }, 50)).toBe(50 + 80 + 8);
  });

  it("вкладка за левым краем — прокрутка влево до её начала с отступом, не меньше нуля", () => {
    expect(revealScrollLeft(strip, { left: 60, right: 140 }, 200)).toBe(200 - 40 - 8);
    expect(revealScrollLeft(strip, { left: 60, right: 140 }, 20)).toBe(0);
  });

  it("видная целиком вкладка прокрутку не меняет; шире строки — выравнивается по левому краю", () => {
    expect(revealScrollLeft(strip, { left: 120, right: 300 }, 30)).toBe(30);
    expect(revealScrollLeft(strip, { left: 380, right: 800 }, 0)).toBe(380 - 100 - 8);
  });

  it("строка без ширины — без изменений", () => {
    expect(revealScrollLeft({ left: 0, right: 0 }, { left: 10, right: 50 }, 5)).toBe(5);
  });
});
