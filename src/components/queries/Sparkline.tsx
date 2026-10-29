const W = 240;
const H = 40;

/** Спарклайн SVG по точкам (значения ≥ 0, по порядку тиков); пустой список — пустая область с подписью. */
export function Sparkline({ points, label }: { points: { atMs: number; value: number }[]; label: string }) {
  if (points.length === 0) return <svg className="sparkline" role="img" aria-label={label} width={W} height={H} />;
  const max = Math.max(...points.map((p) => p.value), 1);
  const x = (i: number) => (points.length === 1 ? W / 2 : (i / (points.length - 1)) * (W - 2) + 1);
  const y = (v: number) => H - 1 - (v / max) * (H - 2);
  const d = points.map((p, i) => `${i === 0 ? "M" : "L"}${x(i).toFixed(1)},${y(p.value).toFixed(1)}`).join(" ");
  return (
    <svg className="sparkline" role="img" aria-label={label} width={W} height={H} viewBox={`0 0 ${W} ${H}`}>
      <title>{label}</title>
      <path d={d} fill="none" stroke="currentColor" strokeWidth="1.5" />
    </svg>
  );
}
