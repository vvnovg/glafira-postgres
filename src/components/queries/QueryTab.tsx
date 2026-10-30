import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { ExplainResult } from "../../api/types";
import { useTopology } from "../../lib/useConnectionEvents";
import { useQueryDetail } from "../../lib/useQueryDetail";
import { useStatementsSampler } from "../../lib/useStatementsSampler";
import { tabId, useWorkspace } from "../../state/workspace";
import { SubTabs } from "../catalog/SubTabs";
import { ErrorBanner } from "../common/ErrorBanner";
import { CopyLlmButton } from "./CopyLlmButton";
import { QueryOverview } from "./QueryOverview";
import { QueryPlan } from "./QueryPlan";
import { queryTitle } from "./queriesText";

type Section = "overview" | "plan";

/**
 * Вкладка запроса (спец. P3 §7): шапка с базой, `queryId`, узлом, «Обновить» и «Скопировать для LLM», подвкладки «Обзор» и
 * «План». Подписана на сэмплер pgss узла (история копится, пока открыта хотя бы одна вкладка); таблицы запроса — из
 * последнего плана, хранятся в состоянии вкладки. «Скопировать для LLM» получает последний план, только если он получен на
 * узле вкладки (план реплики к узлу вкладки не относится — тогда Rust строит пробный план сам). Подвкладка «План» после первого открытия остаётся смонтированной
 * (скрытой на «Обзоре»): значения параметров, история и показанный план живут, пока открыта вкладка. Заголовок вкладки,
 * открытой без текста (из находки), становится началом текста, когда тот прочитан. Узла больше нет в топологии —
 * заглушка с выбором другого узла (открывает запрос там, как и переключатель узла), карточка и сэмплер не опрашиваются.
 */
export function QueryTab({ connectionId, member, database, queryId, active }: { connectionId: string; member: string; database: string; queryId: string; active: boolean }) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const retitle = useWorkspace((s) => s.retitle);
  const id = tabId({ kind: "query", connectionId, member, database, queryId });
  const title = useWorkspace((s) => {
    const tab = s.tabs.find((x) => x.id === id);
    return tab?.kind === "query" ? tab.title : undefined;
  });
  const view = useTopology(connectionId);
  const live = view.data?.state === "connected" || view.data?.state === "degraded";
  const topo = view.data?.topology ?? null;
  const gone = topo !== null && !topo.members.some((m) => m.name === member);
  const sampler = useStatementsSampler(connectionId, member, live && !gone);
  const q = useQueryDetail(connectionId, member, database, queryId, { active, live: live && !gone, tickAtMs: sampler.live?.atMs ?? null });
  const [section, setSection] = useState<Section>("overview");
  const [planOpened, setPlanOpened] = useState(false);
  /** Последний план вкладки и узел, где он получен (ANALYZE чтения с лидера идёт на реплику). */
  const [last, setLast] = useState<{ result: ExplainResult; member: string } | null>(null);
  const onPlanResult = useCallback((result: ExplainResult, on: string) => setLast({ result, member: on }), []);
  /** «Получить план»: подвкладка «План» запустит пробу, как только сможет, и снимет запрос. */
  const [autoPlan, setAutoPlan] = useState(false);
  const autoTaken = useCallback(() => setAutoPlan(false), []);
  const detail = q.data;
  const text = detail?.text ?? null;
  const members = topo && topo.kind !== "server" ? topo.members : null;
  useEffect(() => {
    // Свой заголовок у вкладки уже есть (начало текста из отчёта) — не трогаем; без него (только queryId) — ставим.
    if (text !== null && (title === undefined || title === queryId)) retitle(id, queryTitle(text, queryId));
  }, [text, title, id, queryId, retitle]);
  const show = (s: Section) => {
    setSection(s);
    if (s === "plan") setPlanOpened(true);
  };
  const getPlan = () => {
    show("plan");
    setAutoPlan(true);
  };
  const switchMember = (m: string) => open({ kind: "query", connectionId, member: m, database, queryId, title: queryTitle(text, queryId) });

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
  if (gone) {
    return (
      <div className="page query-tab">
        <div className="toolbar">
          <h2>{t("diag.query.heading")}</h2>
          <span className="muted mono">
            {database} · {queryId}
          </span>
        </div>
        <p className="diag-note" role="status">
          {t("diag.query.memberGone", { member })}
        </p>
        {topo.members.length > 0 && (
          <label className="inline">
            <span>{t("diag.query.openOn")}</span>
            <select aria-label={t("diag.query.openOn")} value="" onChange={(e) => e.target.value && switchMember(e.target.value)}>
              <option value="" disabled>
                —
              </option>
              {topo.members.map((m) => (
                <option key={m.name} value={m.name}>
                  {m.name}
                </option>
              ))}
            </select>
          </label>
        )}
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
        <button type="button" onClick={() => void q.refetch()}>
          {t("diag.header.refresh")}
        </button>
        <CopyLlmButton connectionId={connectionId} member={member} database={database} source={{ kind: "query", queryId }} plan={last && last.member === member ? last.result.plan : null} />
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
                  relations={last?.result.relations ?? null}
                  onPlan={() => show("plan")}
                  onGetPlan={getPlan}
                  onConsole={() => open({ kind: "explain", connectionId, member, database, sql: detail.text ?? "" })}
                />
              )}
              {(s === "plan" || planOpened) && (
                <div hidden={s !== "plan"}>
                  <QueryPlan connectionId={connectionId} member={member} database={database} detail={detail} members={topo?.members ?? null} onPlanResult={onPlanResult} autoRun={autoPlan} onAutoRun={autoTaken} />
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
