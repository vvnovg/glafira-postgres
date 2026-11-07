import { useMutation, useQuery } from "@tanstack/react-query";
import { type KeyboardEvent, useEffect, useId, useRef } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import { trapTab } from "../../../lib/focusTrap";
import { ErrorBanner } from "../../common/ErrorBanner";

/**
 * «Показать журнал» запуска фонового ассистента (спец. P4c §4.5): хвост журнала (до 256 КБ) простым текстом и «Показать
 * файл». Диалог в странице (системный window.confirm в Tauri 2 не показывается): фокус — на контейнер, Tab держится
 * внутри, Escape и «Закрыть» закрывают, фокус возвращается туда, где был. Журнал читается при каждом открытии (кэш не
 * хранится): запуск мог дописать его. Модели журнал не отдаётся.
 */
export function AgentLogDialog({ runId, onClose }: { runId: string; onClose: () => void }) {
  const { t } = useTranslation();
  const log = useQuery({ queryKey: ["agentLog", runId], queryFn: () => api.readAgentLog(runId), gcTime: 0, retry: false });
  const reveal = useMutation({ mutationFn: () => api.revealAgentLog(runId) });
  const rootRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useEffect(() => {
    // Запоминаем элемент в фокусе до того, как диалог его заберёт.
    const returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    rootRef.current?.focus();
    return () => {
      if (returnTo?.isConnected) returnTo.focus();
    };
  }, []);
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Tab") trapTab(rootRef.current, e);
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
    }
  };
  return (
    <div className="modal-backdrop">
      <div className="modal agent-log-dialog" role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1} ref={rootRef} onKeyDown={onKeyDown}>
        <h3 id={titleId}>{t("llmAssist.agent.log.title", { id: runId })}</h3>
        <p className="muted">{t("llmAssist.agent.log.tail")}</p>
        {log.error ? (
          <ErrorBanner error={log.error} onRetry={() => void log.refetch()} />
        ) : log.data === undefined ? (
          <p className="muted">{t("common.loading")}</p>
        ) : log.data === "" ? (
          <p className="muted">{t("llmAssist.agent.log.empty")}</p>
        ) : (
          // tabIndex — длинный журнал прокручивается с клавиатуры.
          <pre className="agent-log" tabIndex={0}>
            {log.data}
          </pre>
        )}
        {reveal.error && <ErrorBanner error={reveal.error} />}
        <div className="toolbar">
          <button type="button" disabled={reveal.isPending} onClick={() => reveal.mutate()}>
            {t("llmAssist.agent.log.reveal")}
          </button>
          <button type="button" onClick={onClose}>
            {t("common.close")}
          </button>
        </div>
      </div>
    </div>
  );
}
