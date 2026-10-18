import { useTranslation } from "react-i18next";
import { toAppError } from "../../api/client";
import { tr } from "../../i18n/text";
import { useWorkspace } from "../../state/workspace";

export function ErrorBanner({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  const { t } = useTranslation();
  const e = toAppError(error);
  const openTab = useWorkspace((s) => s.open);
  return (
    <div className="error-banner" role="alert">
      <div className="error-title">
        <strong>{tr(e)}</strong> <span className="muted">({e.code})</span>
        {onRetry && (
          <button type="button" onClick={onRetry}>
            {t("common.retry")}
          </button>
        )}
        {e.code === "PRO_REQUIRED" && (
          <button type="button" onClick={() => openTab({ kind: "license" })}>
            {t("common.openLicense")}
          </button>
        )}
      </div>
      {e.details && (
        <details>
          <summary>{t("common.details")}</summary>
          <pre>{e.details}</pre>
        </details>
      )}
    </div>
  );
}
