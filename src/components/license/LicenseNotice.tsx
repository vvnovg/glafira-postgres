import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useLicense } from "../../lib/useLicense";
import { useWorkspace } from "../../state/workspace";

export function LicenseNotice() {
  const { t } = useTranslation();
  const { status } = useLicense();
  const open = useWorkspace((s) => s.open);
  const [hidden, setHidden] = useState(false);
  if (hidden || !status || (status.state !== "expiring" && status.state !== "expired")) return null;
  return (
    <div className="notice" role="status">
      <span>
        {status.state === "expiring"
          ? status.daysLeft == null
            ? t("license.notice.expiringUnknown")
            : status.daysLeft === 0
              ? t("license.notice.expiringToday")
              : t("license.notice.expiring", { count: status.daysLeft })
          : t("license.notice.expired", { date: status.expires })}
      </span>
      <button type="button" className="link" onClick={() => open({ kind: "license" })}>
        {t("license.notice.more")}
      </button>
      <button type="button" className="icon" aria-label={t("license.notice.hideAria")} onClick={() => setHidden(true)}>
        ×
      </button>
    </div>
  );
}
