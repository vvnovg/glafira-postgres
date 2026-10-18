import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../api/client";
import { tr } from "../../i18n/text";
import { ErrorBanner } from "../common/ErrorBanner";

export function AboutPanel() {
  const { t } = useTranslation();
  const info = useQuery({ queryKey: ["app-info"], queryFn: api.getAppInfo });
  const license = useQuery({ queryKey: ["license"], queryFn: api.getLicenseStatus });
  const policy = useQuery({ queryKey: ["policy"], queryFn: api.getPolicyStatus });

  if (info.error) return <ErrorBanner error={info.error} onRetry={() => info.refetch()} />;
  const i = info.data;
  if (!i) return <p className="empty">{t("common.loading")}</p>;
  return (
    <div className="page">
      <div className="toolbar">
        <h2>{i.name}</h2>
      </div>
      <p className="about-summary">{t("about.summary")}</p>
      <dl className="stats">
        <div><dt>{t("about.version")}</dt><dd>{i.version}</dd></div>
        <div><dt>{t("about.build")}</dt><dd>{i.commit}</dd></div>
        <div><dt>{t("about.system")}</dt><dd>{i.os} / {i.arch}</dd></div>
        <div><dt>{t("about.tier")}</dt><dd>{license.data ? (license.data.tier === "pro" ? "Pro" : "Free") : "…"}</dd></div>
        <div><dt>{t("about.policy")}</dt><dd>{policy.data ? (policy.data.managed ? t("about.policySet") : t("about.policyNone")) : "…"}</dd></div>
      </dl>
      {policy.data?.errors.map((e, i) => (
        <p key={i} className="warn">
          {t("about.policyError", { error: tr(e) })}
        </p>
      ))}
      <p className="muted">{t("about.dataDir")} <code>{i.dataDir}</code></p>
      <p className="muted">{t("about.logDir")} <code>{i.logDir}</code></p>
    </div>
  );
}
