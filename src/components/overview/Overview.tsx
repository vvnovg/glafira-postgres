import { useIsMutating, useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../api/client";
import type { AppError, ConnectionKind, Member, PatroniAction, Topology, TopologyEvent } from "../../api/types";
import { i18n, currentLocale } from "../../i18n";
import { tr } from "../../i18n/text";
import { formatBytes, formatCount, formatScheduled, formatTimestamp } from "../../lib/format";
import { roleLabel } from "../../lib/topology";
import { canAct, useConnectionProfile } from "../../lib/useConnectionProfile";
import { refreshAfterConnect, useTopology, useTopologyEvents } from "../../lib/useConnectionEvents";
import { PatroniActionDialog } from "../actions/PatroniActionDialog";
import { ErrorBanner } from "../common/ErrorBanner";
import { Glyph } from "../common/Glyph";
import { hostKeyUnknown } from "../connections/profile";
import { warningText } from "../connections/TestPanel";
import { TrustHostKeyDialog } from "../connections/TrustHostKeyDialog";

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

/**
 * Ключи строк журнала. Новые события добавляются сверху, поэтому ключ по индексу пересоздавал бы все строки.
 * Событие описывается целиком (время, вид, узлы, значения), а полные дубли различаются номером, который
 * считается от старых событий к новым: добавление сверху его не меняет.
 */
function eventKeys(events: TopologyEvent[]): string[] {
  const seen = new Map<string, number>();
  return events
    .map((e) => JSON.stringify(e))
    .reverse()
    .map((base) => {
      const n = seen.get(base) ?? 0;
      seen.set(base, n + 1);
      return n === 0 ? base : `${base}#${n}`;
    })
    .reverse();
}

/** Узлы, возглавляющие кластер: primary, а в standby-кластере Patroni — standby leader. */
function leaders(members: Member[]): Member[] {
  const primaries = members.filter((m) => m.role === "primary");
  return primaries.length > 0 ? primaries : members.filter((m) => m.role === "standbyLeader");
}

/**
 * Перечитывать ли факты узла по таймеру. Скрытая вкладка не опрашивает узлы. После отказа во входе (AUTH_FAILED)
 * опрос останавливается: повтор каждые 10 с с неверным паролем блокирует учётную запись (LDAP, PAM, fail2ban).
 * Rust и сам не входит повторно, пока подключение не откроют заново; закрытие подключения убирает и эти запросы.
 */
function factsRefetchInterval(active: boolean, error: unknown): number | false {
  if (!active) return false;
  return error != null && toAppError(error).code === "AUTH_FAILED" ? false : FACTS_REFRESH_MS;
}

function FactsCells({ connectionId, m, live, active }: { connectionId: string; m: Member; live: boolean; active: boolean }) {
  const { t } = useTranslation();
  const q = useQuery({
    queryKey: ["memberFacts", connectionId, m.name],
    queryFn: () => api.getMemberFacts(connectionId, m.name),
    // Подключение закрыто или упало — спрашивать узлы нечем, в таблице остаются последние известные участники.
    enabled: m.reachable && live,
    refetchInterval: (query) => factsRefetchInterval(active, query.state.error),
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
  // Последний ответ есть, но очередное чтение не удалось: цифры устарели. Значок с причиной и приглушённый цвет
  // говорят об этом; без них сбой («too many clients», таймаут) выглядел бы как живые числа.
  const stale = q.isError;
  const cls = stale ? "stale" : undefined;
  // Время работы — на момент последнего чтения. q.dataUpdatedAt меняется при каждом опросе, даже при тех же данных,
  // поэтому ячейка перерисовывается и подпись не замирает, пока цифры не изменились.
  const uptime = duration(q.dataUpdatedAt - q.data.startedAtMs);
  return (
    <>
      <td className={cls}>
        {uptime}
        {stale && (
          <>
            {" "}
            <Glyph className="warn" symbol="⚠" label={t("overview.factsStale", { error: tr(toAppError(q.error)) })} />
          </>
        )}
      </td>
      <td className={cls}>{`${formatCount(q.data.connections)} / ${formatCount(q.data.maxConnections)}`}</td>
      <td className={cls}>{formatCount(q.data.databases)}</td>
    </>
  );
}

/** wrap — ячейка с длинным списком (теги) переносится по словам, остальные не ломаются. */
type Column = { id: string; header: string; wrap?: boolean; cell: (m: Member) => ReactNode };

/** Действия Patroni над участником (столбец «Действия»): открыть диалог, недоступны ли кнопки и почему (подсказка). */
interface RowActions {
  open: (a: PatroniAction) => void;
  disabled: boolean;
  title: string | undefined;
}

function MembersTable({
  connectionId,
  topo,
  live,
  active,
  selected,
  actions,
}: {
  connectionId: string;
  topo: Topology;
  live: boolean;
  active: boolean;
  selected?: string;
  /** Есть только у кластера Patroni и подключения не для чтения. */
  actions?: RowActions;
}) {
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
              <Glyph className="warn" symbol="⚠" label={t("sidebar.unreachable")} />
              {/* Причина — видимым текстом, а не только в подсказке: её видно без наведения мыши. */}
              {m.error && <div className="member-error muted">{tr(m.error)}</div>}
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
              if (tags.length === 0 && !m.pendingRestart && !m.scheduledRestart) return "—";
              return (
                <>
                  {tags.join(", ")}
                  {m.pendingRestart && <div className="warn">{t("overview.pendingRestart")}</div>}
                  {m.scheduledRestart && <div className="warn">{t("overview.scheduledRestart", { at: formatScheduled(m.scheduledRestart.at) })}</div>}
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
    // Прокручиваемая область достижима с клавиатуры: фокус на ней + стрелки (иначе правые столбцы недоступны).
    <div className="table-scroll" role="region" tabIndex={0} aria-label={t("overview.tableRegion")}>
      <table className="members-table" aria-label={t("sidebar.members")}>
        <thead>
          <tr>
            {columns.map((c) => (
              <th key={c.id} scope="col">
                {c.header}
              </th>
            ))}
            <th scope="col">{t("overview.col.uptime")}</th>
            <th scope="col">{t("overview.col.connections")}</th>
            <th scope="col">{t("overview.col.databases")}</th>
            {actions && <th scope="col">{t("overview.col.actions")}</th>}
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
                <FactsCells connectionId={connectionId} m={m} live={live} active={active} />
                {actions && (
                  <td>
                    {/* Видимый текст — «Restart…», а имя для скринридера называет узел: в столбце их столько, сколько узлов. */}
                    <button
                      type="button"
                      disabled={actions.disabled || !m.reachable}
                      title={actions.title ?? (m.reachable ? undefined : t("errors.actions.nodeUnavailable", { member: m.name }))}
                      aria-label={t("actions.patroni.restartNamed", { member: m.name })}
                      onClick={() => actions.open({ kind: "restart", member: m.name, onlyPending: m.pendingRestart, scheduledAt: null })}
                    >
                      {t("actions.patroni.restart")}
                    </button>
                    {m.scheduledRestart && (
                      <button
                        type="button"
                        disabled={actions.disabled}
                        title={actions.title}
                        aria-label={t("actions.patroni.cancelRestartNamed", { member: m.name })}
                        onClick={() => actions.open({ kind: "cancelRestart", member: m.name })}
                      >
                        {t("actions.patroni.cancelRestart")}
                      </button>
                    )}
                  </td>
                )}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** active — вкладка видна; скрытая вкладка остаётся смонтированной, но узлы не опрашивает. */
export function Overview({ connectionId, member, active = true }: { connectionId: string; member?: string; active?: boolean }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const journalId = useId();
  const conn = useConnectionProfile(connectionId);
  // Действия Patroni: профиль нужен диалогу (окружение, имя для подтверждения) и условию показа кнопок.
  const actor = conn !== undefined && canAct(conn) ? conn : undefined;
  const [patroniAction, setPatroniAction] = useState<{ id: number; action: PatroniAction } | null>(null);
  const patroniSeq = useRef(0);
  // Пока действие выполняется, другой диалог его не вытесняет (результат не должен потеряться), а кнопки неактивны.
  // Область ключа мутации — своя у каждой вкладки: действие другой вкладки этой же не мешает.
  const patroniScope = useId();
  const acting = useIsMutating({ mutationKey: ["patroniAction", connectionId, patroniScope] }) > 0;
  const openPatroni = (action: PatroniAction) => {
    if (acting) return;
    setPatroniAction({ id: ++patroniSeq.current, action });
  };
  const view = useTopology(connectionId);
  const events = useTopologyEvents(connectionId);
  const journal = events.data ?? [];
  const journalKeys = eventKeys(journal);
  const [error, setError] = useState<AppError | null>(null);
  // Ошибка ключа хоста, от диалога доверия по которой пользователь отказался: баннер остаётся, диалог — нет.
  const [trustDismissed, setTrustDismissed] = useState<AppError | null>(null);
  const [busy, setBusy] = useState(false);

  const connect = async () => {
    setBusy(true);
    setError(null);
    setTrustDismissed(null);
    try {
      qc.setQueryData(["topology", connectionId], await api.connect(connectionId));
      // Как в дереве: запросы каталога после отключения удалены, вкладки объектов перечитываются.
      refreshAfterConnect(qc, connectionId);
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(false);
    }
  };

  /**
   * «Переподключить» живое подключение: пулы с запомненным отказом во входе сбрасываются, узлы опрашиваются сразу,
   * журнал и узел каталога остаются. «Подключиться» у живого подключения ничего не меняет, а у failed/disconnected
   * и так сбрасывает пулы и открывает подключение заново — поэтому эта кнопка только у живого.
   */
  const reopen = async () => {
    setBusy(true);
    setError(null);
    setTrustDismissed(null);
    try {
      const v = await api.reconnect(connectionId);
      qc.setQueryData(["topology", connectionId], v);
      // Туннель, который не поднялся из-за неизвестного ключа хоста, оставляет подключение открытым (degraded) с этой
      // ошибкой в виде, а не отказом команды: предложить доверие нужно и здесь, по действию пользователя.
      if (hostKeyUnknown(v.error)) setError(v.error);
      // Подключение то же, события Disconnected нет: факты узлов и каталог после отказа во входе перечитываются сами.
      refreshAfterConnect(qc, connectionId);
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(false);
    }
  };

  const state = view.data?.state;
  // Спец. P1 §7: подключение открывается при открытии вкладки. Решает первый загруженный вид, один раз на подключение
  // вкладки (ref переживает двойной монтаж StrictMode): явное «Отключиться» при открытой вкладке подключение не
  // возвращает — для этого остаётся кнопка. `failed` и `connecting` сами по себе подключение не запускают.
  const autoConnected = useRef<string | null>(null);
  useEffect(() => {
    if (state === undefined || autoConnected.current === connectionId) return;
    autoConnected.current = connectionId;
    if (state === "disconnected") void connect();
    // connect замыкает только connectionId и setState: ждать его смены не нужно.
  }, [state, connectionId]);

  // Ошибка попытки подключения устаревает, когда подключение стало живым (например, из дерева).
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
  /**
   * Повтор после доверия ключу хоста: сам Rust ничего не повторяет (туннель на стоянке ждёт «Переподключить»).
   * Открытое подключение переподключается на месте, закрытое или упавшее — подключается заново.
   */
  const retryAfterTrust = () => {
    setError(null);
    void (live ? reopen() : connect());
  };
  // Диалог доверия — только для ошибки, которой можно доверять, по собственному действию (не по фоновому событию)
  // и на видимой вкладке: скрытая остаётся смонтированной и не должна открывать окно поверх чужой.
  const trustError = active && hostKeyUnknown(error) && error !== trustDismissed ? error : null;
  const shownError = error ?? v.error;
  // У одиночного сервера один узел и «лидера» нет: в шапке его роль (как значок в дереве). У остальных видов лидер
  // показывается всегда; без него — «—», чтобы отсутствие лидера было видно, а не пропадала сама строка.
  const isServer = topo?.kind === "server";
  const head = topo && !isServer ? leaders(topo.members) : [];
  const serverRole = isServer ? topo.members[0]?.role : undefined;
  // Кнопки действий: только у кластера Patroni и подключения не для чтения. Без логина API Patroni они неактивны с
  // подсказкой, что указать; при закрытом подключении действовать не на чем (и предпросмотр не прочитается).
  const patroniActions = topo?.kind === "patroni" && actor !== undefined;
  const noLogin = !conn?.patroni?.username;
  const actionTitle = noLogin ? t("actions.patroni.authRequired") : undefined;
  const actionsOff = !live || acting || noLogin;
  const rowActions: RowActions | undefined = patroniActions ? { open: openPatroni, disabled: actionsOff, title: actionTitle } : undefined;

  return (
    <div className="page">
      <div className="toolbar">
        <h2>{conn?.name ?? t("tabs.unknownConnection")}</h2>
        <span className="muted">{conn ? t(`connections.form.kinds.${conn.kind}`) : ""}</span>
        {topo?.scope && <span className="muted">{t("overview.scope", { scope: topo.scope })}</span>}
        {topo?.paused && <span className="warn">{t("overview.paused")}</span>}
        {topo?.scheduledSwitchover && (
          <p className="warn toolbar-note">
            {t("overview.scheduledSwitchover", {
              at: formatScheduled(topo.scheduledSwitchover.at),
              from: topo.scheduledSwitchover.from ?? "—",
              to: topo.scheduledSwitchover.to ?? "—",
            })}
            {patroniActions && (
              <>
                {" "}
                <button type="button" disabled={actionsOff} title={actionTitle} onClick={() => openPatroni({ kind: "cancelSwitchover" })}>
                  {t("actions.patroni.cancel")}
                </button>
              </>
            )}
          </p>
        )}
        {topo && !isServer && (
          <span>
            {t("overview.head", {
              role: roleLabel(head[0]?.role ?? "primary", topo.kind),
              names: head.length > 0 ? head.map((m) => m.name).join(", ") : "—",
            })}
          </span>
        )}
        {serverRole !== undefined && serverRole !== "unknown" && <span className="kind-badge">{roleLabel(serverRole, "server")}</span>}
        <span>{t(`sidebar.state.${v.state}`)}</span>
        {(v.state === "disconnected" || v.state === "failed") && (
          <button type="button" className="primary" disabled={busy} onClick={() => void connect()}>
            {t("sidebar.connect")}
          </button>
        )}
        {/* Только при частичной недоступности: у connected повода нет, при connecting подключение ещё открывается. */}
        {v.state === "degraded" && (
          <button type="button" disabled={busy} onClick={() => void reopen()}>
            {t("sidebar.reopen")}
          </button>
        )}
        {patroniActions && (
          <>
            <button type="button" disabled={actionsOff} title={actionTitle} onClick={() => openPatroni({ kind: "switchover", candidate: null, scheduledAt: null })}>
              {t("actions.patroni.switchover")}
            </button>
            <button type="button" disabled={actionsOff} title={actionTitle} onClick={() => openPatroni({ kind: "failover", candidate: "" })}>
              {t("actions.patroni.failover")}
            </button>
            <button type="button" disabled={actionsOff} title={actionTitle} onClick={() => openPatroni({ kind: "pause", paused: topo.paused !== true })}>
              {t(topo.paused === true ? "actions.patroni.resume" : "actions.patroni.pause")}
            </button>
          </>
        )}
      </div>
      {view.error && <ErrorBanner error={view.error} onRetry={() => void view.refetch()} />}
      {shownError && <ErrorBanner error={shownError} />}
      {trustError && <TrustHostKeyDialog error={trustError} onTrusted={retryAfterTrust} onCancel={() => setTrustDismissed(trustError)} />}
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
      {patroniAction && actor && patroniActions && (
        <PatroniActionDialog key={patroniAction.id} connectionId={connectionId} profile={actor} action={patroniAction.action} scope={patroniScope} onClose={() => setPatroniAction(null)} />
      )}
      {topo && <MembersTable connectionId={connectionId} topo={topo} live={live} active={active} selected={member} actions={rowActions} />}
      {topo && (
        <section aria-labelledby={journalId}>
          <h3 id={journalId}>{t("overview.journal")}</h3>
          {journal.length === 0 ? (
            <p className="muted">{t("overview.journalEmpty")}</p>
          ) : (
            <ul className="steps">
              {journal.map((e, i) => (
                <li key={journalKeys[i]}>
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
