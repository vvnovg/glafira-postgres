import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { PlanNode, PlanTree } from "../../../api/types";
import { formatCount, formatMs, formatPercent } from "../../../lib/format";
import { estimateLabel, estimateSeverity, flatten, marks, nodeLabel } from "./planText";
import { Icon } from "../../common/Icon";

/** Полоса доли собственного времени, ширина — проценты; процент доступен и вспомогательным технологиям (картинка с подписью). */
function ShareBar({ pct }: { pct: number | null }) {
  const { t } = useTranslation();
  if (pct === null) return null;
  const label = t("diag.plan.share", { pct: formatPercent(pct) });
  return (
    <span className="plan-share" role="img" aria-label={label} title={label}>
      <span style={{ width: `${Math.min(100, Math.max(0, pct))}%` }} />
    </span>
  );
}

/** Свёрнутые узлы одного плана: номера узлов привязаны к плану, поэтому состояние помнит, к какому плану относится. */
interface Collapsed {
  tree: PlanTree;
  ids: ReadonlySet<number>;
}
const NONE: ReadonlySet<number> = new Set();

/** Номера предков узла `id` (от корня), `null` — узла в дереве нет. */
function ancestors(n: PlanNode, id: number): number[] | null {
  if (n.id === id) return [];
  for (const c of n.children) {
    const path = ancestors(c, id);
    if (path) return [n.id, ...path];
  }
  return null;
}

/**
 * Дерево плана (спец. P3 §8.6): таблица-дерево с отступами и развёрткой; колонки — узел, время (включённое и
 * собственное с полосой доли), строки (оценка → факт, ошибка ×N подсвечена), циклы, буферы, пометки. Горячие узлы
 * выделены классом `plan-hot`; клик по узлу выбирает его для панели деталей. Без ANALYZE вместо времени — стоимость;
 * при `TIMING OFF` (ANALYZE без времён) в колонке времени прочерк, горячие узлы определены по стоимости.
 * `reveal` — новый объект на каждый переход к узлу извне («Показать узел» рекомендации): свёрнутые предки выбранного узла
 * и узлов `related` раскрываются, строка выбранного прокручивается в видимую часть, фокус — на её названии. `related` —
 * узлы рекомендации: класс `plan-related` и метка (смысл не держится на одном цвете).
 */
