import type { TFunction } from "i18next";
import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AgentRun, OptimizationRequest } from "../../../api/types";
import { elapsedText, isActive } from "../../../lib/useAgentRuns";
import { useShownProposals } from "../../../state/shownProposals";
import { AgentLogDialog } from "../../mcp/agent/AgentLogDialog";
import { failReasonText, warningText } from "../../mcp/agent/agentText";

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
  onCancel: () => void;
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

/** Главная строка (спец. P4c §8.1): что с запросом и запуском сейчас; пусто — у отвеченного запроса после конца запуска. */
function phaseText(request: OptimizationRequest, run: AgentRun | null, ahead: number, t: TFunction): string {
  const waiting = request.status === "waiting";
  if (run === null || run.status === "skipped") return waiting ? t("llmAssist.ask.waiting", { id: request.id }) : "";
  if (run.status === "queued") return ahead > 0 ? t("llmAssist.agent.queued", { n: ahead }) : t("llmAssist.agent.queuedNext");
  if (run.status === "starting") return t("llmAssist.agent.starting");
  if (run.status === "running") return t("llmAssist.agent.running");
  if (!waiting) return "";
  switch (run.status) {
    case "done":
      return t("llmAssist.agent.noProposals");
    case "failed":
      return t("llmAssist.agent.failed", { reason: failReasonText(run, t) });
    case "cancelled":
      return t("llmAssist.agent.stopped");
    default:
      return t("llmAssist.agent.interrupted");
  }
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
 * сообщение — своей строкой (смоук P4b, Z1): фаза, предупреждения Ollama, «Ассистент прислал N предложений» и итог
 * копирования — в живой области; ход работы (время, вызовы, инструмент) — отдельной строкой вне её, чтобы скринридер не
 * читал его каждую секунду. Действия — ссылками отдельным рядом, вне живой области: «Ответ ассистента» (итог простым
 * текстом, без разметки), «Показать журнал», «Повторить» / «Запустить фонового ассистента», «Скопировать строку для чата»
 * (видна и во время работы: чат и фон работают с одним запросом), «Отменить запрос». О присланных предложениях строка
 * пишет, пока панель «Предложения LLM» их не показала (`useShownProposals`); дальше у отвеченного запроса — обычное
 * состояние (ничего). После «Повторить» кнопка исчезает (запуск снова в очереди), и фокус переходит на саму строку, а не
 * теряется на <body>.
 */
export function AgentRunStatus({ request, run, ahead, chatLine, busy, onRetry, onCancel }: AgentRunStatusProps) {
  const { t } = useTranslation();
  const active = run !== null && isActive(run);
  const now = useNow(run !== null && (run.status === "starting" || run.status === "running"));
  const shown = useShownProposals((s) => s.shown);
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
  const phase = phaseText(request, run, ahead, t);
  const unseen = run !== null && request.status === "answered" && run.proposalIds.some((id) => !shown[id]);
  const arrived = run !== null && unseen ? t("llmAssist.agent.arrived", { count: run.proposalIds.length }) : "";
  if (!phase && !arrived) return null;
  const finished = run !== null && !active && run.status !== "skipped";
  const summary = waiting && run !== null && finished && run.summary ? run.summary : null;

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
        {phase && <p>{phase}</p>}
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
      {waiting && (
        <div className="agent-run-actions">
          {summary && (
            <button type="button" className="link" aria-expanded={summaryOpen} aria-controls={summaryId} onClick={() => setSummaryOpen(!summaryOpen)}>
              {t("llmAssist.agent.summary")}
            </button>
          )}
          {run?.status === "failed" && (
            <button type="button" className="link" onClick={() => setLogOpen(true)}>
              {t("llmAssist.agent.showLog")}
            </button>
          )}
          {finished ? (
            <button type="button" className="link" disabled={busy} onClick={retry}>
              {t("common.retry")}
            </button>
          ) : (
            !active && (
              <button type="button" className="link" disabled={busy} onClick={retry}>
                {t("llmAssist.agent.start")}
              </button>
            )
          )}
          <button type="button" className="link" onClick={() => void copy()}>
            {t("llmAssist.agent.copyLine")}
          </button>
          <button type="button" className="link" disabled={busy} onClick={onCancel}>
            {t("llmAssist.ask.cancel")}
          </button>
        </div>
      )}
      {summary && <div id={summaryId}>{summaryOpen && <p className="llm-text agent-run-summary">{summary}</p>}</div>}
      {logOpen && run !== null && <AgentLogDialog runId={run.id} onClose={() => setLogOpen(false)} />}
    </div>
  );
}
