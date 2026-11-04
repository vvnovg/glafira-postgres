import { useMutation, useQueryClient } from "@tanstack/react-query";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import { trapTab } from "../../lib/focusTrap";
import { MCP_STATUS_KEY, useMcpStatus } from "../../lib/useMcpStatus";
import { ErrorBanner } from "../common/ErrorBanner";
import { clientLabel } from "./text";

/**
 * Подтверждение нового MCP-клиента (спец. P4 §4.4) поверх любого экрана (Layout); несколько ожидающих — по очереди.
 * Системный window.confirm в Tauri 2 не показывается, поэтому — диалог в странице.
 *
 * Диалог появляется сам, без действий пользователя, поэтому забирает фокус на контейнер (не на «Разрешить»: случайный
 * Enter не должен выдавать доступ), держит Tab внутри, по Escape отклоняет и после ответа возвращает фокус туда, где
 * он был. Ответ уходит командой, новый статус приходит событием `mcp://state` (и перечитыванием): до него отвеченный
 * запрос скрыт здесь, иначе тот же вопрос мелькнул бы ещё раз и мог получить второй ответ.
 */
export function ApprovalDialog() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { status: s } = useMcpStatus();
  const [answered, setAnswered] = useState<string[]>([]);
  const answer = useMutation({
    mutationFn: ({ id, approve }: { id: string; approve: boolean }) => api.answerMcpApproval(id, approve),
    onSuccess: (_r, { id }) => {
      setAnswered((a) => [...a, id]);
      void qc.invalidateQueries({ queryKey: MCP_STATUS_KEY, exact: true });
    },
  });
  const rootRef = useRef<HTMLDivElement>(null);
  const queue = (s?.pending ?? []).filter((p) => !answered.includes(p.sessionId));
  const req = queue[0];
  const sessionId = req?.sessionId;
  const open = sessionId !== undefined;
  const { reset } = answer;

  // Отвеченные, которых в статусе уже нет, забываются: список не растёт, а id сессии после перезапуска сервера повторяется.
  useEffect(() => {
    if (!s) return;
    setAnswered((a) => {
      const next = a.filter((id) => s.pending.some((p) => p.sessionId === id));
      return next.length === a.length ? a : next;
    });
  }, [s]);

  // Объявлен раньше эффекта фокуса: запоминает элемент в фокусе до того, как диалог его заберёт.
  useEffect(() => {
    if (!open) return;
    const returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return () => {
      if (returnTo?.isConnected) returnTo.focus();
    };
  }, [open]);

  // Следующий запрос очереди — новый вопрос: фокус на диалог, ошибка прошлого ответа не нужна.
  useEffect(() => {
    reset();
    if (sessionId !== undefined) rootRef.current?.focus();
  }, [sessionId, reset]);

  if (!s || !req) return null;
  const client = clientLabel(req.clientName, req.clientVersion, t("mcp.unknownClient"));
  const decide = (approve: boolean) => answer.mutate({ id: req.sessionId, approve });
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Tab") trapTab(rootRef.current, e);
    if (e.key === "Escape") {
      e.stopPropagation();
      if (!answer.isPending) decide(false);
    }
  };

  return (
    <div className="modal-backdrop">
      <div className="modal" role="dialog" aria-modal="true" aria-labelledby="mcp-approval-title" tabIndex={-1} ref={rootRef} onKeyDown={onKeyDown}>
        <h3 id="mcp-approval-title">{t("mcp.approval.title", { client })}</h3>
        <p>
          {s.dataConnections > 0
            ? t("mcp.approval.bodyData", { n: s.openConnections, m: s.dataConnections })
            : t("mcp.approval.body", { n: s.openConnections })}
        </p>
        <p className="muted">{t("mcp.approval.readOnly")}</p>
        {!req.clientName && <p className="muted">{t("mcp.approval.unnamed")}</p>}
        {queue.length > 1 && <p className="muted">{t("mcp.approval.more", { n: queue.length - 1 })}</p>}
        {answer.error && <ErrorBanner error={answer.error} />}
        <div className="toolbar">
          <button type="button" className="primary" disabled={answer.isPending} onClick={() => decide(true)}>
            {t("mcp.approval.allow")}
          </button>
          <button type="button" disabled={answer.isPending} onClick={() => decide(false)}>
            {t("mcp.approval.reject")}
          </button>
        </div>
      </div>
    </div>
  );
}
