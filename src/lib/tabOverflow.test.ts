import { describe, expect, it } from "vitest";
import { hiddenTabIds } from "./tabOverflow";

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