export function PlanTreeView({
  tree,
  selectedId,
  onSelect,
  reveal,
  related,
}: {
  tree: PlanTree;
  selectedId: number | null;
  onSelect: (id: number) => void;
  reveal?: object | null;
  related?: ReadonlySet<number>;
}) {
  const { t } = useTranslation();
  // Развёртка сбрасывается при смене плана (номера узлов нового плана — другие): состояние хранит план, к которому относится.
  const [state, setState] = useState<Collapsed>(() => ({ tree, ids: NONE }));
  const collapsed = state.tree === tree ? state.ids : NONE;
  const rows = flatten(tree.root, collapsed);
  const toggle = (id: number) =>
    setState((prev) => {
      const next = new Set(prev.tree === tree ? prev.ids : NONE);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return { tree, ids: next };
    });
  const withActual = tree.root.actual !== null;
  const tableRef = useRef<HTMLTableElement>(null);
  const pendingScroll = useRef<number | null>(null);
  useEffect(() => {
    if (!reveal || selectedId === null) return;
    const own = ancestors(tree.root, selectedId);
    if (!own) return;
    const path = [...own, ...[...(related ?? [])].flatMap((id) => ancestors(tree.root, id) ?? [])];
    pendingScroll.current = selectedId;
    setState((prev) => {
      const ids = prev.tree === tree ? prev.ids : NONE;
      if (!path.some((id) => ids.has(id))) return prev;
      const next = new Set(ids);
      path.forEach((id) => next.delete(id));
      return { tree, ids: next };
    });
  }, [reveal, selectedId, tree, related]);
  // После раскрытия строка уже в DOM: прокрутка и фокус — один раз на переход.
  useEffect(() => {
    const id = pendingScroll.current;
    if (id === null) return;
    const row = tableRef.current?.querySelector<HTMLElement>(`tr[data-node-id="${id}"]`);
    if (!row) return;
    pendingScroll.current = null;
    row.scrollIntoView?.({ block: "nearest" });
    row.querySelector<HTMLButtonElement>("button.link")?.focus({ preventScroll: true });
  });
  return (
    <div className="table-scroll">
      <table ref={tableRef} className="diag-table plan-tree" role="treegrid" aria-label={t("diag.plan.tree")}>
        <thead>
          <tr>
            <th scope="col">{t("diag.plan.columns.node")}</th>
            <th scope="col" className="num">{withActual ? t("diag.plan.columns.time") : t("diag.plan.columns.cost")}</th>
            <th scope="col" className="num">{t("diag.plan.columns.rows")}</th>
            <th scope="col" className="num">{t("diag.plan.columns.loops")}</th>
            <th scope="col" className="num">{t("diag.plan.columns.buffers")}</th>
            <th scope="col">{t("diag.plan.columns.marks")}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map(({ node: n, depth }) => {
            const est = estimateLabel(n);
            const severity = estimateSeverity(n);
            const isCollapsed = collapsed.has(n.id);
            const isRelated = related?.has(n.id) ?? false;
            const classes = [n.hot ? "plan-hot" : "", isRelated ? "plan-related" : "", n.id === selectedId ? "plan-selected" : ""].filter(Boolean).join(" ") || undefined;
            return (
              <tr key={n.id} data-node-id={n.id} className={classes} aria-level={depth + 1} aria-expanded={n.children.length ? !isCollapsed : undefined} aria-selected={n.id === selectedId}>
                <td style={{ paddingLeft: `${8 + depth * 18}px` }}>
                  {/* Стрелка и название — в одной строке-flex: длинное название переносится внутри себя, а не под стрелку. */}
                  <div className="plan-node">
                    {n.children.length > 0 ? (
                      <button type="button" className="icon plan-caret" aria-label={isCollapsed ? t("diag.plan.expand") : t("diag.plan.collapse")} onClick={() => toggle(n.id)}>
                        <span className={isCollapsed ? "tree-chevron" : "tree-chevron open"} aria-hidden="true">
                          <Icon name="chevron" size={14} />
                        </span>
                      </button>
                    ) : (
                      <span className="plan-caret" aria-hidden="true" />
                    )}
                    <button type="button" className="link" onClick={() => onSelect(n.id)}>{nodeLabel(n)}</button>
                    {n.subplanName && <span className="muted">{n.subplanName}</span>}
                  </div>
                </td>
                <td className="num">
                  {/* Не выполнявшийся узел — прочерк, а не «<0,01 мс» и пустая полоса: времени у него нет. */}
                  {withActual ? (
                    !n.neverExecuted && n.inclusiveMs !== null && n.exclusiveMs !== null ? (
                      <>
                        <span title={t("diag.plan.inclusive")}>{formatMs(n.inclusiveMs)}</span>
                        <span className="muted"> / {formatMs(n.exclusiveMs)}</span>
                        <ShareBar pct={n.sharePct} />
                      </>
                    ) : (
                      "—"
                    )
                  ) : (
                    `${formatCount(n.startupCost, 2)}..${formatCount(n.totalCost, 2)}`
                  )}
                </td>
                <td className="num">
                  {formatCount(n.planRows)}
                  {n.actual && <> → {formatCount(n.actual.rows)}</>}
                  {est && <> <span className={severity === "severe" ? "plan-misestimate plan-misestimate-severe" : "plan-misestimate"}>{est}</span></>}
                </td>
                <td className="num">{n.actual ? formatCount(n.actual.loops) : "—"}</td>
                <td className="num">
                  {n.buffers ? `${formatCount(n.buffers.sharedHit)} / ${formatCount(n.buffers.sharedRead)}` : "—"}
                  {n.buffers && n.buffers.tempWritten > 0 && <span className="muted"> temp {formatCount(n.buffers.tempWritten)}</span>}
                </td>
                <td>
                  {isRelated && <span className="kind-badge plan-related-mark">{t("recommend.related")}</span>}
                  {marks(n).map((m) => (
                    <span key={m} className="kind-badge">{t(`diag.plan.marks.${m}`)}</span>
                  ))}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
