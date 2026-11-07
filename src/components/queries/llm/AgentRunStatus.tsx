import type { TFunction } from "i18next";
import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AgentRun, OptimizationRequest } from "../../../api/types";
import { formatDuration } from "../../../lib/format";
import { elapsedText, isActive } from "../../../lib/useAgentRuns";
import { useClosedResults } from "../../../state/closedResults";
import { AgentLogDialog } from "../../mcp/agent/AgentLogDialog";
import { AgentSummary } from "../../mcp/agent/AgentSummary";
import { failReasonText, runSeconds, warningText } from "../../mcp/agent/agentText";
import { hasCost, RunCost } from "../../mcp/agent/RunCost";

export interface AgentRunStatusProps {
  /** Запрос источника: ждущий или (после ответа) последний. */
  request: OptimizationRequest;
  /** Последний запуск запроса; null — запуска не было. */
  run: AgentRun | null;
  /** Запусков перед этим в общей очереди (идущий и поставленные раньше); важно только для `queued`. */
  ahead: number;
  /** Строка для чата этого запроса (`llmAssist.ask.chatLine`). */
  chatLine: string;
  /** Действие кнопки ещё выполняется: «Повторить» и «Отменить запрос» неактивны. */
  busy: boolean;
  /** «Повторить» и «Запустить фонового ассистента»: новый запуск того же запроса. */
  onRetry: () => void;
  /** «Отменить запрос» / «Закрыть запрос»: ждущий запрос закрывается (с запуском), у отвеченного — строка его итога. */
  onCancel: () => void;
  /** Запрос — к прежнему тексту консоли (смоук P4c, R23): начало его текста; null или нет — к тексту редактора. */
  staleHead?: string | null;
}

/** Текущее время для хода работы: обновляется раз в секунду, пока `ticking`. */
function useNow(ticking: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!ticking) return;
    setNow(Date.now());
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [ticking]);
  return now;
}

/** Итог кончившегося запуска (смоук P4c, R20): прислал N предложений, без предложений, ошибка, остановлен, прерван. */
function resultText(run: AgentRun, t: TFunction): string {
  switch (run.status) {
    case "done":
      return run.proposalIds.length > 0 ? t("llmAssist.agent.arrived", { count: run.proposalIds.length }) : t("llmAssist.agent.noProposals");
    case "failed":
      return t("llmAssist.agent.failed", { reason: failReasonText(run, t) });
    case "cancelled":
      return t("llmAssist.agent.stopped");
    default:
      return t("llmAssist.agent.interrupted");
  }
}

/**
 * Главная строка (спец. P4c §8.1): что с запросом и запуском сейчас — ожидание, очередь, запуск, работа или итог запуска (и у
 * отвеченного запроса, R20); пусто — у отвеченного запроса без запуска или с пропущенным.
 */
function phaseText(request: OptimizationRequest, run: AgentRun | null, ahead: number, t: TFunction): string {
  if (run === null || run.status === "skipped") return request.status === "waiting" ? t("llmAssist.ask.waiting", { id: request.id }) : "";
  if (run.status === "queued") return ahead > 0 ? t("llmAssist.agent.queued", { n: ahead }) : t("llmAssist.agent.queuedNext");
  if (run.status === "starting") return t("llmAssist.agent.starting");
  if (run.status === "running") return t("llmAssist.agent.running");
  return resultText(run, t);
}

/**
 * Ждущий запрос после ошибки, остановки или прерывания запуска (смоук P4c, R15): «Отменить запрос» читалось бы как отмена
 * упавшего запуска — ссылка называется «Закрыть запрос», действие то же. Пока запрос ждёт, стоит в очереди, работает или
 * ассистент закончил без предложений — «Отменить запрос».
 */
function closesAfterFailure(run: AgentRun | null): boolean {
  return run !== null && (run.status === "failed" || run.status === "cancelled" || run.status === "interrupted");
}

/**
 * Строке статуса есть что сказать: иначе `AgentRunStatus` ничего не рисует, а кнопка не отдаёт ей ряд (правило P4b: пустой
 * блок строк ряда не занимает). Ждущий запрос — всегда; у отвеченного — идущий запуск или итог кончившегося (смоук P4c, R20:
 * он остаётся и после того, как панель показала предложения), пока его не закрыли (`closed` — `useClosedResults().closed`).
 * Пропущенный запуск (запрос ответил чат) сказать ничего не может.
 */
