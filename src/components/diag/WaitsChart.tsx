import { memo, useId, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { WaitBucket, WaitClass, WaitsData } from "../../api/types";
import { formatAas, formatCount, formatTimestamp } from "../../lib/format";

/**
 * Порядок классов в столбце снизу вверх, в легенде и в таблице: сначала то, что видно чаще всего (CPU*, блокировки),
 * редкие классы — выше. Цвета классов — переменные `--wait-<класс>` в styles.css.
 */
export const WAIT_CLASS_ORDER: WaitClass[] = ["cpu", "lock", "lwlock", "io", "ipc", "client", "timeout", "bufferPin", "extension", "activity", "other"];

/** Подпись класса: ключ выбирается по значению, поэтому — статическими литералами. «CPU*» — со звёздочкой, её объясняет сноска вида. */
export const CLASS_LABEL_KEYS: Record<WaitClass, string> = {
  cpu: "diag.waits.classes.cpu",
  lock: "diag.waits.classes.lock",
  lwlock: "diag.waits.classes.lwlock",
  io: "diag.waits.classes.io",
  ipc: "diag.waits.classes.ipc",
  client: "diag.waits.classes.client",
  timeout: "diag.waits.classes.timeout",
  bufferPin: "diag.waits.classes.bufferPin",
  extension: "diag.waits.classes.extension",
  activity: "diag.waits.classes.activity",
  other: "diag.waits.classes.other",
};

/** Высота рисунка фиксирована; ширина — по контейнеру. */
const HEIGHT = 220;
const MARGIN = { top: 10, right: 12, bottom: 24, left: 44 };
/** Зазор цвета поверхности между столбцами и между сегментами столбца, px. */
const GAP = 2;
/** Столбец не толще 24 px: на широком экране при малом числе интервалов остаётся воздух. */
const MAX_BAR = 24;
/** Ширина до первого замера (и там, где замера нет — в тестах без раскладки). */
const FALLBACK_WIDTH = 720;
const MIN_WIDTH = 240;
/** Подпись оси времени занимает порядка 60 px; между подписями — запас. */
const TICK_SPACING = 96;

/** Шаги «круглого» максимума оси: 1, 1,5, 2, 2,5, 3, 4, 5, 6, 8, 10 × 10^k. */
const NICE = [1, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10];

/** Максимум оси Y: ближайшее «круглое» число не меньше `v`; для пустого окна — 1. */
export function niceMax(v: number): number {
  if (!(v > 0)) return 1;
  const exp = Math.floor(Math.log10(v));
  const base = 10 ** exp;
  const f = v / base;
  // 1e-9: погрешность вычисления не должна поднимать максимум на ступень (v = 2 при f = 2,0000000000000004).
  const step = NICE.find((s) => s >= f - 1e-9) ?? 10;
  return Number((step * base).toPrecision(12));
}

/** Время для подписей и подсказок: «ГГГГ-ММ-ДД ЧЧ:ММ:СС» (миллисекунды границ интервалов — шум). */
const stamp = (ms: number) => formatTimestamp(ms).slice(0, 19);

/** Ширина контейнера: измеряется при монтировании и при изменении размера; без `ResizeObserver` остаётся запасная. */
function useWidth() {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(FALLBACK_WIDTH);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      if (el.clientWidth > 0) setWidth(Math.max(el.clientWidth, MIN_WIDTH));
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, width] as const;
}

/** Значения интервала по классам в порядке `WAIT_CLASS_ORDER`: только ненулевые. */
const segments = (b: WaitBucket): { cls: WaitClass; v: number }[] =>
  WAIT_CLASS_ORDER.flatMap((cls) => {
    const v = b.aas[cls] ?? 0;
    return v > 0 ? [{ cls, v }] : [];
  });

const totalOf = (b: WaitBucket) => segments(b).reduce((sum, s) => sum + s.v, 0);

/**
 * Профиль ожиданий (спец. P2 §7.4): столбцы с накоплением — среднее число активных сессий (AAS) по классам ожиданий в
 * каждом интервале окна. Свой SVG без библиотек. Промежутки, когда сэмплер не работал, затенены штриховкой. Для экранного
 * диктора — таблица тех же чисел; подсказка столбца (`<title>`) — время, всего и классы. Класс `sr-only` — на обёртке
 * таблицы, а не на `<table>`: у таблицы height и overflow не обрезают строки, и невидимая таблица удлиняла вкладку.
 * `memo`: вид перерисовывается каждую секунду (тик сэмплера), а данные графика берутся из отчёта и меняются только
 * с ним — до 90 столбцов на каждый тик перерисовывать незачем.
 */
