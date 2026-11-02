import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { ConnectionKind, Member, QueriesParams, QuerySort, QueryStat, SnapshotInfo } from "../../api/types";
import { formatBytes, formatCount, formatDuration, formatMs, formatPercent, formatTimestamp } from "../../lib/format";
import { useQueriesReport } from "../../lib/useQueriesReport";
import { useStatementsSampler } from "../../lib/useStatementsSampler";
import { useWorkspace } from "../../state/workspace";
import { ErrorBanner } from "../common/ErrorBanner";
import { DiagHeader } from "../diag/DiagHeader";
import { FindingsPanel } from "../diag/FindingsPanel";
import { unavailableText } from "../diag/unavailable";
import { BLOCK_BYTES, QUERY_KEY_ID, cpuMs, queryTitle, snapshotFileParts, sortAvailable } from "./queriesText";
import { SnapshotsPanel } from "./SnapshotsPanel";

/** Интервал обновления режима «с момента сброса» по умолчанию, с. */
const DEFAULT_INTERVAL_S = 10;
const LIMITS = [50, 100, 200] as const;
/** Задержка ввода в поиске, мс: запрос отчёта уходит, когда владелец перестал печатать. */
const SEARCH_DEBOUNCE_MS = 300;
/** Ключи, тексты которых не удалось получить, перезапрашиваются не чаще этого, мс: постоянная ошибка не повторяется на каждом обновлении. */
const TEXT_RETRY_MS = 60_000;

interface Col {
  id: string;
  sort: QuerySort | null;
  labelKey: string;
  cell(r: QueryStat, trackPlanning: boolean): string;
  /** Колонка `pg_stat_kcache`: показывается, только когда расширение есть (`pgss.kcache`). */
  kcache?: true;
}

const COLS: Col[] = [
  { id: "share", sort: "totalTime", labelKey: "diag.queries.columns.share", cell: (r) => formatPercent(r.sharePct) },
  { id: "total", sort: "totalTime", labelKey: "diag.queries.columns.total", cell: (r) => formatMs(r.totalExecMs) },
  { id: "mean", sort: "meanTime", labelKey: "diag.queries.columns.mean", cell: (r) => formatMs(r.meanExecMs) },
  { id: "calls", sort: "calls", labelKey: "diag.queries.columns.calls", cell: (r) => formatCount(r.calls) },
  { id: "rowsPerCall", sort: "rowsPerCall", labelKey: "diag.queries.columns.rowsPerCall", cell: (r) => formatCount(r.rowsPerCall, 1) },
  { id: "hit", sort: "hitRatio", labelKey: "diag.queries.columns.hitRatio", cell: (r) => (r.hitRatio === null ? "—" : formatPercent(r.hitRatio * 100)) },
  { id: "read", sort: "sharedRead", labelKey: "diag.queries.columns.read", cell: (r) => formatBytes(r.sharedRead * BLOCK_BYTES) },
  { id: "temp", sort: "tempWritten", labelKey: "diag.queries.columns.temp", cell: (r) => formatBytes(r.tempWritten * BLOCK_BYTES) },
  { id: "wal", sort: "walBytes", labelKey: "diag.queries.columns.wal", cell: (r) => formatBytes(r.walBytes) },
  { id: "io", sort: "ioTime", labelKey: "diag.queries.columns.io", cell: (r) => formatMs(r.ioReadMs + r.ioWriteMs) },
  { id: "cpu", sort: "cpuTime", labelKey: "diag.queries.columns.cpu", kcache: true, cell: (r) => { const v = cpuMs(r); return v === null ? "—" : formatMs(v); } },
  { id: "disk", sort: null, labelKey: "diag.queries.columns.disk", kcache: true, cell: (r) => (r.diskReadBytes === null ? "—" : formatBytes(r.diskReadBytes)) },
  { id: "plan", sort: "planTime", labelKey: "diag.queries.columns.plan", cell: (r, trackPlanning) => (trackPlanning ? formatMs(r.totalPlanMs) : "—") },
  { id: "jit", sort: "jitTime", labelKey: "diag.queries.columns.jit", cell: (r) => (r.jitMs === null ? "—" : formatMs(r.jitMs)) },
  { id: "stddev", sort: "stddevTime", labelKey: "diag.queries.columns.stddev", cell: (r) => (r.stddevExecMs === null ? "—" : formatMs(r.stddevExecMs)) },
  { id: "max", sort: "maxTime", labelKey: "diag.queries.columns.max", cell: (r) => (r.maxExecMs === null ? "—" : formatMs(r.maxExecMs)) },
];