export function agentRunVisible(request: OptimizationRequest, run: AgentRun | null, closed: readonly string[]): boolean {
  if (request.status === "waiting") return true;
  if (run === null) return false;
  if (isActive(run)) return true;
  return request.status === "answered" && run.status !== "skipped" && !closed.includes(request.id);
}

/** Ход работы: время с начала запуска, вызовы и последний инструмент; пока процесс запускается — только время. */
function progressText(run: AgentRun, now: number, t: TFunction): string {
  const elapsed = elapsedText(now - (run.startedAtMs ?? now));
  if (run.status === "starting") return elapsed;
  return run.lastTool
    ? t("llmAssist.agent.progressTool", { elapsed, calls: run.calls, tool: run.lastTool })
    : t("llmAssist.agent.progress", { elapsed, calls: run.calls });
}

/**
 * Строка статуса фонового ассистента под «Передать LLM-ассистенту» (спец. P4c §8.1) — в консоли и в карточке. Каждое
 * сообщение — своей строкой (смоук P4b, Z1): запрос к прежнему тексту (R23, `staleHead`: «Запрос r… к прежнему тексту: SELECT
 * …»), фаза, предупреждения Ollama, «Ассистент прислал N предложений» (пока запуск ещё идёт) и итог копирования — в живой
 * области; итог кончившегося запуска — «Ассистент прислал 2 предложения · 48 с · ≈ 0,007 $» (R20: длительность, если она не
 * меньше секунды, и стоимость, `RunCost`, через « · »); ход работы (время, вызовы, инструмент) — отдельной строкой вне её,
 * чтобы скринридер не читал его каждую секунду. Итог остаётся и у отвеченного запроса (R20) — до следующей отправки, до
 * «Закрыть запрос» или до подстановки другого текста (тогда строка называет запрос к прежнему тексту, R23). Действия — ссылками
 * отдельным рядом, вне живой области: «Ответ ассистента» (итог простым текстом без разметки, свёрнут до пяти строк, во всю
 * ширину строки — `AgentSummary`, смоук P4c, R16), «Показать журнал», у ждущего запроса — «Повторить» / «Запустить фонового
 * ассистента» и «Скопировать строку для чата» (видна и во время работы: чат и фон работают с одним запросом), «Отменить
 * запрос» — пока запрос ждёт и запуск не кончился ошибкой; «Закрыть запрос» — после ошибки, остановки или прерывания запуска
 * (R15, `closesAfterFailure`), у запроса к прежнему тексту (R23) и у отвеченного запроса (закрывает строку его итога, `onCancel`).
 * «Показать журнал» — у ошибки начавшегося запуска (`startedAtMs`): журнал создаёт запуск процесса. После «Повторить» кнопка
 * исчезает (запуск снова в очереди), и фокус переходит на саму строку, а не теряется на <body>.
 */
