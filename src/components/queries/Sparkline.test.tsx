import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Sparkline } from "./Sparkline";

const path = () => screen.getByRole("img", { name: "ряд" }).querySelector("path")?.getAttribute("d") ?? "";
/** Координаты y точек пути по порядку. */
const ys = () => Array.from(path().matchAll(/[ML]([\d.]+),([\d.]+)/g)).map((m) => Number(m[2]));

describe("Sparkline", () => {
  it("масштаб — по наибольшему значению, и когда оно меньше единицы", () => {
    render(<Sparkline label="ряд" points={[{ atMs: 0, value: 0.1 }, { atMs: 60_000, value: 0.3 }]} />);
    // Наибольшее значение — у верхнего края (y = 1), а не на трети высоты.
    expect(ys()).toEqual([26.3, 1]);
  });

  it("все нули — линия по нижнему краю, без деления на ноль", () => {
    render(<Sparkline label="ряд" points={[{ atMs: 0, value: 0 }, { atMs: 60_000, value: 0 }]} />);
    expect(ys()).toEqual([39, 39]);
    expect(path()).not.toContain("NaN");
  });

  it("промежуток сэмплера рвёт линию: точки по разные стороны не соединяются", () => {
    const points = [0, 60_000, 300_000, 360_000].map((atMs) => ({ atMs, value: 1 }));
    render(<Sparkline label="ряд" points={points} gaps={[{ fromMs: 120_000, toMs: 300_000 }]} />);
    expect(path().match(/M/g)).toHaveLength(2);
    expect(path()).toMatch(/^M[\d.,]+ L[\d.,]+ M[\d.,]+ L[\d.,]+$/);
  });

  it("одиночная точка между промежутками рисуется точкой", () => {
    const points = [0, 120_000, 240_000].map((atMs) => ({ atMs, value: 1 }));
    render(<Sparkline label="ряд" points={points} gaps={[{ fromMs: 60_000, toMs: 120_000 }, { fromMs: 180_000, toMs: 240_000 }]} />);
    expect(path().split("M").filter(Boolean).every((seg) => seg.trim().endsWith("h0"))).toBe(true);
  });
});
