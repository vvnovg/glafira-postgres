import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../api/client";
import type { AppError, ConnectionKind, Member, Topology, TopologyEvent } from "../../api/types";
import { i18n, currentLocale } from "../../i18n";
import { tr } from "../../i18n/text";
import { formatBytes, formatCount, formatTimestamp } from "../../lib/format";
import { roleLabel } from "../../lib/topology";
import { useTopology, useTopologyEvents } from "../../lib/useConnectionEvents";
import { ErrorBanner } from "../common/ErrorBanner";
import { Glyph } from "../common/Glyph";
import { warningText } from "../connections/TestPanel";

/** Как часто обновляются быстрые факты узлов (спец. P1 §9): топология опрашивается раз в 10 с. */
const FACTS_REFRESH_MS = 10_000;

export function eventText(e: TopologyEvent, kind: ConnectionKind): string {
  const t = i18n.t.bind(i18n);
  switch (e.kind) {
    case "leaderChanged":
      return t("overview.event.leaderChanged", { from: e.from ?? "—", to: e.to ?? "—" });
    case "memberAdded":
      return t("overview.event.memberAdded", { name: e.name });
    case "memberRemoved":
      return t("overview.event.memberRemoved", { name: e.name });
    case "memberRoleChanged":
      return t("overview.event.memberRoleChanged", { name: e.name, from: roleLabel(e.from, kind), to: roleLabel(e.to, kind) });
    case "memberStateChanged":
      return t("overview.event.memberStateChanged", { name: e.name, from: e.from ?? "—", to: e.to ?? "—" });
    case "memberUnreachable":
      return t("overview.event.memberUnreachable", { name: e.name });
    case "memberReachable":
      return t("overview.event.memberReachable", { name: e.name });
    case "timelineChanged":
      return t("overview.event.timelineChanged", { from: e.from, to: e.to });
    case "pauseChanged":
      return t(e.paused ? "overview.event.paused" : "overview.event.resumed");
  }
}

/** Время работы узла. Часы сервера и клиента могут расходиться, поэтому отрицательная разница — это 0. */
function duration(ms: number): string {
  const m = Math.floor(Math.max(0, ms) / 60_000);
  if (m < 60) return i18n.t("overview.minutes", { count: m });
  const h = Math.floor(m / 60);
  if (h < 48) return i18n.t("overview.hours", { count: h });
  return i18n.t("overview.days", { count: Math.floor(h / 24) });
}

