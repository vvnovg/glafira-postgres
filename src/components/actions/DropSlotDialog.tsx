import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type KeyboardEvent, useEffect, useId, useRef } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { ConnectionProfile, PatroniSlot } from "../../api/types";
import { formatTimestamp } from "../../lib/format";
import { ConfirmDialog } from "../common/ConfirmDialog";
import { ErrorBanner } from "../common/ErrorBanner";
import { bytesText, metaText } from "../diag/viewText";

// Ключи — литералами: проверка словарей (`pnpm i18n:check`) находит только их.
const PATRONI_WARNING_KEYS: Record<PatroniSlot, string> = {
  member: "actions.slots.patroniMember",
  permanent: "actions.slots.patroniPermanent",
};

interface Props {
  connectionId: string;
  profile: ConnectionProfile;
  /** Узел, на котором слот виден в отчёте (на нём он и удаляется). */
  member: string;
  /** Имя слота; его же нужно ввести для подтверждения. */
  slot: string;
  /** Вкладка-владелец: входит в ключ мутации, и вкладка отличает свое удаление от удалений на других вкладках подключения. */
  scope: string;
  onClose: () => void;
}

/**
 * Удаление слота репликации (спец. P2 §8.4): предпросмотр заново читает слот узла (что он держит и чей он в Patroni),
 * подтверждение — ввод имени слота, на prod ещё имя подключения. Сервер удаляет только неактивный слот.
 */
export function DropSlotDialog({ connectionId, profile, member, slot, scope, onClose }: Props) {
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

  // Слот читается один раз и не обновляется под рукой у пользователя, пока он подтверждает.
  // Экземпляр диалога входит в ключ: повторное открытие того же слота читает его заново, а не берёт кэш прошлого диалога.
  const instance = useId();
  const preview = useQuery({
    queryKey: ["dropSlotPreview", connectionId, member, slot, instance],
    queryFn: () => api.previewDropSlot(connectionId, { member, slot }),
    staleTime: Infinity,
    gcTime: 0,
  });
  const node = preview.data?.member ?? member;
  const run = useMutation({
    // Ключ мутации нужен вкладке: пока слот удаляется, другой диалог поверх этого не открывается (чужие вкладки не ждут).
    mutationKey: ["dropSlot", connectionId, scope],
    mutationFn: (c: { confirm: string | null; typedName: string | null }) => api.dropSlot(connectionId, { member: node, slot }, c),
    onSuccess: () => {
      // Слот пропал из отчётов, а запись об удалении появилась в журнале подключения.
      void qc.invalidateQueries({ queryKey: ["diag", connectionId] });
      void qc.invalidateQueries({ queryKey: ["actions", connectionId] });
    },
  });

  // Escape закрывает диалог, где бы ни стоял фокус внутри него, — не только внутри группы подтверждения. Перехват в фазе
  // погружения: Escape в группе иначе обработал бы ещё и ConfirmDialog, и onClose вызвался бы дважды. Пока команда
  // выполняется, диалог не закрывается (как и в ConfirmDialog).
  const onKeyDownCapture = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    e.stopPropagation();
    if (!run.isPending) onClose();
  };
  const s = preview.data?.slot;

  return (
    // Имя секции не повторяет заголовок группы подтверждения внутри неё: два одинаковых имени читались бы дважды.
    <section ref={rootRef} tabIndex={-1} className="panel" aria-label={t("actions.kind.dropSlot")} onKeyDownCapture={onKeyDownCapture}>
      <p className="muted">{t("actions.sessions.node", { member: node })}</p>
      {preview.isLoading && <p className="muted">{t("common.loading")}</p>}
      {preview.error && (
        <>
          <ErrorBanner error={preview.error} />
          <button type="button" onClick={onClose}>
            {t("common.close")}
          </button>
        </>
      )}
      {s && !run.data && (
        <>
          <dl className="kv">
            <dt>{t("diag.slots.columns.type")}</dt>
            <dd>{metaText([s.slotType, s.plugin])}</dd>
            <dt>{t("diag.slots.columns.database")}</dt>
            <dd>{s.database ?? "—"}</dd>
            <dt>{t("diag.slots.columns.state")}</dt>
            <dd>{s.active ? (s.activePid === null ? t("diag.slots.active") : t("diag.slots.activePid", { pid: s.activePid })) : t("diag.slots.inactive")}</dd>
            <dt>{t("diag.slots.columns.walStatus")}</dt>
            <dd>{s.walStatus ?? "—"}</dd>
            <dt>{t("diag.slots.columns.retained")}</dt>
            <dd>{bytesText(s.retainedBytes)}</dd>
            {s.inactiveSinceMs !== null && (
              <>
                <dt>{t("diag.slots.columns.inactiveSince")}</dt>
                <dd>{formatTimestamp(s.inactiveSinceMs)}</dd>
              </>
            )}
          </dl>
          {s.active ? (
            // Слот ожил, пока пользователь шёл к диалогу: удаляется только неактивный слот, подтверждения нет.
            <>
              <p className="diag-warn" role="status">
                {t("actions.slots.activeNoDrop")}
              </p>
              <button type="button" onClick={onClose}>
                {t("common.close")}
              </button>
            </>
          ) : (
            <>
              {s.patroni !== null && <p className="diag-warn">{t(PATRONI_WARNING_KEYS[s.patroni])}</p>}
              <ConfirmDialog
                connection={profile}
                title={t("actions.slots.dropTitle", { slot })}
                confirmLabel={t("actions.slots.confirm")}
                objectName={slot}
                objectLabel={t("actions.slots.nameLabel")}
                danger
                busy={run.isPending}
                onConfirm={(c) => run.mutate(c)}
                onCancel={onClose}
              />
            </>
          )}
        </>
      )}
      {run.error && <ErrorBanner error={run.error} />}
      {run.data && (
        <>
          <p role="status">{t("actions.slots.done")}</p>
          {!run.data.journalWritten && <p className="diag-warn">{t("actions.journalNotWritten")}</p>}
          <button type="button" onClick={onClose}>
            {t("common.close")}
          </button>
        </>
      )}
    </section>
  );
}
