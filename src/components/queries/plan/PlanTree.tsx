import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { PlanTree } from "../../../api/types";
import { formatCount, formatMs } from "../../../lib/format";
import { estimateLabel, flatten, marks, nodeLabel } from "./planText";

/** Полоса доли собственного времени, ширина — проценты. */
function ShareBar({ pct }: { pct: number | null }) {
  if (pct === null) return null;
  return (
    <span className="plan-share" aria-hidden="true">
      <span style={{ width: `${Math.min(100, Math.max(0, pct))}%` }} />
    </span>
  );
}

/**
 * Дерево плана (спец. P3 §8.6): таблица-дерево с отступами и развёрткой; колонки — узел, время (включённое и
 * собственное с полосой доли), строки (оценка → факт, ошибка ×N подсвечена), циклы, буферы, пометки. Горячие узлы
 * выделены классом `plan-hot`; клик по узлу выбирает его для панели деталей. Без ANALYZE вместо времени — стоимость;
 * при `TIMING OFF` (ANALYZE без времён) в колонке времени прочерк, горячие узлы определены по стоимости.
 */
export function PlanTreeView({ tree, selectedId, onSelect }: { tree: PlanTree; selectedId: number | null; onSelect: (id: number) => void }) {
  const { t } = useTranslation();
  const [collapsed, setCollapsed] = useState<ReadonlySet<number>>(new Set());
  const rows = flatten(tree.root, collapsed);
  const toggle = (id: number) =>
    setCollapsed((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const withActual = tree.root.actual !== null;
  return (
    <div className="table-scroll">
      <table className="diag-table plan-tree" role="treegrid" aria-label={t("diag.plan.tree")}>
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
            const isCollapsed = collapsed.has(n.id);
            const classes = [n.hot ? "plan-hot" : "", n.id === selectedId ? "plan-selected" : ""].filter(Boolean).join(" ") || undefined;
            return (
              <tr key={n.id} className={classes} aria-level={depth + 1} aria-expanded={n.children.length ? !isCollapsed : undefined} aria-selected={n.id === selectedId}>
                <td style={{ paddingLeft: `${8 + depth * 18}px` }}>
                  {/* Стрелка и название — в одной строке-flex: длинное название переносится внутри себя, а не под стрелку. */}
                  <div className="plan-node">
                    {n.children.length > 0 ? (
                      <button type="button" className="icon plan-caret" aria-label={isCollapsed ? t("diag.plan.expand") : t("diag.plan.collapse")} onClick={() => toggle(n.id)}>
                        <span aria-hidden="true">{isCollapsed ? "▸" : "▾"}</span>
                      </button>
                    ) : (
                      <span className="plan-caret" aria-hidden="true" />
                    )}
                    <button type="button" className="link" onClick={() => onSelect(n.id)}>{nodeLabel(n)}</button>
                    {n.subplanName && <span className="muted">{n.subplanName}</span>}
                  </div>
                </td>
                <td className="num">
                  {withActual ? (
                    n.inclusiveMs !== null && n.exclusiveMs !== null ? (
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
                  {est && <> <span className="plan-misestimate">{est}</span></>}
                </td>
                <td className="num">{n.actual ? formatCount(n.actual.loops) : "—"}</td>
                <td className="num">
                  {n.buffers ? `${formatCount(n.buffers.sharedHit)} / ${formatCount(n.buffers.sharedRead)}` : "—"}
                  {n.buffers && n.buffers.tempWritten > 0 && <span className="muted"> temp {formatCount(n.buffers.tempWritten)}</span>}
                </td>
                <td>
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
