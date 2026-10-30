import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { ConnectionKind, Member, QueriesParams, QuerySort, QueryStat } from "../../api/types";
import { formatBytes, formatCount, formatMs, formatPercent, formatTimestamp } from "../../lib/format";
import { type QueriesReport, useQueriesReport } from "../../lib/useQueriesReport";
import { useStatementsSampler } from "../../lib/useStatementsSampler";
import { useWorkspace } from "../../state/workspace";
import { ErrorBanner } from "../common/ErrorBanner";
import { DiagHeader } from "../diag/DiagHeader";
import { FindingsPanel } from "../diag/FindingsPanel";
import { unavailableText } from "../diag/unavailable";
import { QUERY_KEY_ID, queryTitle, sortAvailable } from "./queriesText";

/** Интервал обновления режима «с момента сброса» по умолчанию, с. */
const DEFAULT_INTERVAL_S = 10;
const LIMITS = [50, 100, 200] as const;

interface Col {
  id: string;
  sort: QuerySort | null;
  labelKey: string;
  cell(r: QueryStat): string;
}

const COLS: Col[] = [
  { id: "share", sort: "totalTime", labelKey: "diag.queries.columns.share", cell: (r) => formatPercent(r.sharePct) },
  { id: "total", sort: "totalTime", labelKey: "diag.queries.columns.total", cell: (r) => formatMs(r.totalExecMs) },
  { id: "mean", sort: "meanTime", labelKey: "diag.queries.columns.mean", cell: (r) => formatMs(r.meanExecMs) },
  { id: "calls", sort: "calls", labelKey: "diag.queries.columns.calls", cell: (r) => formatCount(r.calls) },
  { id: "rowsPerCall", sort: "rowsPerCall", labelKey: "diag.queries.columns.rowsPerCall", cell: (r) => formatCount(r.rowsPerCall, 1) },
  { id: "hit", sort: "hitRatio", labelKey: "diag.queries.columns.hitRatio", cell: (r) => (r.hitRatio === null ? "—" : formatPercent(r.hitRatio * 100)) },
  { id: "read", sort: "sharedRead", labelKey: "diag.queries.columns.read", cell: (r) => formatBytes(r.sharedRead * 8192) },
  { id: "temp", sort: "tempWritten", labelKey: "diag.queries.columns.temp", cell: (r) => formatBytes(r.tempWritten * 8192) },
  { id: "wal", sort: "walBytes", labelKey: "diag.queries.columns.wal", cell: (r) => formatBytes(r.walBytes) },
  { id: "io", sort: "ioTime", labelKey: "diag.queries.columns.io", cell: (r) => formatMs(r.ioReadMs + r.ioWriteMs) },
  { id: "plan", sort: "planTime", labelKey: "diag.queries.columns.plan", cell: (r) => formatMs(r.totalPlanMs) },
  { id: "jit", sort: "jitTime", labelKey: "diag.queries.columns.jit", cell: (r) => (r.jitMs === null ? "—" : formatMs(r.jitMs)) },
  { id: "stddev", sort: "stddevTime", labelKey: "diag.queries.columns.stddev", cell: (r) => (r.stddevExecMs === null ? "—" : formatMs(r.stddevExecMs)) },
  { id: "max", sort: "maxTime", labelKey: "diag.queries.columns.max", cell: (r) => (r.maxExecMs === null ? "—" : formatMs(r.maxExecMs)) },
];

const modeValue = (m: QueriesParams["mode"]) => (m.kind === "window" ? `window:${m.s}` : m.kind);
const parseMode = (v: string): QueriesParams["mode"] => (v.startsWith("window:") ? { kind: "window", s: Number(v.slice(7)) as 300 | 900 | 3600 } : { kind: "sinceReset" });

/**
 * Инструмент «Запросы» (спец. P3 §4.7): общая шапка диагностики, режим и фильтры, таблица топа, находки. Отчёт «за
 * окно» обновляется по тику сэмплера pgss, на который вкладка подписана, пока смонтирована; тексты строк подгружаются
 * по ключам один раз на набор строк.
 */