export function AgentRunStatus({ request, run, ahead, chatLine, busy, onRetry, onCancel, staleHead = null }: AgentRunStatusProps) {
  const { t } = useTranslation();
  const active = run !== null && isActive(run);
  const now = useNow(run !== null && (run.status === "starting" || run.status === "running"));
  const closed = useClosedResults((s) => s.closed);
  const [summaryOpen, setSummaryOpen] = useState(false);
  const [copied, setCopied] = useState<boolean | null>(null);
  const [logOpen, setLogOpen] = useState(false);
  const summaryId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  // «Повторить» нажата: когда её кнопка исчезнет и фокус окажется на <body>, он вернётся на строку статуса.
  const refocus = useRef(false);
  useEffect(() => {
    if (!refocus.current) return;
    const a = document.activeElement;
    if (a === null || a === document.body) {
      refocus.current = false;
      rootRef.current?.focus();
    } else if (!rootRef.current?.contains(a)) refocus.current = false;
  });

  const waiting = request.status === "waiting";
  if (!agentRunVisible(request, run, closed)) return null;
  const phase = phaseText(request, run, ahead, t);
  // Предложение пришло, а запуск ещё идёт: своей строкой под фазой; у кончившегося запуска это его итог (`resultText`).
  const arrived = run !== null && active && run.proposalIds.length > 0 ? t("llmAssist.agent.arrived", { count: run.proposalIds.length }) : "";
  const finished = run !== null && !active && run.status !== "skipped";
  const summary = run !== null && finished && run.summary ? run.summary : null;
  // Длительность и стоимость кончившегося запуска — рядом с его итогом (R20): «· 48 с · ≈ 0,01 $»; меньше секунды — без
  // длительности. Оценка — «≈ …» с подсказкой о дне цен, точная стоимость Claude Code без Ollama — как есть (sf-T7).
  const seconds = run !== null && finished ? runSeconds(run) : null;
  const tail =
    run !== null && finished ? (
      <>
        {seconds !== null && seconds >= 1 && ` · ${formatDuration(seconds)}`}
        {hasCost(run) && (
          <>
            {" · "}
            <RunCost run={run} />
          </>
        )}
      </>
    ) : null;
  // Ссылка закрытия: «Отменить запрос» — только у ждущего запроса этого текста, пока запуск не кончился ошибкой.
  const closes = !waiting || staleHead !== null || closesAfterFailure(run);

  const retry = () => {
    refocus.current = true;
    onRetry();
  };
  const copy = async () => {
    let ok = true;
    try {
      if (!navigator.clipboard) throw new Error("clipboard unavailable");
      await navigator.clipboard.writeText(chatLine);
    } catch {
      ok = false;
    }
    setCopied(ok);
  };

  return (
    <div className="agent-run" role="group" aria-label={t("llmAssist.agent.title", { id: request.id })} tabIndex={-1} ref={rootRef}>
      <div role="status" className="agent-run-lines">
        {staleHead !== null && <p>{t("llmAssist.agent.stale", { id: request.id, head: staleHead })}</p>}
        {phase && (
          <p>
            {phase}
            {tail}
          </p>
        )}
        {run !== null &&
          active &&
          run.warnings.map((w, i) => (
            <p key={i} className="diag-warn">
              {warningText(w, t)}
            </p>
          ))}
        {arrived && <p>{arrived}</p>}
        {copied !== null && <p className="muted">{copied ? t("llmAssist.agent.copied", { line: chatLine }) : t("llmAssist.agent.copyFailed", { line: chatLine })}</p>}
      </div>
      {run !== null && (run.status === "starting" || run.status === "running") && <p className="muted agent-run-progress">{progressText(run, now, t)}</p>}
      {(waiting || finished) && (
        <div className="agent-run-actions">
          {summary && (
            <button type="button" className="link" aria-expanded={summaryOpen} aria-controls={summaryId} onClick={() => setSummaryOpen(!summaryOpen)}>
              {t("llmAssist.agent.summary")}
            </button>
          )}
          {/* Журнал создаётся при запуске процесса: у ошибки до старта (политика, MCP выключен…) его нет. */}
          {run?.status === "failed" && run.startedAtMs != null && (
            <button type="button" className="link" onClick={() => setLogOpen(true)}>
              {t("llmAssist.agent.showLog")}
            </button>
          )}
          {waiting &&
            (finished ? (
              <button type="button" className="link" disabled={busy} onClick={retry}>
                {t("common.retry")}
              </button>
            ) : (
              !active && (
                <button type="button" className="link" disabled={busy} onClick={retry}>
                  {t("llmAssist.agent.start")}
                </button>
              )
            ))}
          {waiting && (
            <button type="button" className="link" onClick={() => void copy()}>
              {t("llmAssist.agent.copyLine")}
            </button>
          )}
          <button type="button" className="link" disabled={busy} onClick={onCancel}>
            {closes ? t("llmAssist.ask.close") : t("llmAssist.ask.cancel")}
          </button>
        </div>
      )}
      {summary && (
        <div id={summaryId} className="agent-run-answer">
          {summaryOpen && <AgentSummary text={summary} />}
        </div>
      )}
      {logOpen && run !== null && <AgentLogDialog runId={run.id} onClose={() => setLogOpen(false)} />}
    </div>
  );
}
