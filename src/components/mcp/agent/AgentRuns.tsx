import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Fragment, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import type { AgentRun } from "../../../api/types";
import { formatDuration, formatTimestamp } from "../../../lib/format";
import { AGENT_RUNS_KEY, isActive, type RunsState } from "../../../lib/useAgentRuns";
import { ErrorBanner } from "../../common/ErrorBanner";
import { AgentSummary } from "./AgentSummary";
import { RunCost } from "./RunCost";
import { agentLabel, runResultText, runSeconds } from "./agentText";

/** Сколько последних запусков подраздел показывает сразу (спец. P4c §8.3, смоук P4c R25); остальные — «Показать все». */
export const RUNS_SHOWN = 10;

/**
 * «Запуски» подраздела «Фоновый ассистент» (спец. P4c §8.3): последние 10 запусков всех подключений, новые первыми, и
 * «Показать все (N)» / «Свернуть» (смоук P4c, R25: при 20+ запусках блок не вытесняет остальное), — время постановки в
 * очередь, подключение, запрос (у «Проверить» — «Проверка»), агент и модель, итог, длительность, вызовы, стоимость (если CLI её
 * сообщил, или оценка облачного запуска через Ollama — `RunCost`; не переносится). Действия стоят под итогом в его ячейке, а
 * не отдельной колонкой, а таблица шире блока прокручивается внутри него с видимой полосой (R26: в узком окне колонка
 * «Стоимость» обрезалась). «Ответ ассистента» раскрывает итог (`AgentSummary`: без разметки, свёрнут до пяти строк), «Показать
 * журнал» — у начавшихся запусков, «Остановить» — у ждущего и идущего (запрос остаётся ждущим; итог `cancelled` приходит
 * событием).
 * Подписи кнопок с id запуска — для скринридера. Пока список читается — «Загрузка…», не прочитался — ошибка с «Повторить»
 * (`state`, `error`, `onRetry` — из `useAgentRuns`), а не «Запусков пока нет». «Остановить» во время записи — `aria-disabled`;
 * когда кнопка исчезает (запуск остановлен), фокус переходит на «Показать журнал» того же запуска или на заголовок списка.
 */
export function AgentRuns({
  runs,
  state = "ready",
  error,
  onRetry,
  onShowLog,
}: {
  runs: AgentRun[];
  state?: RunsState;
  error?: unknown;
  onRetry?: () => void;
  onShowLog: (runId: string) => void;
}) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const connections = useQuery({ queryKey: ["connections"], queryFn: () => api.listConnections() });
  const stop = useMutation({
    mutationFn: (runId: string) => api.stopAgentRun(runId),
    onSuccess: () => void qc.invalidateQueries({ queryKey: AGENT_RUNS_KEY, exact: true }),
    // Не остановился — кнопка остаётся, и переводить фокус потом незачем (иначе он прыгнул бы с <body> позже).
    onError: () => {
      refocus.current = null;
    },
  });
  const [open, setOpen] = useState<string | null>(null);
  const [all, setAll] = useState(false);
  const tableId = useId();
  const tableRef = useRef<HTMLTableElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  // «Остановить» нажата: когда её кнопка исчезнет и фокус окажется на <body>, он перейдёт на журнал запуска или заголовок.
  const refocus = useRef<string | null>(null);
  useEffect(() => {
    const id = refocus.current;
    if (id === null) return;
    const a = document.activeElement;
    if (a === null || a === document.body) {
      refocus.current = null;
      (tableRef.current?.querySelector<HTMLElement>(`[data-run-log="${id}"]`) ?? headingRef.current)?.focus();
    } else if (!tableRef.current?.contains(a)) refocus.current = null;
  });
  const shown = all ? runs : runs.slice(0, RUNS_SHOWN);
  // Подключения уже нет (или список не прочитан) — показываем его id.
  const connectionName = (id: string | null | undefined) => (id ? (connections.data?.find((c) => c.id === id)?.name ?? id) : "—");
  const stopRun = (id: string) => {
    if (stop.isPending) return;
    refocus.current = id;
    stop.mutate(id);
  };
  return (
    <>
      <h4 ref={headingRef} tabIndex={-1}>
        {t("settings.mcp.agent.runs.title")}
      </h4>
      {state === "loading" ? (
        <p className="muted">{t("common.loading")}</p>
      ) : state === "failed" ? (
        <ErrorBanner error={error} onRetry={onRetry} />
      ) : shown.length === 0 ? (
        <p className="muted">{t("settings.mcp.agent.runs.none")}</p>
      ) : (
        <div className="table-scroll agent-runs-scroll">
          <table className="agent-runs" id={tableId} aria-label={t("settings.mcp.agent.runs.title")} ref={tableRef}>
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
              </tr>
            </thead>
            <tbody>
              {shown.map((run) => {
                const seconds = runSeconds(run);
                const summaryId = `agent-run-summary-${run.id}`;
                // «2023-11-15 01:15:00»: в узкой таблице дата и время переносятся только между собой.
                const [day, time] = formatTimestamp(run.queuedAtMs).slice(0, 19).split(" ");
                const expanded = open === run.id && !!run.summary;
                const actions = !!run.summary || run.startedAtMs != null || isActive(run);
                return (
                  <Fragment key={run.id}>
                    <tr>
                      <td className="agent-run-when">
                        <span>{day}</span> <span>{time}</span>
                      </td>
                      <td>{run.kind === "check" ? "—" : connectionName(run.connectionId)}</td>
                      {run.kind === "check" ? <td>{t("settings.mcp.agent.runs.check")}</td> : <td className="mono">{run.requestId ?? ""}</td>}
                      <td>{agentLabel(run)}</td>
                      <td>
                        {runResultText(run, t)}
                        {actions && (
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
                              <button
                                type="button"
                                className="link"
                                data-run-log={run.id}
                                aria-label={t("settings.mcp.agent.runs.logAria", { id: run.id })}
                                onClick={() => onShowLog(run.id)}
                              >
                                {t("llmAssist.agent.showLog")}
                              </button>
                            )}
                            {isActive(run) && (
                              <button
                                type="button"
                                className="link"
                                aria-label={t("settings.mcp.agent.runs.stopAria", { id: run.id })}
                                aria-disabled={stop.isPending || undefined}
                                onClick={() => stopRun(run.id)}
                              >
                                {t("settings.mcp.agent.runs.stop")}
                              </button>
                            )}
                          </span>
                        )}
                      </td>
                      <td>{seconds === null ? "—" : formatDuration(seconds)}</td>
                      <td>{run.calls}</td>
                      <td className="agent-run-cost">
                        <RunCost run={run} />
                      </td>
                    </tr>
                    {expanded && (
                      <tr id={summaryId}>
                        <td colSpan={8}>
                          <AgentSummary text={run.summary ?? ""} />
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
      {runs.length > RUNS_SHOWN && state === "ready" && (
        <button type="button" className="link" aria-expanded={all} aria-controls={tableId} onClick={() => setAll(!all)}>
          {all ? t("settings.mcp.agent.runs.showLess") : t("settings.mcp.agent.runs.showAll", { n: runs.length })}
        </button>
      )}
      {stop.error && <ErrorBanner error={stop.error} />}
    </>
  );
}
