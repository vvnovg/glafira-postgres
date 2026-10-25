import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type KeyboardEvent, useEffect, useId, useRef } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { ConnectionProfile, DenyReason, PreviewRow, SessionResult, SessionTarget, SignalKind } from "../../api/types";
import { tr } from "../../i18n/text";
import { formatDuration } from "../../lib/format";
import { ConfirmDialog } from "../common/ConfirmDialog";
import { ErrorBanner } from "../common/ErrorBanner";

/**
 * Имя приложения (`application_name`) собственных соединений Glafira. Дублирует `PG_APPLICATION_NAME` из Rust
 * (`src-tauri/src/product.rs`): при смене там нужно менять и здесь, иначе пометка «Соединение Glafira» пропадёт.
 */
const GLAFIRA_APPLICATION = "glafira-pg";

// Ключи — литералами: проверка словарей (`pnpm i18n:check`) находит только их.
const REASON_KEYS: Record<DenyReason, string> = {
  superuserSession: "actions.sessions.reason.superuserSession",
  otherRole: "actions.sessions.reason.otherRole",
  notClientBackend: "actions.sessions.reason.notClientBackend",
  ownConnection: "actions.sessions.reason.ownConnection",
};
const RESULT_KEYS: Record<SessionResult["result"], string> = {
  terminated: "actions.sessions.result.terminated",
  signalSent: "actions.sessions.result.signalSent",
  gone: "actions.sessions.result.gone",
  noLongerMatches: "actions.sessions.result.noLongerMatches",
  notAllowed: "actions.sessions.result.notAllowed",
  failed: "actions.sessions.result.failed",
};

/** Что выбрал пользователь: вид сигнала и цель. Представления (сессии, блокировки) отдают выбор вкладке, та открывает диалог. */
export interface SessionActionChoice {
  kind: SignalKind;
  target: SessionTarget;
}

interface Props {
  connectionId: string;
  profile: ConnectionProfile;
  /** Узел, на котором выполняется действие (на нём сняты отчёт и находка). */
  member: string;
  kind: SignalKind;
  target: SessionTarget;
  onClose: () => void;
}

/**
 * Отмена запроса или завершение сессий (спец. P2 §8.3): предпросмотр заново читает сессии цели с правами,
 * выполнение получает только разрешённые; пакет на prod подтверждается числом сессий.
 */