const modeValue = (m: QueriesParams["mode"]) => (m.kind === "window" ? `window:${m.s}` : m.kind);
/** Режим из списка; «Между снимками» в списке — только текущий (пара выбирается в панели снимков), выбор другого его сбрасывает. */
const parseMode = (v: string, current: QueriesParams["mode"]): QueriesParams["mode"] => {
  if (v.startsWith("window:")) return { kind: "window", s: Number(v.slice(7)) as 300 | 900 | 3600 };
  if (v === "between" && current.kind === "between") return current;
  return { kind: "sinceReset" };
};

/**
 * Инструмент «Запросы» (спец. P3 §4.7): общая шапка диагностики, режим и фильтры, таблица топа, находки. Отчёт «за
 * окно» обновляется по тику сэмплера pgss, на который вкладка подписана, пока смонтирована. Тексты строк подгружаются по
 * ключам один раз и хранятся здесь же (по ключу строки), а к строкам отчёта подмешиваются при отрисовке: обновление
 * отчёта, ушедшее раньше ответа с текстами, их не затирает. Список баз копится из всех прочитанных отчётов узла.
 *
 * Без подключения (`live === false`, `closed` — уведомление вкладки с кнопкой «Подключиться») доступны только снимки:
 * панель снимков и режим «между снимками», который читается с диска. Живые режимы ждут подключения; обрыв подключения
 * в режиме «между снимками» отчёт не теряет — вкладка рисует тот же вид.
 */