export function QueriesView({
  connectionId,
  member,
  active,
  live,
  members,
  kind,
  onSelectMember,
}: {
  connectionId: string;
  member: string | null;
  active: boolean;
  live: boolean;
  members: Member[] | null;
  kind: ConnectionKind;
  onSelectMember: (m: string | null) => void;
}) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const open = useWorkspace((s) => s.open);
  const [params, setParams] = useState<QueriesParams>({ mode: { kind: "sinceReset" }, sort: "totalTime", limit: 50, database: null, toplevelOnly: true });
  const [intervalS, setIntervalS] = useState<number | null>(DEFAULT_INTERVAL_S);
  const sampler = useStatementsSampler(connectionId, member, live);
  const q = useQueriesReport(connectionId, member, params, { active, live, intervalS, tickAtMs: sampler.live?.atMs ?? null });
  const report = q.data;
  const data = report?.data;
  const window = params.mode.kind === "window";
  const reportMember = report?.member ?? null;

  // Тексты — по ключам без текста, один запрос на ключ; при смене узла набор запрошенных ключей сбрасывается.
  const requested = useRef(new Set<string>());
  useEffect(() => {
    requested.current.clear();
  }, [connectionId, reportMember]);
  useEffect(() => {
    if (!data || reportMember === null) return;
    const keys = data.rows.filter((r) => r.text === null && !requested.current.has(QUERY_KEY_ID(r.key))).map((r) => r.key);
    if (keys.length === 0) return;
    keys.forEach((k) => requested.current.add(QUERY_KEY_ID(k)));
    api
      .queryTexts(connectionId, reportMember, keys)
      .then((texts) => {
        const byKey = new Map(texts.map((x) => [QUERY_KEY_ID(x.key), x]));
        qc.setQueriesData<QueriesReport>({ queryKey: ["diag", connectionId, "queries"] }, (old) =>
          old
            ? {
                ...old,
                data: {
                  ...old.data,
                  rows: old.data.rows.map((r) => {
                    const x = byKey.get(QUERY_KEY_ID(r.key));
                    return x && x.text !== null ? { ...r, text: x.text, textTruncated: x.truncated } : r;
                  }),
                },
              }
            : old,
        );
      })
      // Тексты — украшение строк: при сбое остаётся queryId, а ключи освобождаются для следующего отчёта.
      .catch(() => keys.forEach((k) => requested.current.delete(QUERY_KEY_ID(k))));
  }, [data, connectionId, reportMember, qc]);

  // Базы выбираются из строк отчёта; выбранная база остаётся в списке и тогда, когда в её отчёте строк нет.
  const databases = useMemo(
    () => Array.from(new Set([...(params.database ? [params.database] : []), ...(data?.rows ?? []).map((r) => r.datname).filter((d): d is string => d !== null)])).sort(),
    [data, params.database],
  );
  const setSort = (sort: QuerySort) => setParams((p) => ({ ...p, sort }));
  const openQuery = (r: QueryStat) => {
    if (reportMember === null || r.datname === null) return;
    open({ kind: "query", connectionId, member: reportMember, database: r.datname, queryId: r.key.queryId, title: queryTitle(r.text, r.key.queryId) });
  };
  const coverage = () => {
    if (!window || !data) return null;
    const want = params.mode.kind === "window" ? params.mode.s / 60 : 0;
    if (data.coverageS === null) return t("diag.queries.firstTick");
    return t("diag.queries.coverage", { have: Math.round(data.coverageS / 60), want });
  };

  return (
    <>
      <DiagHeader
        tool="queries"
        controls={{
          members,
          kind,
          selected: member,
          leader: report?.followsLeader ? report.member : null,
          onSelect: onSelectMember,
          intervalS,
          onInterval: setIntervalS,
          windowS: null,
          onWindow: () => {},
          onRefresh: () => void q.refetch(),
          updatedAtMs: q.dataUpdatedAt > 0 ? q.dataUpdatedAt : null,
          stale: q.isError && report !== undefined,
        }}
      />
      <div className="toolbar">
        <label className="inline">
          <span>{t("diag.queries.mode")}</span>
          <select
            aria-label={t("diag.queries.mode")}
            value={modeValue(params.mode)}
            onChange={(e) =>
              setParams((p) => ({
                ...p,
                mode: parseMode(e.target.value),
                sort: sortAvailable(p.sort, data?.pgss ?? null, e.target.value.startsWith("window:")) ? p.sort : "totalTime",
              }))
            }
          >
            <option value="sinceReset">{t("diag.queries.modes.sinceReset")}</option>
            {[300, 900, 3600].map((s) => (
              <option key={s} value={`window:${s}`}>
                {t("diag.queries.modes.window", { min: s / 60 })}
              </option>
            ))}
          </select>
        </label>
        {coverage() && (
          <span className="kind-badge" role="status">
            {coverage()}
          </span>
        )}
        <label className="inline">
          <span>{t("diag.queries.database")}</span>
          <select aria-label={t("diag.queries.database")} value={params.database ?? ""} onChange={(e) => setParams((p) => ({ ...p, database: e.target.value || null }))}>
            <option value="">{t("diag.queries.allDatabases")}</option>
            {databases.map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
        </label>
        <label className="inline">
          <input type="checkbox" checked={params.toplevelOnly} onChange={(e) => setParams((p) => ({ ...p, toplevelOnly: e.target.checked }))} />
          <span>{t("diag.queries.toplevelOnly")}</span>
        </label>
        <label className="inline">
          <span>{t("diag.queries.limit")}</span>
          <select aria-label={t("diag.queries.limit")} value={params.limit} onChange={(e) => setParams((p) => ({ ...p, limit: Number(e.target.value) as 50 | 100 | 200 }))}>
            {LIMITS.map((l) => (
              <option key={l} value={l}>
                {l}
              </option>
            ))}
          </select>
        </label>
        <button type="button" onClick={() => open({ kind: "explain", connectionId, member })}>
          {t("diag.queries.openConsole")}
        </button>
      </div>
      {q.error && <ErrorBanner error={q.error} />}
      {sampler.error && <ErrorBanner error={sampler.error} />}
      {report && data ? (
        <div className={q.isPlaceholderData ? "diag-body diag-pending" : "diag-body"} aria-busy={q.isPlaceholderData}>
          <FindingsPanel connectionId={connectionId} findings={report.findings} tool="queries" member={report.member} />
          {report.unavailable.length > 0 && (
            <div className="diag-unavailable">
              {report.unavailable.map((u, i) => (
                <p key={`${u.part}:${i}`} className="muted">
                  {unavailableText(u)}
                </p>
              ))}
            </div>
          )}
          {data.pgss && (
            <div className="table-scroll">
              <table className="diag-table" aria-label={t("diag.tools.queries")}>
                <thead>
                  <tr>
                    <th scope="col">{t("diag.queries.columns.query")}</th>
                    {COLS.map((c) => {
                      const enabled = c.sort !== null && sortAvailable(c.sort, data.pgss, window);
                      const current = c.sort === params.sort;
                      return (
                        <th key={c.id} scope="col" className="num" aria-sort={current ? (params.sort === "hitRatio" ? "ascending" : "descending") : undefined}>
                          <button type="button" className="link" disabled={!enabled} title={enabled ? undefined : t("diag.queries.sortUnavailable")} onClick={() => c.sort && setSort(c.sort)}>
                            {t(c.labelKey)}
                            {current ? " ▾" : ""}
                          </button>
                        </th>
                      );
                    })}
                  </tr>
                </thead>
                <tbody>
                  {data.rows.map((r) => (
                    <tr key={QUERY_KEY_ID(r.key)}>
                      <td className="diag-query-cell" title={r.text ?? undefined}>
                        <button type="button" className="link mono" onClick={() => openQuery(r)} disabled={reportMember === null || r.datname === null}>
                          {queryTitle(r.text, r.key.queryId, 90)}
                        </button>
                        {r.textTruncated && <span className="muted"> {t("diag.queries.truncated")}</span>}
                      </td>
                      {COLS.map((c) => (
                        <td key={c.id} className="num">
                          {c.cell(r)}
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
              {data.rows.length === 0 && <p className="empty">{window ? t("diag.queries.emptyWindow") : t("diag.queries.empty")}</p>}
            </div>
          )}
          {data.pgss && (
            <p className="muted diag-note">
              {t("diag.queries.pgss", { version: data.pgss.version, reset: data.pgss.statsResetMs === null ? "—" : formatTimestamp(data.pgss.statsResetMs) })}
              {!data.pgss.trackPlanning && ` · ${t("diag.queries.noTrackPlanning")}`}
            </p>
          )}
        </div>
      ) : (
        !q.error && <p className="empty">{t("common.loading")}</p>
      )}
    </>
  );
}
