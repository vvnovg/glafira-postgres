import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../api/client";
import type { AppError, DiagReport, DiagTool } from "../../api/types";
import { tr } from "../../i18n/text";
import { DEFAULT_WAITS_WINDOW_S, defaultIntervalS, isClusterTool, useDiagnostic } from "../../lib/useDiagnostic";
import { refreshAfterConnect, useTopology } from "../../lib/useConnectionEvents";
import { useWorkspace } from "../../state/workspace";
import { ErrorBanner } from "../common/ErrorBanner";
import { DiagHeader } from "./DiagHeader";
import { FailoverView } from "./FailoverView";
import { FindingsPanel } from "./FindingsPanel";
import { HealthView } from "./HealthView";
import { HorizonView } from "./HorizonView";
import { LocksView } from "./LocksView";
import { PulseView } from "./PulseView";
import { ReplicationView } from "./ReplicationView";
import { SessionsView } from "./SessionsView";
import { SlotsView } from "./SlotsView";
import { WaitsView } from "./WaitsView";

/**
 * Тело инструмента: вид данных выбирается по `report.data.kind`.
 * `tool` отличает вкладку `idleTx` от `sessions`: данные у них одни, а набор сессий разный.
 */
function ToolBody({ tool, report, connectionId }: { tool: DiagTool; report: DiagReport; connectionId: string }) {
  switch (report.data.kind) {
    case "pulse":
      return <PulseView connectionId={connectionId} report={report} />;
    case "sessions":
      return <SessionsView key={tool} report={report} preset={tool === "idleTx" ? "idleTx" : "all"} />;
    case "locks":
      return <LocksView report={report} />;
    case "horizon":
      return <HorizonView connectionId={connectionId} report={report} />;
    case "replication":
      return <ReplicationView connectionId={connectionId} report={report} />;
    case "slots":
      return <SlotsView connectionId={connectionId} report={report} />;
    case "failover":
      return <FailoverView connectionId={connectionId} report={report} />;
    case "health":
      return <HealthView connectionId={connectionId} report={report} />;
    case "waits":
      return <WaitsView connectionId={connectionId} report={report} />;
  }
}

/**
 * Вкладка инструмента диагностики (спец. P2 §7.2): шапка с выбором узла и интервала, находки, недоступное и тело
 * инструмента. Узел (`null` — «за лидером») целиком определяет спека вкладки: выбор в шапке записывается в неё
 * (`open`), и повторное открытие вкладки из ссылки или клетки пульса с любым узлом всегда действует. `active` —
 * вкладка видна (скрытая не опрашивает сервер). `tabId` вкладки нужен справке (F1 открывает главу инструмента).
 */