export const WaitsChart = memo(function WaitsChart({ data }: { data: WaitsData }) {
  const { t } = useTranslation();
  const [wrap, width] = useWidth();
  const patternId = `waits-hatch-${useId().replace(/[^A-Za-z0-9_-]/g, "")}`;

  const { buckets, bucketS } = data;
  const n = buckets.length;
  const plotW = Math.max(width - MARGIN.left - MARGIN.right, 1);
  const plotH = HEIGHT - MARGIN.top - MARGIN.bottom;
  const slot = plotW / Math.max(n, 1);
  const barW = Math.max(Math.min(slot - GAP, MAX_BAR), 1);
  const totals = buckets.map(totalOf);
  const yMax = niceMax(Math.max(0, ...totals));
  const yOf = (v: number) => MARGIN.top + plotH - (v / yMax) * plotH;
  const bottom = MARGIN.top + plotH;
  const digits = [0, 1, 2].find((d) => Math.abs(Number(yMax.toFixed(d)) - yMax) < 1e-9) ?? 2;

  // Классы, которые есть в окне: легенда и таблица не носят пустые столбцы.
  const present = WAIT_CLASS_ORDER.filter((cls) => buckets.some((b) => (b.aas[cls] ?? 0) > 0));
  const label = (cls: WaitClass) => t(CLASS_LABEL_KEYS[cls]);

  // Промежутки: время переводится в долю окна по интервалам; то, что вышло за окно, обрезается, целиком внешние — не рисуются.
  const t0 = n > 0 ? buckets[0].startMs : 0;
  const bucketMs = bucketS * 1000;
  const xOfTime = (ms: number) => MARGIN.left + Math.min(Math.max(((ms - t0) / bucketMs) * slot, 0), plotW);
  const gaps = data.gaps.flatMap((g, i) => {
    if (n === 0) return [];
    const x1 = xOfTime(g.fromMs);
    const x2 = xOfTime(g.toMs);
    return x2 - x1 > 0 ? [{ key: `${g.fromMs}:${g.toMs}:${i}`, x: x1, w: x2 - x1 }] : [];
  });

  const maxTicks = Math.max(2, Math.floor(plotW / TICK_SPACING));
  const tickStep = Math.max(1, Math.ceil(n / maxTicks));
  const ticks: { i: number; x: number; text: string }[] = [];
  for (let i = 0; i < n; i += tickStep) {
    const x = MARGIN.left + i * slot;
    // Подпись, которая не помещается у правого края, не рисуется.
    if (x + 30 <= width) ticks.push({ i, x, text: stamp(buckets[i].startMs).slice(11) });
  }

  const tooltip = (b: WaitBucket, segs: { cls: WaitClass; v: number }[]) => {
    const time = stamp(b.startMs);
    if (b.sampledS <= 0) return t("diag.waits.tooltipEmpty", { time });
    return [t("diag.waits.tooltip", { time, total: formatAas(totalOf(b)) }), ...segs.map((s) => `${label(s.cls)}: ${formatAas(s.v)}`)].join("\n");
  };

  return (
    <div className="waits-chart">
      <div ref={wrap} className="waits-plot">
        <svg role="img" aria-label={t("diag.waits.chartLabel")} viewBox={`0 0 ${width} ${HEIGHT}`} width={width} height={HEIGHT}>
          <defs>
            <pattern id={patternId} width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
              <line className="waits-hatch" x1="0" y1="0" x2="0" y2="6" />
            </pattern>
          </defs>
          <line className="waits-grid" x1={MARGIN.left} x2={MARGIN.left + plotW} y1={yOf(yMax)} y2={yOf(yMax)} />
          <line className="waits-axis" x1={MARGIN.left} x2={MARGIN.left + plotW} y1={bottom} y2={bottom} />
          <text className="waits-ytick" x={MARGIN.left - 6} y={bottom} textAnchor="end" dominantBaseline="middle">
            {formatCount(0)}
          </text>
          <text className="waits-ytick" x={MARGIN.left - 6} y={yOf(yMax)} textAnchor="end" dominantBaseline="middle">
            {formatCount(yMax, digits)}
          </text>
          {ticks.map((tk) => (
            <g key={tk.i}>
              <line className="waits-axis" x1={tk.x} x2={tk.x} y1={bottom} y2={bottom + 4} />
              <text className="waits-xtick" x={tk.x} y={bottom + 16} textAnchor="middle">
                {tk.text}
              </text>
            </g>
          ))}
          {gaps.map((g) => (
            <rect key={g.key} className="gap" x={g.x} y={MARGIN.top} width={g.w} height={plotH} fill={`url(#${patternId})`} pointerEvents="none" />
          ))}
          {buckets.map((b, i) => {
            const segs = segments(b);
            const slotX = MARGIN.left + i * slot;
            const x = slotX + (slot - barW) / 2;
            let acc = 0;
            return (
              <g key={b.startMs} className="waits-col">
                <title>{tooltip(b, segs)}</title>
                {segs.map((s) => {
                  const y = yOf(acc + s.v);
                  // Нижний сегмент стоит на оси; остальные уступают GAP, и между цветами остаётся цвет поверхности.
                  const h = Math.max(yOf(acc) - y - (acc > 0 ? GAP : 0), 1);
                  acc += s.v;
                  return <rect key={s.cls} data-class={s.cls} x={x} y={y} width={barW} height={h} style={{ fill: `var(--wait-${s.cls})` }} />;
                })}
                {/* Цель наведения — весь интервал на всю высоту: тонкий столбец и пустой интервал тоже показывают подсказку. */}
                <rect className="waits-hit" x={slotX} y={MARGIN.top} width={slot} height={plotH} />
              </g>
            );
          })}
        </svg>
      </div>
      {present.length > 0 && (
        <ul className="waits-legend" aria-label={t("diag.waits.legendLabel")}>
          {present.map((cls) => (
            <li key={cls}>
              <span className="waits-swatch" data-class={cls} aria-hidden="true" />
              {label(cls)}
            </li>
          ))}
        </ul>
      )}
      <div className="sr-only">
        <table aria-label={t("diag.waits.tableLabel")}>
          <thead>
            <tr>
              <th scope="col">{t("diag.waits.columns.interval")}</th>
              <th scope="col">{t("diag.waits.columns.total")}</th>
              {present.map((cls) => (
                <th key={cls} scope="col">
                  {label(cls)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {buckets.map((b) => (
              <tr key={b.startMs}>
                <th scope="row">{stamp(b.startMs)}</th>
                {b.sampledS <= 0 ? (
                  <td colSpan={1 + present.length}>{t("diag.waits.noData")}</td>
                ) : (
                  <>
                    <td>{formatAas(totalOf(b))}</td>
                    {present.map((cls) => (
                      <td key={cls}>{formatAas(b.aas[cls] ?? 0)}</td>
                    ))}
                  </>
                )}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
});
