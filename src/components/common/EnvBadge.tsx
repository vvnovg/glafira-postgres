import { useTranslation } from "react-i18next";
import type { Environment } from "../../api/types";
import { ENV_COLORS, ENV_LABELS } from "../../lib/env";

export function EnvBadge({ environment, readOnly }: { environment: Environment; readOnly: boolean }) {
  const { t } = useTranslation();
  return (
    <>
      <span className="env-badge" style={{ background: ENV_COLORS[environment] }}>
        {ENV_LABELS[environment]}
      </span>
      {readOnly && (
        <span role="img" aria-label={t("common.readOnly")} title={t("common.readOnly")}>
          🔒
        </span>
      )}
    </>
  );
}