export function DiagTab({ connectionId, tool, member, active }: { connectionId: string; tool: DiagTool; member: string | null; active: boolean; tabId: string }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const view = useTopology(connectionId);
  const [intervalS, setIntervalS] = useState<number | null>(defaultIntervalS(tool));
  // Окно профиля ожиданий (выбор — в шапке); у остальных инструментов не используется.
  const [windowS, setWindowS] = useState(DEFAULT_WAITS_WINDOW_S);
  const [connectError, setConnectError] = useState<AppError | null>(null);
  const [busy, setBusy] = useState(false);
  // Лидер, на котором прошлый отчёт «за лидером» был получен; смена объявляется строкой, пока пользователь не выберет узел сам.
  const lastLeader = useRef<string | null>(null);
  const [moved, setMoved] = useState<{ from: string; to: string } | null>(null);

  const cluster = isClusterTool(tool);
  const state = view.data?.state;
  const live = state === "connected" || state === "degraded";
  // Отчёты кластера узла не берут; остальные ждут выбранный узел (null — лидер).
  const q = useDiagnostic(connectionId, tool, cluster ? null : member, {
    active,
    intervalS,
    windowS: tool === "waits" ? windowS : undefined,
    live,
  });
  const report = q.data;

  // Выбор узла в шапке — то же открытие вкладки, что и из ссылки находки: спека (и узел вместе с ней) обновляется.
  const pick = (m: string | null) => useWorkspace.getState().open({ kind: "diag", connectionId, tool, member: m });

  // Смена узла в спеке (выбор в шапке или повторное открытие) — не смена лидера: объявление сбрасывается.
  // Эффект стоит раньше эффекта лидера: тот видит уже сброшенный `lastLeader`.
  const prevMember = useRef(member);
  useEffect(() => {
    if (prevMember.current === member) return;
    prevMember.current = member;
    lastLeader.current = null;
    setMoved(null);
  }, [member]);

  // Placeholder — отчёт прошлого узла на время загрузки нового: лидером «за лидером» он не считается.
  const placeholder = q.isPlaceholderData;
  useEffect(() => {
    if (!report || placeholder || cluster || !report.followsLeader || report.member === null) return;
    const prev = lastLeader.current;
    lastLeader.current = report.member;
    if (prev !== null && prev !== report.member) setMoved({ from: prev, to: report.member });
  }, [report, placeholder, cluster]);

  const connect = async () => {
    setBusy(true);
    setConnectError(null);
    try {
      qc.setQueryData(["topology", connectionId], await api.connect(connectionId));
      // Данные подключения, прочитанные раньше (в том числе отчёты инструментов), перечитываются.
      refreshAfterConnect(qc, connectionId);
    } catch (e) {
      setConnectError(toAppError(e));
    } finally {
      setBusy(false);
    }
  };

  if (view.data === undefined) {
    return view.error ? <ErrorBanner error={view.error} onRetry={() => void view.refetch()} /> : <p className="empty">{t("common.loading")}</p>;
  }
  if (!live) {
    return (
      <div className="page diag-tab">
        <DiagHeader tool={tool} />
        {state === "connecting" ? (
          <p className="muted" role="status">
            {t("sidebar.state.connecting")}
          </p>
        ) : (
          <div className="diag-closed">
            <p className="muted">{t("diag.closed")}</p>
            <button type="button" className="primary" disabled={busy} onClick={() => void connect()}>
              {t("diag.connect")}
            </button>
          </div>
        )}
        {(connectError ?? view.data.error) && <ErrorBanner error={connectError ?? view.data.error} />}
      </div>
    );
  }

  const topo = view.data.topology;
  // Выбор узла — у узловых инструментов; у одиночного сервера узел один, у инструментов кластера узел не берётся.
  const members = !cluster && topo && topo.kind !== "server" ? topo.members : null;
  return (
    <div className="page diag-tab">
      <DiagHeader
        tool={tool}
        controls={{
          members,
          kind: topo?.kind ?? "server",
          selected: member,
          leader: report?.followsLeader ? report.member : null,
          onSelect: pick,
          intervalS,
          onInterval: setIntervalS,
          windowS: tool === "waits" ? windowS : null,
          onWindow: setWindowS,
          onRefresh: () => void q.refetch(),
          observedAtMs: report?.observedAtMs ?? null,
          stale: q.isError && report !== undefined,
        }}
      />
      {moved && (
        <p className="diag-note" role="status">
          {t("diag.header.nodeChanged", { from: moved.from, to: moved.to })}
        </p>
      )}
      {q.error && <ErrorBanner error={q.error} />}
      {report ? (
        <div className={placeholder ? "diag-body diag-pending" : "diag-body"} aria-busy={placeholder}>
          <FindingsPanel connectionId={connectionId} findings={report.findings} showMember={cluster} />
          {report.unavailable.length > 0 && (
            <div className="diag-unavailable">
              {report.unavailable.map((u, i) => (
                <p key={`${u.part}:${u.member ?? ""}:${i}`} className="muted">
                  {u.member ? t("diag.unavailable.onMember", { member: u.member, text: tr(u.reason) }) : tr(u.reason)}
                </p>
              ))}
            </div>
          )}
          <ToolBody tool={tool} report={report} connectionId={connectionId} />
        </div>
      ) : (
        !q.error && <p className="empty">{t("common.loading")}</p>
      )}
    </div>
  );
}
