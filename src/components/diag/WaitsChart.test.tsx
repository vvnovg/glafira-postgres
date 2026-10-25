import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { WaitBucket } from "../../api/types";
import { waitsData } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";
import { niceMax, WAIT_CLASS_ORDER, WaitsChart } from "./WaitsChart";

// Местное время: подписи форматируются по часовому поясу машины.
const T0 = new Date(2026, 9, 25, 12, 0, 0).getTime();
const bucket = (i: number, aas: WaitBucket["aas"], sampledS = 5): WaitBucket => ({ startMs: T0 + i * 5000, sampledS, aas });
const chart = () => screen.getByRole("img", { name: "Среднее число активных сессий по классам ожиданий" });

describe("WaitsChart", () => {
  it("столбцы с накоплением по классам и таблица для экранного диктора", () => {
    const data = waitsData({ bucketS: 5, buckets: [
      { startMs: 0, sampledS: 5, aas: { cpu: 1, lock: 0.4 } },
      { startMs: 5000, sampledS: 0, aas: {} },
    ], gaps: [{ fromMs: 5000, toMs: 10000 }] });
    renderWithClient(<WaitsChart data={data} />);
    const svg = screen.getByRole("img", { name: "Среднее число активных сессий по классам ожиданий" });
    expect(svg.querySelectorAll("rect[data-class='lock']")).toHaveLength(1);
    expect(svg.querySelectorAll(".gap")).toHaveLength(1);
    const table = screen.getByRole("table", { name: "Профиль ожиданий по интервалам" });
    expect(within(table).getAllByRole("row")[1]).toHaveTextContent("1,4");
  });

  it("порядок классов — как в брифе; сегмент столбца ложится в порядке классов снизу вверх", () => {
    expect(WAIT_CLASS_ORDER).toEqual(["cpu", "lock", "lwlock", "io", "ipc", "client", "timeout", "bufferPin", "extension", "activity", "other"]);
    renderWithClient(<WaitsChart data={waitsData({ bucketS: 5, buckets: [bucket(0, { other: 0.5, io: 1, cpu: 2 })] })} />);
    const segs = [...chart().querySelectorAll("rect[data-class]")];
    // В DOM — снизу вверх: у более раннего класса `y` больше (SVG растёт вниз).
    expect(segs.map((r) => r.getAttribute("data-class"))).toEqual(["cpu", "io", "other"]);
    const ys = segs.map((r) => Number(r.getAttribute("y")));
    expect(ys[0]).toBeGreaterThan(ys[1]);
    expect(ys[1]).toBeGreaterThan(ys[2]);
  });

  it("класс с нулём в интервале прямоугольника не даёт", () => {
    renderWithClient(<WaitsChart data={waitsData({ bucketS: 5, buckets: [bucket(0, { cpu: 1, lock: 0 }), bucket(1, { cpu: 0.5 })] })} />);
    expect(chart().querySelectorAll("rect[data-class='cpu']")).toHaveLength(2);
    expect(chart().querySelectorAll("rect[data-class='lock']")).toHaveLength(0);
  });

  it("цвет столбца — переменная --wait-<класс> (светлая и тёмная тема меняют её в одном месте)", () => {
    renderWithClient(<WaitsChart data={waitsData({ bucketS: 5, buckets: [bucket(0, { lock: 1 })] })} />);
    const rect = chart().querySelector("rect[data-class='lock']") as SVGRectElement;
    expect(rect.style.fill).toBe("var(--wait-lock)");
  });

  it("подсказка столбца: время, всего и классы со значениями; интервал без сэмплов — «нет данных»", () => {
    renderWithClient(<WaitsChart data={waitsData({ bucketS: 5, buckets: [bucket(0, { cpu: 1, lock: 0.4 }), bucket(1, {}, 0)] })} />);
    const cols = chart().querySelectorAll("g.waits-col");
    expect(cols).toHaveLength(2);
    const first = cols[0].querySelector("title")?.textContent ?? "";
    expect(first).toContain("12:00:00");
    expect(first).toContain("1,4");
    expect(first).toContain("CPU*: 1");
    expect(first).toContain("Lock: 0,4");
    expect(cols[1].querySelector("title")?.textContent).toContain("нет данных");
  });

  it("ось Y: подписи «0» и округлённый максимум", () => {
    renderWithClient(<WaitsChart data={waitsData({ bucketS: 5, buckets: [bucket(0, { cpu: 1, lock: 0.4 })] })} />);
    const labels = [...chart().querySelectorAll("text.waits-ytick")].map((n) => n.textContent);
    expect(labels).toEqual(["0", "1,5"]);
  });

  it("круглый максимум оси: не меньше данных, из ряда 1 · 1,5 · 2 · 2,5 · 3 · 4 · 5 · 6 · 8 · 10", () => {
    expect([0, 0.04, 0.3, 1, 1.4, 2, 2.1, 3.5, 7, 12, 95].map(niceMax)).toEqual([1, 0.04, 0.3, 1, 1.5, 2, 2.5, 4, 8, 15, 100]);
  });

  it("легенда — только классы, которые есть в окне, с «CPU*» для cpu; образцы цвета скрыты от диктора", () => {
    renderWithClient(<WaitsChart data={waitsData({ bucketS: 5, buckets: [bucket(0, { cpu: 1, lock: 0.4 })] })} />);
    const legend = screen.getByRole("list", { name: "Классы ожиданий" });
    expect(within(legend).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["CPU*", "Lock"]);
    expect(legend.querySelectorAll("[aria-hidden='true']")).toHaveLength(2);
  });

  it("пустое окно: график и таблица есть, столбцов нет", () => {
    renderWithClient(<WaitsChart data={waitsData({ bucketS: 5, buckets: [bucket(0, {}, 0), bucket(1, {}, 0)] })} />);
    expect(chart().querySelectorAll("rect[data-class]")).toHaveLength(0);
    const rows = within(screen.getByRole("table", { name: "Профиль ожиданий по интервалам" })).getAllByRole("row");
    expect(rows).toHaveLength(3);
    expect(rows[1]).toHaveTextContent("нет данных");
  });

  it("промежуток в истории: штриховка занимает его долю по времени и не принимает мышь", () => {
    const buckets = [bucket(0, { cpu: 1 }), bucket(1, {}, 0), bucket(2, {}, 0), bucket(3, { cpu: 1 })];
    renderWithClient(<WaitsChart data={waitsData({ bucketS: 5, buckets, gaps: [{ fromMs: T0 + 5000, toMs: T0 + 15000 }] })} />);
    const gap = chart().querySelector("rect.gap") as SVGRectElement;
    const col = chart().querySelectorAll("g.waits-col rect.waits-hit");
    const slot = Number(col[0].getAttribute("width"));
    expect(Number(gap.getAttribute("x"))).toBeCloseTo(Number(col[1].getAttribute("x")), 1);
    expect(Number(gap.getAttribute("width"))).toBeCloseTo(slot * 2, 1);
    expect(gap.getAttribute("pointer-events")).toBe("none");
  });

  it("промежуток вне окна обрезается по краям", () => {
    const buckets = [bucket(0, { cpu: 1 }), bucket(1, { cpu: 1 })];
    renderWithClient(<WaitsChart data={waitsData({ bucketS: 5, buckets, gaps: [{ fromMs: T0 - 60_000, toMs: T0 + 2500 }, { fromMs: T0 + 100_000, toMs: T0 + 200_000 }] })} />);
    // Второй промежуток целиком правее окна: рисуется только первый, и он не выходит влево за поле построения.
    const gaps = chart().querySelectorAll("rect.gap");
    expect(gaps).toHaveLength(1);
    const hit0 = chart().querySelector("g.waits-col rect.waits-hit") as SVGRectElement;
    expect(Number(gaps[0].getAttribute("x"))).toBeCloseTo(Number(hit0.getAttribute("x")), 1);
  });
});
