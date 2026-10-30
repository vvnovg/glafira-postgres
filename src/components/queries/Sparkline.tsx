const W = 240;
const H = 40;

interface Point {
  atMs: number;
  value: number;
}

/** Промежуток, когда сэмплер стоял: точки по разные его стороны не соединяются. */
export interface SparkGap {
  fromMs: number;
  toMs: number;
}

/**
 * Спарклайн SVG по точкам (значения ≥ 0, по порядку тиков); пустой список — пустая область с подписью. Масштаб — по
 * наибольшему значению (и для значений меньше единицы: среднее 0,3 мс занимает всю высоту); все нули — линия по нижнему
 * краю. Промежуток `gaps` между соседними точками рвёт линию; отрезок из одной точки рисуется точкой.
 */
export function Sparkline({ points, gaps = [], label }: { points: Point[]; gaps?: SparkGap[]; label: string }) {
  if (points.length === 0) return <svg className="sparkline" role="img" aria-label={label} width={W} height={H} />;
  const max = Math.max(...points.map((p) => p.value));
  const scale = max > 0 ? max : 1;
  const x = (i: number) => (points.length === 1 ? W / 2 : (i / (points.length - 1)) * (W - 2) + 1);
  const y = (v: number) => H - 1 - (v / scale) * (H - 2);
  const broken = (a: Point, b: Point) => gaps.some((g) => g.fromMs < b.atMs && g.toMs > a.atMs);
  const at = (i: number) => `${x(i).toFixed(1)},${y(points[i].value).toFixed(1)}`;
  let d = "";
  points.forEach((p, i) => {
    const starts = i === 0 || broken(points[i - 1], p);
    const ends = i === points.length - 1 || broken(p, points[i + 1]);
    // Отрезок из одной точки: нулевая длина с круглым концом — точка.
    d += starts ? `${d ? " " : ""}M${at(i)}${ends ? " h0" : ""}` : ` L${at(i)}`;
  });
  return (
    <svg className="sparkline" role="img" aria-label={label} width={W} height={H} viewBox={`0 0 ${W} ${H}`}>
      <title>{label}</title>
      <path d={d} fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}
