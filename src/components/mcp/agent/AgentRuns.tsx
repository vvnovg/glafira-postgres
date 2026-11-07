import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Fragment, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import type { AgentRun } from "../../../api/types";
import { currentLocale } from "../../../i18n";
import { formatDuration, formatTimestamp } from "../../../lib/format";
import { AGENT_RUNS_KEY, isActive } from "../../../lib/useAgentRuns";
import { ErrorBanner } from "../../common/ErrorBanner";
import { agentLabel, runResultText, runSeconds } from "./agentText";

/** Сколько последних запусков показывает подраздел (спец. P4c §8.3). */
export const RUNS_SHOWN = 20;

/** Стоимость запуска, как её сообщил CLI: доллары с двумя знаками. */
function cost(usd: number): string {
  return `$${usd.toLocaleString(currentLocale(), { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

/**
 * «Запуски» подраздела «Фоновый ассистент» (спец. P4c §8.3): последние 20 запусков всех подключений, новые первыми —
 * время постановки в очередь, подключение, запрос (у «Проверить» — «Проверка»), агент и модель, итог, длительность, вызовы,
 * стоимость (если CLI её сообщил). «Ответ ассистента» раскрывает итоговый текст простым текстом, «Показать журнал» — у
 * начавшихся запусков, «Остановить» — у ждущего и идущего (запрос остаётся ждущим; итог `cancelled` приходит событием).
 * Подписи кнопок с id запуска — для скринридера.
 */
export function AgentRuns({ runs, onShowLog }: { runs: AgentRun[]; onShowLog: (runId: string) => void }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const connections = useQuery({ queryKey: ["connections"], queryFn: () => api.listConnections() });
  const stop = useMutation({
    mutationFn: (runId: string) => api.stopAgentRun(runId),
    onSuccess: () => void qc.invalidateQueries({ queryKey: AGENT_RUNS_KEY, exact: true }),
  });
  const [open, setOpen] = useState<string | null>(null);
  const shown = runs.slice(0, RUNS_SHOWN);
  // Подключения уже нет (или список не прочитан) — показываем его id.
  const connectionName = (id: string | null | undefined) => (id ? (connections.data?.find((c) => c.id === id)?.name ?? id) : "—");
  return (
    <>
      <h5>{t("settings.mcp.agent.runs.title")}</h5>
      {shown.length === 0 ? (
        <p className="muted">{t("settings.mcp.agent.runs.none")}</p>
      ) : (
        <div className="table-scroll">
          <table className="agent-runs" aria-label={t("settings.mcp.agent.runs.title")}>
            <thead>
              <tr>
                <th>{t("settings.mcp.agent.runs.colTime")}</th>
                <th>{t("settings.mcp.agent.runs.colConnection")}</th>
                <th>{t("settings.mcp.agent.runs.colRequest")}</th>
                <th>{t("settings.mcp.agent.runs.colAgent")}</th>
                <th>{t("settings.mcp.agent.runs.colResult")}</th>
                <th>{t("settings.mcp.agent.runs.colDuration")}</th>
                <th>{t("settings.mcp.agent.runs.colCalls")}</th>
                <th>{t("settings.mcp.agent.runs.colCost")}</th>
                <th>
                  <span className="sr-only">{t("settings.mcp.agent.runs.colActions")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {shown.map((run) => {
                const seconds = runSeconds(run);
                const summaryId = `agent-run-summary-${run.id}`;
                // «2023-11-15 01:15:00»: в узкой таблице дата и время переносятся только между собой.
                const [day, time] = formatTimestamp(run.queuedAtMs).slice(0, 19).split(" ");
                const expanded = open === run.id && !!run.summary;
                return (
                  <Fragment key={run.id}>
                    <tr>
                      <td className="agent-run-when">
                        <span>{day}</span> <span>{time}</span>
                      </td>
                      <td>{run.kind === "check" ? "—" : connectionName(run.connectionId)}</td>
                      {run.kind === "check" ? <td>{t("settings.mcp.agent.runs.check")}</td> : <td className="mono">{run.requestId ?? ""}</td>}
                      <td>{agentLabel(run)}</td>
                      <td>{runResultText(run, t)}</td>
                      <td>{seconds === null ? "—" : formatDuration(seconds)}</td>
                      <td>{run.calls}</td>
                      <td>{run.costUsd != null ? cost(run.costUsd) : ""}</td>
                      <td>
                        <span className="agent-run-actions">
                          {run.summary && (
                            <button
                              type="button"
                              className="link"
                              aria-expanded={expanded}
                              aria-controls={expanded ? summaryId : undefined}
                              aria-label={t("settings.mcp.agent.runs.summaryAria", { id: run.id })}
                              onClick={() => setOpen(expanded ? null : run.id)}
                            >
                              {t("llmAssist.agent.summary")}
                            </button>
                          )}
                          {run.startedAtMs != null && (
                            <button type="button" className="link" aria-label={t("settings.mcp.agent.runs.logAria", { id: run.id })} onClick={() => onShowLog(run.id)}>
                              {t("llmAssist.agent.showLog")}
                            </button>
                          )}
                          {isActive(run) && (
                            <button
                              type="button"
                              className="link"
                              aria-label={t("settings.mcp.agent.runs.stopAria", { id: run.id })}
                              disabled={stop.isPending}
                              onClick={() => stop.mutate(run.id)}
                            >
                              {t("settings.mcp.agent.runs.stop")}
                            </button>
                          )}
                        </span>
                      </td>
                    </tr>
                    {expanded && (
                      <tr id={summaryId}>
                        <td colSpan={9}>
                          <p className="llm-text agent-run-summary">{run.summary}</p>
                        </td>
                      </tr>
                    )}
                  </Fragment>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      {stop.error && <ErrorBanner error={stop.error} />}
    </>
  );
}
