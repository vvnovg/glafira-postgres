import { type KeyboardEvent, type ReactNode, useEffect, useRef, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import type { Confirmation, ConnectionProfile } from "../../api/types";

/** Нужен ли диалог: prod-подключение или операция с вводом имени объекта (удаление слота, failover). */
export function needsConfirmation(connection: ConnectionProfile, objectName?: string): boolean {
  return connection.environment === "prod" || objectName !== undefined;
}

interface Props {
  connection: ConnectionProfile;
  title: string;
  confirmLabel: string;
  /** Имя объекта, которое нужно ввести для подтверждения. */
  objectName?: string;
  /** Что именно вводится: «имя слота», «имя кандидата», «число сессий». */
  objectLabel?: string;
  danger?: boolean;
  busy?: boolean;
  children?: ReactNode;
  onConfirm: (c: Confirmation) => void;
  onCancel: () => void;
}

/**
 * Подтверждение внутри страницы (нативный window.confirm в Tauri 2 не показывается).
 * Проверка дублируется в Rust: без верных значений команда вернёт CONFIRMATION_REQUIRED.
 */
export function ConfirmDialog({
  connection,
  title,
  confirmLabel,
  objectName,
  objectLabel,
  danger,
  busy,
  children,
  onConfirm,
  onCancel,
}: Props) {
  const { t } = useTranslation();
  const label = objectLabel ?? t("common.confirm.defaultLabel");
  const [connName, setConnName] = useState("");
  const [typed, setTyped] = useState("");
  const rootRef = useRef<HTMLDivElement>(null);
  const isProd = connection.environment === "prod";
  const ok =
    (!isProd || connName.trim() === connection.name.trim()) &&
    (objectName === undefined || typed.trim() === objectName.trim());

  useEffect(() => {
    rootRef.current?.querySelector("input")?.focus();
  }, []);

  const submit = () =>
    onConfirm({
      confirm: isProd ? connName.trim() : null,
      typedName: objectName !== undefined ? typed.trim() : null,
    });

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      if (!busy) onCancel();
    } else if (e.key === "Enter" && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      if (ok && !busy) submit();
    }
  };

  return (
    <div className="confirm" role="group" aria-label={title} ref={rootRef} onKeyDown={onKeyDown}>
      <strong>{title}</strong>
      {children}
      {objectName !== undefined && (
        <label>
          <Trans i18nKey="common.confirm.typeObject" values={{ label, name: objectName }} components={{ code: <code /> }} />
          <input aria-label={t("common.confirm.objectAria", { label })} value={typed} disabled={busy} onChange={(e) => setTyped(e.target.value)} />
        </label>
      )}
      {isProd && (
        <label>
          <Trans i18nKey="common.confirm.prodNotice" values={{ name: connection.name }} components={{ code: <code /> }} />
          <input aria-label={t("common.confirm.connectionAria")} value={connName} disabled={busy} onChange={(e) => setConnName(e.target.value)} />
        </label>
      )}
      <div className="toolbar">
        <button
          type="button"
          className={danger ? "danger" : "primary"}
          disabled={!ok || busy}
          onClick={submit}
        >
          {confirmLabel}
        </button>
        <button type="button" disabled={busy} onClick={onCancel}>
          {t("common.cancel")}
        </button>
      </div>
    </div>
  );
}
