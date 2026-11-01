import { useTranslation } from "react-i18next";
import type { Change, Comparison, QueryStat } from "../../api/types";
import { formatCount, formatMs } from "../../lib/format";
import { QUERY_KEY_ID, queryTitle } from "./queriesText";

/** Ячейка запроса: начало текста (или queryId) и пометка запроса самой Глафиры — такие строки приглушены. */
function QueryCell({ stat }: { stat: QueryStat }) {
  const { t } = useTranslation();
  return (
    <td className="diag-query-cell mono" title={stat.text ?? undefined}>
      {queryTitle(stat.text, stat.key.queryId, 80)}
      {stat.own && (
        <span className="kind-badge" title={t("diag.queries.ownHint")}>
          {t("diag.queries.own")}
        </span>
      )}
    </td>
  );
}

function Plain({ title, rows }: { title: string; rows: QueryStat[] }) {
  const { t } = useTranslation();
  return (
    <section>
      <h4>
        {title} ({rows.length})
      </h4>
      {rows.length === 0 ? (
        <p className="muted">—</p>
      ) : (
        <table className="diag-table" aria-label={title}>
          <thead>
            <tr>
              <th scope="col">{t("diag.queries.columns.query")}</th>
              <th scope="col" className="num">
                {t("diag.queries.columns.calls")}
              </th>
              <th scope="col" className="num">
                {t("diag.queries.columns.total")}
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={QUERY_KEY_ID(r.key)} className={r.own ? "snapshot-own" : undefined}>
                <QueryCell stat={r} />
                <td className="num">{formatCount(r.calls)}</td>
                <td className="num">{formatMs(r.totalExecMs)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

function Changes({ title, rows }: { title: string; rows: Change[] }) {
  const { t } = useTranslation();
  return (
    <section>
      <h4>
        {title} ({rows.length})
      </h4>
      {rows.length === 0 ? (
        <p className="muted">—</p>
      ) : (
        <table className="diag-table" aria-label={title}>
          <thead>
            <tr>
              <th scope="col">{t("diag.queries.columns.query")}</th>
              <th scope="col" className="num">
                {t("diag.snapshots.before")}
              </th>
              <th scope="col" className="num">
                {t("diag.snapshots.after")}
              </th>
              <th scope="col" className="num">
                {t("diag.queries.columns.calls")}
              </th>
              <th scope="col" className="num">
                {t("diag.snapshots.ratio")}
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((c) => {
              const stat = c.after.text !== null ? c.after : c.before;
              return (
                <tr key={QUERY_KEY_ID(c.key)} className={c.after.own || c.before.own ? "snapshot-own" : undefined}>
                  <QueryCell stat={{ ...stat, own: c.after.own || c.before.own }} />
                  <td className="num">{formatMs(c.before.meanExecMs)}</td>
                  <td className="num">{formatMs(c.after.meanExecMs)}</td>
                  <td className="num">{formatCount(c.after.calls)}</td>
                  <td className="num">×{formatCount(c.ratio, 1)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}

/**
 * Итог сравнения двух снимков (спец. P3 §9.3): четыре списка; «Топ между снимками» переводит инструмент в режим дельт.
 * Запросы самой Глафиры Rust не отбрасывает — они приглушены и помечены.
 */
export function SnapshotCompare({ comparison, title, onBetween }: { comparison: Comparison; title: string; onBetween: () => void }) {
  const { t } = useTranslation();
  return (
    <div className="snapshot-compare" role="region" aria-label={title}>
      <div className="toolbar">
        <strong>{title}</strong>
        <span className="muted">
          {t("diag.snapshots.totals", { from: formatCount(comparison.totalsFrom.calls), to: formatCount(comparison.totalsTo.calls) })}
        </span>
        <button type="button" onClick={onBetween}>
          {t("diag.snapshots.between")}
        </button>
      </div>
      {comparison.reset && <p className="diag-note">{t("diag.snapshots.reset")}</p>}
      <Changes title={t("diag.snapshots.degraded")} rows={comparison.degraded} />
      <Changes title={t("diag.snapshots.improved")} rows={comparison.improved} />
      <Plain title={t("diag.snapshots.new")} rows={comparison.new} />
      <Plain title={t("diag.snapshots.gone")} rows={comparison.gone} />
    </div>
  );
}
