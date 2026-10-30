import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ExplainResult, Member, QueryDetail } from "../../api/types";
import { PlanRunner } from "./plan/PlanRunner";

/** Выбранный источник текста: нормализованный pgss или выполняющийся экземпляр (его текст запоминается при выборе). */
type Source = { id: "pgss" } | { id: string; pid: number; sql: string };

const runningId = (pid: number) => `running:${pid}`;

/**
 * Подвкладка «План» (спец. P3 §8.2). Источник текста: нормализованный pgss или текст выполняющегося экземпляра (с
 * литералами; обрезанный — нельзя, пункт серый). Текст экземпляра запоминается при выборе: экземпляры перечитываются
 * каждые 5 с, и закончившийся запрос не должен подменить текст формы под рукой у пользователя. `members` — участники
 * топологии (выбор реплики для ANALYZE чтения); `autoRun`/`onAutoRun` — «Получить план» обзора (см. `PlanRunner`).
 */
export function QueryPlan({
  connectionId,
  member,
  database,
  detail,
  members,
  onPlanResult,
  autoRun,
  onAutoRun,
}: {
  connectionId: string;
  member: string;
  database: string;
  detail: QueryDetail;
  members: Member[] | null;
  onPlanResult: (r: ExplainResult) => void;
  autoRun?: boolean;
  onAutoRun?: () => void;
}) {
  const { t } = useTranslation();
  const [source, setSource] = useState<Source>({ id: "pgss" });
  const pick = (id: string) => {
    if (id === "pgss") {
      setSource({ id: "pgss" });
      return;
    }
    const r = detail.running.find((x) => runningId(x.pid) === id);
    if (r && !r.truncated) setSource({ id, pid: r.pid, sql: r.query });
  };
  const sql = "sql" in source ? source.sql : detail.text;
  // Выбранный экземпляр уже закончился: пункт остаётся в списке, текст — тот, что был выбран.
  const finished = "pid" in source && !detail.running.some((r) => runningId(r.pid) === source.id) ? source : null;
  return (
    <div className="query-plan">
      <label className="inline">
        <span>{t("diag.query.source")}</span>
        <select aria-label={t("diag.query.source")} value={source.id} onChange={(e) => pick(e.target.value)}>
          <option value="pgss">{t("diag.query.sourcePgss")}</option>
          {detail.running.map((r) => (
            <option key={r.pid} value={runningId(r.pid)} disabled={r.truncated}>
              {t("diag.query.sourceRunning", { pid: r.pid })}
              {r.truncated ? ` — ${t("diag.query.sourceTruncated")}` : ""}
            </option>
          ))}
          {finished && <option value={finished.id}>{t("diag.query.sourceRunning", { pid: finished.pid })}</option>}
        </select>
      </label>
      {sql ? (
        <>
          <pre className="query-text" role="group" tabIndex={0} aria-label={t("diag.query.text")}>
            <code>{sql}</code>
          </pre>
          <PlanRunner
            key={source.id}
            connectionId={connectionId}
            member={member}
            database={database}
            sql={sql}
            serverVersionNum={detail.serverVersionNum}
            inRecovery={detail.inRecovery}
            members={members}
            onResult={onPlanResult}
            autoRun={autoRun}
            onAutoRun={onAutoRun}
          />
        </>
      ) : (
        <p className="muted">{t("diag.query.gone")}</p>
      )}
    </div>
  );
}
