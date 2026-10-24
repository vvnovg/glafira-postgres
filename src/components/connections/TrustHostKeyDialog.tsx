import { type KeyboardEvent, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../api/client";
import type { AppError } from "../../api/types";
import { trapTab } from "../../lib/focusTrap";
import { ErrorBanner } from "../common/ErrorBanner";
import { hostKeyUnknown } from "./profile";

/**
 * Диалог доверия ключу хоста ssh (спец. P1 §5.2): хост, отпечаток, «Доверять и подключиться» → `trust_ssh_host_key`,
 * затем `onTrusted` — повтор действия, которое упёрлось в ключ (подключение, переподключение или проверка): сам Rust
 * после доверия ничего не повторяет. Диалог открывают только для ошибки, которой можно доверять (`hostKeyUnknown`):
 * для другой ошибки он ничего не рисует. Смена ключа хоста (`hostKeyChanged`) сюда не попадает: её лечат правкой known_hosts.
 *
 * Модальный: фокус при открытии — на «Отмена» (случайный Enter ничему не доверяет), Tab ходит по кругу внутри,
 * Escape отменяет, при закрытии фокус возвращается туда, где он был.
 */
export function TrustHostKeyDialog({ error, onTrusted, onCancel }: { error: AppError; onTrusted: () => void; onCancel: () => void }) {
  const { t } = useTranslation();
  const titleId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<AppError | null>(null);
  const info = hostKeyUnknown(error);

  useEffect(() => {
    const returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    cancelRef.current?.focus();
    return () => {
      if (returnTo?.isConnected) returnTo.focus();
    };
  }, []);

  // Пока шла команда, кнопки были отключены и фокус мог уйти из диалога за его пределы: после отказа он возвращается.
  useEffect(() => {
    if (failure) cancelRef.current?.focus();
  }, [failure]);

  if (!info) return null;

  const trust = async () => {
    if (busy) return;
    setBusy(true);
    setFailure(null);
    try {
      await api.trustSshHostKey(info.target, info.fingerprint);
      onTrusted();
    } catch (e) {
      setFailure(toAppError(e));
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Tab") trapTab(rootRef.current, e);
    if (e.key === "Escape" && !busy) {
      e.stopPropagation();
      onCancel();
    }
  };

  return (
    <div className="modal-backdrop">
      <div className="modal" role="dialog" aria-modal="true" aria-labelledby={titleId} ref={rootRef} onKeyDown={onKeyDown}>
        <h3 id={titleId}>{t("hostKey.title")}</h3>
        <p>{t("hostKey.explain")}</p>
        <dl className="host-key">
          <dt>{t("hostKey.host")}</dt>
          <dd>{info.host}</dd>
          <dt>{t("hostKey.fingerprint")}</dt>
          <dd>
            <code>{info.fingerprint}</code>
          </dd>
        </dl>
        {failure && <ErrorBanner error={failure} />}
        <div className="toolbar">
          <button type="button" className="primary" disabled={busy} onClick={() => void trust()}>
            {t("hostKey.trust")}
          </button>
          <button type="button" ref={cancelRef} disabled={busy} onClick={onCancel}>
            {t("common.cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