function replayDelay(ms: number): string {
  const s = (ms / 1000).toLocaleString(currentLocale(), { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  return `${s} s`;
}

/** Теги Patroni: включённые флаги — именем, остальные значения (replicatefrom, failover_priority) — `имя: значение`; выключенные не показываются. */
function tagList(tags: Record<string, unknown>): string[] {
  return Object.entries(tags).flatMap(([k, v]) => {
    if (v === true) return [k];
    if (v === false || v === null || v === undefined) return [];
    return [`${k}: ${typeof v === "object" ? JSON.stringify(v) : String(v)}`];
  });
}

/** Узлы, возглавляющие кластер: primary, а в standby-кластере Patroni — standby leader. */
function leaders(members: Member[]): Member[] {
  const primaries = members.filter((m) => m.role === "primary");
  return primaries.length > 0 ? primaries : members.filter((m) => m.role === "standbyLeader");
}

function FactsCells({ connectionId, m, live }: { connectionId: string; m: Member; live: boolean }) {
  const { t } = useTranslation();
  const q = useQuery({
    queryKey: ["memberFacts", connectionId, m.name],
    queryFn: () => api.getMemberFacts(connectionId, m.name),
    // Подключение закрыто или упало — спрашивать узлы нечем, в таблице остаются последние известные участники.
    enabled: m.reachable && live,
    refetchInterval: FACTS_REFRESH_MS,
  });
  const asking = m.reachable && live;
  if (!asking || !q.data) {
    // «…» — ответа ещё нет, «—» — узнать нечего (узел недоступен, подключение закрыто или запрос не удался).
    const none = asking && q.isPending ? "…" : "—";
    return (
      <>
        <td>
          {none}
          {asking && q.isError && (
            <>
              {" "}
              <Glyph className="warn" symbol="⚠" label={t("overview.factsFailed", { error: tr(toAppError(q.error)) })} />
            </>
          )}
        </td>
        <td>{none}</td>
        <td>{none}</td>
      </>
    );
  }
  return (
    <>
      <td>{duration(Date.now() - q.data.startedAtMs)}</td>
      <td>{`${formatCount(q.data.connections)} / ${formatCount(q.data.maxConnections)}`}</td>
      <td>{formatCount(q.data.databases)}</td>
    </>
  );
}

/** wrap — ячейка с длинным списком (теги) переносится по словам, остальные не ломаются. */
type Column = { id: string; header: string; wrap?: boolean; cell: (m: Member) => ReactNode };

function MembersTable({ connectionId, topo, live, selected }: { connectionId: string; topo: Topology; live: boolean; selected?: string }) {
  const { t } = useTranslation();
  const patroni = topo.kind === "patroni";
  // У одиночного сервера столбцы, которые у него бывают пустыми, не показываются: лаг, источник WAL и состояние появляются у standby.
  const filled = (has: (m: Member) => boolean) => topo.kind !== "server" || topo.members.some(has);
  const dash = (v: ReactNode) => v ?? "—";

  const columns: Column[] = [
    {
      id: "name",
      header: t("overview.col.name"),
      cell: (m) => (
        <>
          {m.name}
          {!m.reachable && (
            <>
              {" "}
              <Glyph
                className="warn"
                symbol="⚠"
                label={m.error ? t("overview.unreachable", { error: tr(m.error) }) : t("sidebar.unreachable")}
              />
            </>
          )}
        </>
      ),
    },
    { id: "role", header: t("overview.col.role"), cell: (m) => roleLabel(m.role, topo.kind) },
    ...(filled((m) => m.state !== null) ? [{ id: "state", header: t("overview.col.state"), cell: (m: Member) => dash(m.state) }] : []),
    { id: "address", header: t("overview.col.address"), cell: (m) => `${m.host}:${m.port}` },
    ...(patroni ? [{ id: "timeline", header: t("overview.col.timeline"), cell: (m: Member) => dash(m.timeline) }] : []),
    ...(filled((m) => m.lagBytes !== null)
      ? [{ id: "lag", header: t("overview.col.lag"), cell: (m: Member) => (m.lagBytes === null ? "—" : formatBytes(m.lagBytes)) }]
      : []),
    ...(filled((m) => m.replayDelayMs !== null)
      ? [{ id: "replay", header: t("overview.col.replay"), cell: (m: Member) => (m.replayDelayMs === null ? "—" : replayDelay(m.replayDelayMs)) }]
      : []),
    ...(filled((m) => m.upstream !== null) ? [{ id: "upstream", header: t("overview.col.upstream"), cell: (m: Member) => dash(m.upstream) }] : []),
    ...(patroni
      ? [
          {
            id: "tags",
            header: t("overview.col.tags"),
            wrap: true,
            cell: (m: Member) => {
              const tags = tagList(m.tags);
              if (tags.length === 0 && !m.pendingRestart) return "—";
              return (
                <>
                  {tags.join(", ")}
                  {m.pendingRestart && <div className="warn">{t("overview.pendingRestart")}</div>}
                </>
              );
            },
          },
        ]
      : []),
    {
      id: "versions",
      header: t("overview.col.versions"),
      cell: (m) => (
        <>
          {m.serverVersion ?? "—"}
          {m.patroniVersion && <span className="muted"> / Patroni {m.patroniVersion}</span>}
          {m.dcsLastSeenMs !== null && <div className="muted">{t("overview.dcsSeen", { at: formatTimestamp(m.dcsLastSeenMs) })}</div>}
        </>
      ),
    },
  ];

  return (
    <div className="table-scroll">
      <table className="members-table" aria-label={t("sidebar.members")}>
        <thead>
          <tr>
            {columns.map((c) => (
              <th key={c.id}>{c.header}</th>
            ))}
            <th>{t("overview.col.uptime")}</th>
            <th>{t("overview.col.connections")}</th>
            <th>{t("overview.col.databases")}</th>
          </tr>
        </thead>
        <tbody>
          {topo.members.map((m) => {
            const isSelected = m.name === selected;
            return (
              <tr key={m.name} className={isSelected ? "selected" : undefined} aria-current={isSelected ? "true" : undefined}>
                {columns.map((c) => (
                  <td key={c.id} className={c.wrap ? "wrap" : undefined}>
                    {c.cell(m)}
                  </td>
                ))}
                <FactsCells connectionId={connectionId} m={m} live={live} />
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export function Overview({ connectionId, member }: { connectionId: string; member?: string }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const journalId = useId();
  const conns = useQuery({ queryKey: ["connections"], queryFn: api.listConnections });
  const conn = conns.data?.find((c) => c.id === connectionId);
  const view = useTopology(connectionId);
  const events = useTopologyEvents(connectionId);
  const [error, setError] = useState<AppError | null>(null);
  const [busy, setBusy] = useState(false);

  const connect = async () => {
    setBusy(true);
    setError(null);
    try {
      qc.setQueryData(["topology", connectionId], await api.connect(connectionId));
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(false);
    }
  };

  // Ошибка попытки подключения устаревает, когда подключение стало живым (например, из дерева).
  const state = view.data?.state;
  useEffect(() => {
    if (state === "connected" || state === "degraded") setError(null);
  }, [state]);

  // Данные важнее ошибки обновления: при сбое перечитывания остаётся последний известный вид.
  if (view.data === undefined) {
    return view.error ? <ErrorBanner error={view.error} onRetry={() => void view.refetch()} /> : <p className="empty">{t("common.loading")}</p>;
  }
  const v = view.data;
  const topo = v.topology;
  const live = v.state === "connected" || v.state === "degraded";
  const shownError = error ?? v.error;
  // У одиночного сервера один узел и «лидера» нет: его роль видна в таблице и в дереве.
  const head = topo && topo.kind !== "server" ? leaders(topo.members) : [];

  return (
    <div className="page">
      <div className="toolbar">
        <h2>{conn?.name ?? t("tabs.unknownConnection")}</h2>
        <span className="muted">{conn ? t(`connections.form.kinds.${conn.kind}`) : ""}</span>
        {topo?.scope && <span className="muted">{t("overview.scope", { scope: topo.scope })}</span>}
        {topo?.paused && <span className="warn">{t("overview.paused")}</span>}
        {topo && head.length > 0 && (
          <span>{t("overview.head", { role: roleLabel(head[0].role, topo.kind), names: head.map((m) => m.name).join(", ") })}</span>
        )}
        <span>{t(`sidebar.state.${v.state}`)}</span>
        {(v.state === "disconnected" || v.state === "failed") && (
          <button type="button" className="primary" disabled={busy} onClick={() => void connect()}>
            {t("sidebar.connect")}
          </button>
        )}
      </div>
      {view.error && <ErrorBanner error={view.error} onRetry={() => void view.refetch()} />}
      {shownError && <ErrorBanner error={shownError} />}
      {topo && topo.warnings.length > 0 && (
        <div className="confirm" role="alert">
          {topo.warnings.map((w, i) => (
            // Возможный split-brain — единственное предупреждение красным (спец. P1 §6.3).
            <span key={i} className={w.kind === "multiplePrimaries" ? "warning-red" : undefined}>
              {warningText(w, t)}
            </span>
          ))}
        </div>
      )}
      {topo && <MembersTable connectionId={connectionId} topo={topo} live={live} selected={member} />}
      {topo && (
        <section aria-labelledby={journalId}>
          <h3 id={journalId}>{t("overview.journal")}</h3>
          {(events.data ?? []).length === 0 ? (
            <p className="muted">{t("overview.journalEmpty")}</p>
          ) : (
            <ul className="steps">
              {(events.data ?? []).map((e, i) => (
                <li key={`${e.atMs}:${i}`}>
                  <span className="muted">{formatTimestamp(e.atMs)}</span> {eventText(e, topo.kind)}
                </li>
              ))}
            </ul>
          )}
        </section>
      )}
    </div>
  );
}