export function SessionActionDialog({ connectionId, profile, member, kind, target, onClose }: Props) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const rootRef = useRef<HTMLElement>(null);
  // Диалог открывается над телом вкладки, а кнопка могла быть внизу длинного списка: фокус и прокрутка — к нему.
  // При закрытии фокус возвращается на кнопку, открывшую диалог (если она осталась на странице).
  useEffect(() => {
    const returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    rootRef.current?.focus({ preventScroll: true });
    rootRef.current?.scrollIntoView?.({ block: "nearest" });
    return () => {
      if (returnTo?.isConnected) returnTo.focus({ preventScroll: true });
    };
  }, []);

  // Список сессий читается один раз и не обновляется под рукой у пользователя, пока он подтверждает.
  // Экземпляр диалога входит в ключ: повторное открытие того же выбора читает сессии заново, а не берёт кэш прошлого диалога.
  const instance = useId();
  const preview = useQuery({
    queryKey: ["actionPreview", connectionId, member, kind, target, instance],
    queryFn: () => api.previewSessionAction(connectionId, { member, kind, target }),
    staleTime: Infinity,
    gcTime: 0,
  });
  const allowed = preview.data?.rows.filter((r) => r.allowed) ?? [];
  // Как в Rust: несколько сессий или любой выбор по критерию — пакет, и на prod нужно ввести число сессий.
  const batch = allowed.length > 1 || target.kind === "idleInTransaction";
  const run = useMutation({
    mutationFn: (c: { confirm: string | null; typedName: string | null }) =>
      api.executeSessionAction(
        connectionId,
        { member: preview.data?.member ?? member, kind, target, sessions: allowed.map((r) => ({ pid: r.session.pid, backendStartUs: r.session.backendStartUs })) },
        c,
      ),
    onSuccess: () => {
      // Сессии в отчётах изменились, а запись о действии появилась в журнале подключения.
      void qc.invalidateQueries({ queryKey: ["diag", connectionId] });
      void qc.invalidateQueries({ queryKey: ["actions", connectionId] });
    },
  });

  const title = kind === "cancel" ? t("actions.sessions.cancelTitle") : t("actions.sessions.terminateTitle");
  // Одна сессия — без числа («Завершить сессию»); число нужно только пакету. Формы _one в ru ещё и «21», «31»: с числом.
  const count = allowed.length;
  let label: string;
  if (kind === "cancel") label = count === 1 ? t("actions.sessions.cancelOne") : t("actions.sessions.cancel", { count });
  else label = count === 1 ? t("actions.sessions.terminateOne") : t("actions.sessions.terminate", { count });
  // Escape закрывает диалог, где бы ни стоял фокус внутри него (на самой секции, на «Закрыть», в таблице), — не только
  // внутри группы подтверждения. Перехват в фазе погружения: Escape в группе иначе обработал бы ещё и ConfirmDialog, и onClose
  // вызвался бы дважды. Пока команда выполняется, диалог не закрывается (как и в ConfirmDialog).
  const onKeyDownCapture = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    e.stopPropagation();
    if (!run.isPending) onClose();
  };
  const noteOf = (r: PreviewRow): string => {
    if (!r.allowed) return r.reason ? t(REASON_KEYS[r.reason]) : "";
    // Разрешённое сервером остаётся в списке, но пользователь не должен завершить приложение случайно.
    return r.session.application === GLAFIRA_APPLICATION ? t("actions.sessions.glafiraNote") : "";
  };

  return (
    // Имя секции не повторяет заголовок группы подтверждения внутри неё: два одинаковых имени читались бы дважды.
    <section
      ref={rootRef}
      tabIndex={-1}
      className="panel"
      aria-label={kind === "cancel" ? t("actions.kind.cancel") : t("actions.kind.terminate")}
      onKeyDownCapture={onKeyDownCapture}
    >
      <p className="muted">{t("actions.sessions.node", { member: preview.data?.member ?? member })}</p>
      {target.kind === "idleInTransaction" && <p className="muted">{t("actions.sessions.criterionIdle", { duration: formatDuration(target.olderThanS) })}</p>}
      {preview.isLoading && <p className="muted">{t("common.loading")}</p>}
      {preview.error && (
        <>
          <ErrorBanner error={preview.error} />
          <button type="button" onClick={onClose}>
            {t("common.close")}
          </button>
        </>
      )}
      {preview.data && !run.data && (
        <>
          {preview.data.rows.length > 0 && (
            <div className="table-scroll">
              <table className="diag-table" aria-label={kind === "cancel" ? t("actions.sessions.tableCancel") : t("actions.sessions.tableTerminate")}>
                <thead>
                  <tr>
                    <th scope="col">{t("diag.sessions.columns.pid")}</th>
                    <th scope="col">{t("diag.sessions.columns.user")}</th>
                    <th scope="col">{t("diag.sessions.columns.application")}</th>
                    <th scope="col">{t("diag.sessions.columns.state")}</th>
                    <th scope="col">{t("diag.sessions.columns.xactAge")}</th>
                    <th scope="col">{t("diag.sessions.columns.text")}</th>
                    <th scope="col">{t("actions.sessions.columns.note")}</th>
                  </tr>
                </thead>
                <tbody>
                  {preview.data.rows.map((r) => (
                    <tr key={`${r.session.pid}:${r.session.backendStartUs}`} className={r.allowed ? undefined : "muted"}>
                      <td>{r.session.pid}</td>
                      <td>{r.session.user ?? ""}</td>
                      <td>{r.session.application ?? ""}</td>
                      <td>{r.session.state ?? ""}</td>
                      <td>{formatDuration(r.session.xactAgeS)}</td>
                      <td title={r.session.query ?? undefined}>{(r.session.query ?? "").slice(0, 120)}</td>
                      <td className={r.allowed && noteOf(r) ? "diag-warn" : undefined}>{noteOf(r)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
          {allowed.length === 0 ? (
            <>
              {/* Пустой набор (сессии уже нет или ни одна не подошла) — не то же самое, что «сессии есть, но прав нет». */}
              <p className="muted">{preview.data.rows.length === 0 ? t("actions.sessions.noMatching") : t("actions.sessions.nothing")}</p>
              <button type="button" onClick={onClose}>
                {t("common.close")}
              </button>
            </>
          ) : (
            <ConfirmDialog
              connection={profile}
              title={title}
              confirmLabel={label}
              danger={kind === "terminate"}
              busy={run.isPending}
              objectName={batch && profile.environment === "prod" ? String(allowed.length) : undefined}
              objectLabel={t("actions.sessions.countLabel")}
              onConfirm={(c) => run.mutate(c)}
              onCancel={onClose}
            />
          )}
        </>
      )}
      {run.error && <ErrorBanner error={run.error} />}
      {run.data && (
        <>
          <div className="table-scroll">
            <table className="diag-table" aria-label={t("actions.sessions.resultTable")}>
              <thead>
                <tr>
                  <th scope="col">{t("diag.sessions.columns.pid")}</th>
                  <th scope="col">{t("actions.sessions.columns.result")}</th>
                </tr>
              </thead>
              <tbody>
                {run.data.results.map((r) => (
                  <tr key={`${r.pid}:${r.backendStartUs}`}>
                    <td>{r.pid}</td>
                    <td>
                      {t(RESULT_KEYS[r.result])}
                      {r.result === "failed" && <span className="muted"> — {tr(r.error)}</span>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {!run.data.journalWritten && <p className="diag-warn">{t("actions.journalNotWritten")}</p>}
          <button type="button" onClick={onClose}>
            {t("common.close")}
          </button>
        </>
      )}
    </section>
  );
}
