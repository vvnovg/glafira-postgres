import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ExplainResult } from "../../api/types";
import { useTopology } from "../../lib/useConnectionEvents";
import { useQueryDetail } from "../../lib/useQueryDetail";
import { useStatementsSampler } from "../../lib/useStatementsSampler";
import { useWorkspace } from "../../state/workspace";
import { SubTabs } from "../catalog/SubTabs";
import { ErrorBanner } from "../common/ErrorBanner";
import { CopyLlmButton } from "./CopyLlmButton";
import { QueryOverview } from "./QueryOverview";
import { QueryPlan } from "./QueryPlan";
import { queryTitle } from "./queriesText";

type Section = "overview" | "plan";

/**
 * Вкладка запроса (спец. P3 §7): шапка с базой, `queryId`, узлом и «Скопировать для LLM», подвкладки «Обзор» и «План».
 * Подписана на сэмплер pgss узла (история копится, пока открыта хотя бы одна вкладка); таблицы запроса — из последнего
 * плана, хранятся в состоянии вкладки. Подвкладка «План» после первого открытия остаётся смонтированной (скрытой на
 * «Обзоре»): значения параметров, история и показанный план живут, пока открыта вкладка.
 */
export function QueryTab({ connectionId, member, database, queryId, active }: { connectionId: string; member: string; database: string; queryId: string; active: boolean }) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const view = useTopology(connectionId);
  const live = view.data?.state === "connected" || view.data?.state === "degraded";
  const sampler = useStatementsSampler(connectionId, member, live);
  const q = useQueryDetail(connectionId, member, database, queryId, { active, live, tickAtMs: sampler.live?.atMs ?? null });
  const [section, setSection] = useState<Section>("overview");
  const [planOpened, setPlanOpened] = useState(false);
  const [last, setLast] = useState<ExplainResult | null>(null);
  const detail = q.data;
  const topo = view.data?.topology ?? null;
  const members = topo && topo.kind !== "server" ? topo.members : null;
  const show = (s: Section) => {
    setSection(s);
    if (s === "plan") setPlanOpened(true);
  };
  const switchMember = (m: string) => open({ kind: "query", connectionId, member: m, database, queryId, title: queryTitle(detail?.text ?? null, queryId) });

  if (view.data === undefined) {
    return <div className="page">{view.error ? <ErrorBanner error={view.error} onRetry={() => void view.refetch()} /> : <p className="empty">{t("common.loading")}</p>}</div>;
  }
  if (!live) {
    return (
      <div className="page">
        <p className="muted">{t("diag.closed")}</p>
      </div>
    );
  }
  return (
    <div className="page query-tab">
      <div className="toolbar">
        <h2>{t("diag.query.heading")}</h2>
        <span className="muted mono">
          {database} · {queryId}
        </span>
        {members && (
          <label className="inline">
            <span>{t("diag.header.node")}</span>
            <select aria-label={t("diag.header.node")} value={member} onChange={(e) => switchMember(e.target.value)}>
              {members.map((m) => (
                <option key={m.name} value={m.name}>
                  {m.name}
                </option>
              ))}
            </select>
          </label>
        )}
        <CopyLlmButton connectionId={connectionId} member={member} database={database} source={{ kind: "query", queryId }} plan={last?.plan ?? null} />
      </div>
      {q.error && <ErrorBanner error={q.error} onRetry={() => void q.refetch()} />}
      {sampler.error && <ErrorBanner error={sampler.error} />}
      {detail && (
        <SubTabs<Section>
          label={t("diag.query.sections")}
          tabs={[
            { id: "overview", label: t("diag.query.overview") },
            { id: "plan", label: t("diag.query.plan") },
          ]}
          value={section}
          onChange={show}
        >
          {(s) => (
            <>
              {s === "overview" && (
                <QueryOverview
                  connectionId={connectionId}
                  member={member}
                  database={database}
                  detail={detail}
                  relations={last?.relations ?? null}
                  onPlan={() => show("plan")}
                  onConsole={() => open({ kind: "explain", connectionId, member, database, sql: detail.text ?? "" })}
                />
              )}
              {(s === "plan" || planOpened) && (
                <div hidden={s !== "plan"}>
                  <QueryPlan connectionId={connectionId} member={member} database={database} detail={detail} onPlanResult={setLast} />
                </div>
              )}
            </>
          )}
        </SubTabs>
      )}
      {!detail && !q.error && <p className="empty">{t("common.loading")}</p>}
    </div>
  );
}