export function QueriesView({
  connectionId,
  member,
  active,
  live,
  members,
  kind,
  onSelectMember,
  closed,
}: {
  connectionId: string;
  member: string | null;
  active: boolean;
  live: boolean;
  members: Member[] | null;
  kind: ConnectionKind;
  onSelectMember: (m: string | null) => void;
  /** Уведомление о закрытом подключении (вкладка); показывается, пока `live === false`. */
  closed?: ReactNode;
}) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const [params, setParams] = useState<QueriesParams>({ mode: { kind: "sinceReset" }, sort: "totalTime", limit: 50, database: null, toplevelOnly: true });
  /** Колонка, по которой щёлкнули: «Доля» и «Всего» сортируют одинаково, стрелка — только у выбранной. */
  const [sortCol, setSortCol] = useState("share");
  /** Строка поиска как набрана; в параметры отчёта (`params.search`) уходит с задержкой. */
  const [searchInput, setSearchInput] = useState("");
  useEffect(() => {
    const value = searchInput.trim();
    const id = window.setTimeout(() => setParams((p) => ((p.search ?? "") === value ? p : { ...p, search: value === "" ? undefined : value })), SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(id);
  }, [searchInput]);
  const [intervalS, setIntervalS] = useState<number | null>(DEFAULT_INTERVAL_S);
  // Сэмплер — на разрешённом узле: закреплённом или узле отчёта «за лидером» (Rust разрешает лидера на каждом отчёте, а
  // подписку с `null` — один раз). После смены лидера новый отчёт называет новый узел, и подписка переходит на него.
  // Узел берётся только из настоящих данных: заглушка `keepPreviousData` после смены режима узла — отчёт прежнего узла.
  const [resolved, setResolved] = useState<string | null>(null);
  const sampler = useStatementsSampler(connectionId, member ?? resolved, live && (member !== null || resolved !== null));
  const q = useQueriesReport(connectionId, member, params, { active, live, intervalS, tickAtMs: sampler.live?.atMs ?? null });
  const report = q.data;
  const data = report?.data;
  const byTick = params.mode.kind === "window";
  const between = params.mode.kind === "between";
  // min/max/stddev есть только у накопительного отчёта: в окне и между снимками строки — дельты.
  const deltas = params.mode.kind !== "sinceReset";
  const [showSnapshots, setShowSnapshots] = useState(false);
  /** Снимки пары режима «между снимками» — для подписи (время и узел); в параметрах отчёта только имена файлов. */
  const [pair, setPair] = useState<{ from: SnapshotInfo; to: SnapshotInfo } | null>(null);
  const reportMember = report?.member ?? null;
  // Узел отчёта «между снимками» — узел снимка, а не лидер: сэмплер за ним не переходит.
  const freshMember = q.isPlaceholderData || data?.mode.kind === "between" ? null : reportMember;
  useEffect(() => {
    if (member !== null) setResolved(null);
    else if (freshMember !== null) setResolved(freshMember);
  }, [member, freshMember]);

  // Тексты и базы — по узлу отчёта: при смене подключения или узла забываются. Ответ, пришедший для прежнего узла, отбрасывается.
  const [texts, setTexts] = useState<{ owner: string; byId: ReadonlyMap<string, { text: string; truncated: boolean }> }>({ owner: "", byId: new Map() });
  const [seenDbs, setSeenDbs] = useState<{ owner: string; names: ReadonlySet<string> }>({ owner: "", names: new Set() });
  const owner = JSON.stringify([connectionId, reportMember]);
  const requested = useRef(new Set<string>());
  const failedAt = useRef(new Map<string, number>());
  const ownerRef = useRef(owner);
  useEffect(() => {
    if (ownerRef.current === owner) return;
    ownerRef.current = owner;
    requested.current.clear();
    failedAt.current.clear();
  }, [owner]);
  useEffect(() => {
    // Между снимками тексты — из самих снимков: запрос без текста на сервере не ищется (подключение может быть закрыто).
    if (!data || reportMember === null || data.mode.kind === "between") return;
    const now = Date.now();
    const known = texts.owner === owner ? texts.byId : null;
    const keys = data.rows
      .filter((r) => {
        const id = QUERY_KEY_ID(r.key);
        if (r.text !== null || known?.has(id) || requested.current.has(id)) return false;
        const failed = failedAt.current.get(id);
        return failed === undefined || now - failed >= TEXT_RETRY_MS;
      })
      .map((r) => r.key);
    if (keys.length === 0) return;
    const ids = keys.map(QUERY_KEY_ID);
    ids.forEach((id) => requested.current.add(id));
    const asked = owner;
    api
      .queryTexts(connectionId, reportMember, keys)
      .then((got) => {
        if (ownerRef.current !== asked) return;
        setTexts((prev) => {
          const byId = new Map(prev.owner === asked ? prev.byId : []);
          for (const x of got) if (x.text !== null) byId.set(QUERY_KEY_ID(x.key), { text: x.text, truncated: x.truncated });
          return { owner: asked, byId };
        });
      })
      // Тексты — украшение строк: при сбое остаётся queryId, а ключи перезапрашиваются не раньше чем через TEXT_RETRY_MS.
      .catch(() => {
        if (ownerRef.current !== asked) return;
        const at = Date.now();
        ids.forEach((id) => {
          requested.current.delete(id);
          failedAt.current.set(id, at);
        });
      });
  }, [data, connectionId, reportMember, owner, texts]);
  useEffect(() => {
    const names = (data?.rows ?? []).map((r) => r.datname).filter((d): d is string => d !== null);
    setSeenDbs((prev) => {
      const base = prev.owner === owner ? prev.names : new Set<string>();
      if (prev.owner === owner && names.every((n) => base.has(n))) return prev;
      return { owner, names: new Set([...base, ...names]) };
    });
  }, [data, owner]);

  // Строки с подмешанными текстами: объект строки меняется только у тех, кому текст подставлен.
  const rows = useMemo(() => {
    const byId = texts.owner === owner ? texts.byId : null;
    return (data?.rows ?? []).map((r) => {
      const x = r.text === null ? byId?.get(QUERY_KEY_ID(r.key)) : undefined;
      return x ? { ...r, text: x.text, textTruncated: x.truncated } : r;
    });
  }, [data, texts, owner]);
  // Базы выбираются из всех прочитанных отчётов узла; выбранная база остаётся в списке и тогда, когда в её отчёте строк нет.
  const databases = useMemo(() => {
    const names = new Set(seenDbs.owner === owner ? seenDbs.names : []);
    if (params.database) names.add(params.database);
    return Array.from(names).sort();
  }, [seenDbs, owner, params.database]);
  // Текст запроса для находки над таблицей: из строк отчёта по `queryId` и базе (тексты подмешаны); нет — `null`, тогда показывается `queryId`.
  const queryText = (database: string, queryId: string) => rows.find((r) => r.key.queryId === queryId && r.datname === database && r.text !== null)?.text ?? null;
  const setSort = (col: Col) => {
    if (!col.sort) return;
    const sort = col.sort;
    setSortCol(col.id);
    setParams((p) => ({ ...p, sort }));
  };
  // Стрелка — у колонки, по которой щёлкнули, если её сортировка ещё действует; иначе (сброс сортировки сменой режима) — у первой подходящей.
  const activeCol = COLS.find((c) => c.id === sortCol && c.sort === params.sort) ?? COLS.find((c) => c.sort === params.sort);
  // Колонки CPU и диска — только с `pg_stat_kcache` у отчёта.
  const cols = COLS.filter((c) => !c.kcache || data?.pgss?.kcache === true);
  // Подпись пары: местное время снятия и узел один раз; без сведений о снимках (не должно случаться) — имена файлов.
  const betweenLabel = (from: string, to: string) => {
    const known = pair && pair.from.file === from && pair.to.file === to ? pair : null;
    return known
      ? t("diag.queries.betweenLabel", { member: known.from.member, from: formatTimestamp(known.from.takenAtMs), to: formatTimestamp(known.to.takenAtMs) })
      : (() => {
          const a = snapshotFileParts(from);
          return t("diag.queries.betweenLabel", { member: a.member, from: a.stamp, to: snapshotFileParts(to).stamp });
        })();
  };
  const offline = !live;
  const enterBetween = (from: SnapshotInfo, to: SnapshotInfo) => {
    setPair({ from, to });
    // Есть ли kcache в снимках, до отчёта неизвестно: сортировка по CPU сбрасывается, чтобы не получить отказ.
    setParams((p) => ({ ...p, mode: { kind: "between", from: from.file, to: to.file }, sort: p.sort !== "cpuTime" && sortAvailable(p.sort, data?.pgss ?? null, true) ? p.sort : "totalTime" }));
    setShowSnapshots(false);
  };
  const onSnapshotDeleted = (file: string) => {
    if (params.mode.kind === "between" && (params.mode.from === file || params.mode.to === file)) exitBetween();
  };
  const exitBetween = () => setParams((p) => (p.mode.kind === "between" ? { ...p, mode: { kind: "sinceReset" } } : p));
  const openQuery = (r: QueryStat) => {
    if (reportMember === null || r.datname === null) return;
    open({ kind: "query", connectionId, member: reportMember, database: r.datname, queryId: r.key.queryId, title: queryTitle(r.text, r.key.queryId) });
  };
  // Покрытие — по режиму самого отчёта: при переключении в окно на экране ещё может быть отчёт «с момента сброса».
  const coverage = (() => {
    if (data?.mode.kind === "between") return data.coverageS === null ? null : t("diag.queries.betweenCoverage", { duration: formatDuration(data.coverageS) });
    if (!data || data.mode.kind !== "window") return null;
    if (data.coverageS === null) return t("diag.queries.firstTick");
    return t("diag.queries.coverage", { have: Math.round(data.coverageS / 60), want: data.mode.s / 60 });
  })();

  // Без подключения и вне режима «между снимками» показывать нечего, кроме снимков.
  if (offline && !between) {
    return (
      <>
        <DiagHeader tool="queries" />
        {closed}
        <p className="muted">{t("diag.queries.offline")}</p>
        <SnapshotsPanel connectionId={connectionId} member={null} onBetween={enterBetween} onDeleted={onSnapshotDeleted} />
      </>
    );
  }

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
          noInterval: between,
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
                mode: parseMode(e.target.value, p.mode),
                sort: sortAvailable(p.sort, data?.pgss ?? null, e.target.value !== "sinceReset") ? p.sort : "totalTime",
              }))
            }
          >
            <option value="sinceReset" disabled={offline}>
              {t("diag.queries.modes.sinceReset")}
            </option>
            {[300, 900, 3600].map((s) => (
              <option key={s} value={`window:${s}`} disabled={offline}>
                {t("diag.queries.modes.window", { min: s / 60 })}
              </option>
            ))}
            {between && <option value="between">{t("diag.queries.modes.between")}</option>}
          </select>
        </label>
        {coverage && (
          <span className="kind-badge" role="status">
            {coverage}
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
          <span>{t("diag.queries.search")}</span>
          <input type="search" aria-label={t("diag.queries.search")} placeholder={t("diag.queries.searchPlaceholder")} value={searchInput} onChange={(e) => setSearchInput(e.target.value)} />
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
        <button type="button" aria-expanded={showSnapshots} onClick={() => setShowSnapshots((v) => !v)}>
          {t("diag.queries.snapshots")}
        </button>
        <button type="button" onClick={() => open({ kind: "explain", connectionId, member })}>
          {t("diag.queries.openConsole")}
        </button>
      </div>
      {offline && closed}
      {params.mode.kind === "between" && (
        <div className="toolbar snapshots-between">
          <strong>{betweenLabel(params.mode.from, params.mode.to)}</strong>
          <button type="button" onClick={exitBetween}>
            {t("diag.queries.betweenExit")}
          </button>
        </div>
      )}
      {showSnapshots && (
        <SnapshotsPanel
          connectionId={connectionId}
          member={offline ? null : (member ?? resolved)}
          onBetween={enterBetween}
          onDeleted={onSnapshotDeleted}
        />
      )}
      {q.error && <ErrorBanner error={q.error} />}
      {sampler.error && <ErrorBanner error={sampler.error} />}
      {report && data ? (
        <div className={q.isPlaceholderData ? "diag-body diag-pending" : "diag-body"} aria-busy={q.isPlaceholderData}>
          <FindingsPanel connectionId={connectionId} findings={report.findings} tool="queries" member={report.member} queryText={queryText} />
          {report.unavailable.length > 0 && (
            <div className="diag-unavailable">
              {report.unavailable.map((u, i) => (
                <p key={`${u.part}:${i}`} className="muted">
                  {unavailableText(u)}
                  {/* После сброса pgss счётчики pgss — с момента сброса, а pg_stat_kcache не сбрасывался: его дельта — за весь промежуток. */}
                  {u.reason.key === "diag.unavailable.betweenReset" && data.pgss?.kcache && ` ${t("diag.queries.betweenResetKcache")}`}
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
                    {cols.map((c) => {
                      const enabled = c.sort !== null && sortAvailable(c.sort, data.pgss, deltas);
                      const current = c === activeCol;
                      // Hit % сортируется по возрастанию (худшие сверху), остальные — по убыванию.
                      const ascending = params.sort === "hitRatio";
                      return (
                        <th key={c.id} scope="col" className="num" aria-sort={current ? (ascending ? "ascending" : "descending") : undefined}>
                          <button type="button" className="link" disabled={!enabled} title={enabled ? undefined : t("diag.queries.sortUnavailable")} onClick={() => setSort(c)}>
                            {t(c.labelKey)}
                            {current ? (ascending ? " ▴" : " ▾") : ""}
                          </button>
                          {c.kcache && data.pgss?.kcacheSeparate && (
                            <span className="muted" title={t("diag.queries.kcacheColumnHint")}>
                              {" *"}
                            </span>
                          )}
                        </th>
                      );
                    })}
                  </tr>
                </thead>
                <tbody>
                  {rows.map((r) => (
                    <tr key={QUERY_KEY_ID(r.key)}>
                      <td className="diag-query-cell" title={r.text ?? undefined}>
                        <button
                          type="button"
                          className="link mono"
                          onClick={() => openQuery(r)}
                          disabled={reportMember === null || r.datname === null}
                          title={r.datname === null ? t("diag.queries.goneHint") : undefined}
                        >
                          {queryTitle(r.text, r.key.queryId, 90)}
                        </button>
                        {r.own && (
                          <span className="kind-badge" title={t("diag.queries.ownHint")}>
                            {t("diag.queries.own")}
                          </span>
                        )}
                        {r.textTruncated && <span className="muted"> {t("diag.queries.truncated")}</span>}
                      </td>
                      {cols.map((c) => (
                        <td key={c.id} className="num">
                          {c.cell(r, data.pgss?.trackPlanning ?? true)}
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
              {data.rows.length === 0 && (
                <p className="empty">{params.search
                    ? t("diag.queries.noMatch", { q: params.search })
                    : between
                      ? t("diag.queries.emptyBetween")
                      : byTick
                        ? t("diag.queries.emptyWindow")
                        : t("diag.queries.empty")}</p>
              )}
            </div>
          )}
          {data.pgss && (
            <p className="muted diag-note">
              {data.pgss.hasInfo
                ? t("diag.queries.pgss", { version: data.pgss.version, reset: data.pgss.statsResetMs === null ? "—" : formatTimestamp(data.pgss.statsResetMs) })
                : t("diag.queries.pgssNoInfo", { version: data.pgss.version })}
              {!data.pgss.trackPlanning && ` · ${t("diag.queries.noTrackPlanning")}`}
            </p>
          )}
          {data.pgss?.kcacheSeparate && (
            <p className="muted diag-note">
              {"* "}
              {data.pgss.kcacheSinceMs === null ? t("diag.queries.kcacheSeparate") : t("diag.queries.kcacheSince", { since: formatTimestamp(data.pgss.kcacheSinceMs) })}{" "}
              {t("diag.queries.kcacheResetBoth")} <code>SELECT pg_stat_statements_reset(); SELECT pg_stat_kcache_reset();</code>
            </p>
          )}
        </div>
      ) : (
        !q.error && <p className="empty">{t("common.loading")}</p>
      )}
    </>
  );
}
