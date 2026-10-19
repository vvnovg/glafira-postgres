import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type KeyboardEvent, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import type { ImportPreview, PreviewItem } from "../../../api/types";
import { tr } from "../../../i18n/text";
import { trapTab } from "../../../lib/focusTrap";
import { ErrorBanner } from "../../common/ErrorBanner";
import { Hint } from "../../common/Hint";
import { folderPaths } from "./folderTree";

interface Props {
  /** Выбранный файл экспорта. */
  path: string;
  /** Папка, выбранная целью при открытии («Импортировать сюда»); null — корень. */
  initialTargetId: string | null;
  onClose: () => void;
  /** Импорт применён: итог для сообщения в панели. */
  onImported: (result: ImportPreview) => void;
}

const itemLabel = (item: PreviewItem) => [...item.path, item.name].join(" / ");

/** Раскрываемый список одной группы предпросмотра; пустая группа не показывается. */
function Group({ title, items }: { title: string; items: string[] }) {
  if (items.length === 0) return null;
  return (
    <details className="import-group">
      <summary>{`${title} (${items.length})`}</summary>
      <ul>
        {items.map((label, i) => (
          <li key={i}>{label}</li>
        ))}
      </ul>
    </details>
  );
}

/**
 * Модальный диалог импорта подключений внутри страницы: цель, флажок «Обновлять существующие»,
 * предпросмотр (повторяется при каждой смене цели или флажка) и применение.
 */
export function ImportDialog({ path, initialTargetId, onClose, onImported }: Props) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const titleId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const [targetId, setTargetId] = useState<string | null>(initialTargetId);
  const [updateExisting, setUpdateExisting] = useState(true);
  const [busy, setBusy] = useState(false);
  const [applyError, setApplyError] = useState<unknown>(null);

  const foldersQ = useQuery({ queryKey: ["folders"], queryFn: api.listFolders });
  const previewQ = useQuery({
    queryKey: ["import-preview", path, targetId, updateExisting],
    queryFn: () => api.previewImport(path, targetId, updateExisting),
    gcTime: 0,
    staleTime: 0,
  });
  const preview = previewQ.data;
  const paths = folderPaths(foldersQ.data ?? []);

  useEffect(() => {
    // Фокус возвращается туда, где он был до открытия (обычно кнопка «⋯» меню, из которого открыли диалог).
    const returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    rootRef.current?.querySelector<HTMLElement>("select")?.focus();
    return () => {
      if (returnTo?.isConnected) returnTo.focus();
    };
  }, []);

  const apply = async () => {
    setBusy(true);
    setApplyError(null);
    try {
      const result = await api.applyImport(path, targetId, updateExisting);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["folders"] }),
        queryClient.invalidateQueries({ queryKey: ["connections"] }),
      ]);
      onImported(result);
    } catch (err) {
      setApplyError(err);
      setBusy(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Tab") trapTab(rootRef.current, e);
    if (e.key === "Escape" && !busy) {
      e.stopPropagation();
      onClose();
    }
  };

  const canApply = !!preview && !previewQ.isFetching && !busy;

  return (
    <div className="modal-backdrop">
      <div className="modal" role="dialog" aria-modal="true" aria-labelledby={titleId} ref={rootRef} onKeyDown={onKeyDown}>
        <h3 id={titleId}>{t("exchange.import.title")}</h3>
        <div className="form">
          <span className="field-label">
            <label htmlFor={`${titleId}-target`}>{t("exchange.import.target")}</label>
            <Hint k="hints.import.target" label={t("exchange.import.target")} />
          </span>
          <select id={`${titleId}-target`} value={targetId ?? ""} disabled={busy} onChange={(e) => setTargetId(e.target.value || null)}>
            <option value="">{t("connections.form.folderRoot")}</option>
            {paths.map((p) => (
              <option key={p.id} value={p.id}>
                {p.path.join(" / ")}
              </option>
            ))}
          </select>
          <span className="field-label">
            <label htmlFor={`${titleId}-update`}>{t("exchange.import.updateExisting")}</label>
            <Hint k="hints.import.updateExisting" label={t("exchange.import.updateExisting")} />
          </span>
          <input
            id={`${titleId}-update`}
            type="checkbox"
            checked={updateExisting}
            disabled={busy}
            onChange={(e) => setUpdateExisting(e.target.checked)}
          />
        </div>

        {previewQ.error != null && <ErrorBanner error={previewQ.error} />}
        {previewQ.isPending && <p className="muted">{t("common.loading")}</p>}
        {preview && (
          <div className="import-preview">
            <p>
              {t("exchange.import.summary", {
                added: preview.added.length,
                updated: preview.updated.length,
                unchanged: preview.unchanged.length,
                skipped: preview.skipped.length,
              })}
            </p>
            <Group title={t("exchange.import.groupAdded")} items={preview.added.map(itemLabel)} />
            <Group title={t("exchange.import.groupUpdated")} items={preview.updated.map(itemLabel)} />
            <Group title={t("exchange.import.groupUnchanged")} items={preview.unchanged.map(itemLabel)} />
            <Group title={t("exchange.import.groupSkipped")} items={preview.skipped.map((s) => `${s.name} — ${tr(s.reason)}`)} />
            {(preview.warnings.length > 0 || preview.needsPasswords > 0) && (
              <ul className="import-warnings">
                {preview.warnings.map((w, i) => (
                  <li key={i}>{tr(w)}</li>
                ))}
                {preview.needsPasswords > 0 && <li>{t("exchange.import.passwords", { count: preview.needsPasswords })}</li>}
              </ul>
            )}
          </div>
        )}
        {applyError != null && <ErrorBanner error={applyError} />}

        <div className="toolbar">
          <button type="button" className="primary" disabled={!canApply} onClick={apply}>
            {t("exchange.import.apply")}
          </button>
          <button type="button" disabled={busy} onClick={onClose}>
            {t("common.cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
