import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../../api/client";
import type { LicenseStatus } from "../../api/types";
import { tr } from "../../i18n/text";
import { ErrorBanner } from "../common/ErrorBanner";

export function LicensePanel() {
  const { t } = useTranslation();
  const uid = useId();
  const queryClient = useQueryClient();
  const license = useQuery({ queryKey: ["license"], queryFn: api.getLicenseStatus });
  const policy = useQuery({ queryKey: ["policy"], queryFn: api.getPolicyStatus });
  const [key, setKey] = useState("");
  const [confirmRemove, setConfirmRemove] = useState(false);
  const apply = (st: LicenseStatus) => queryClient.setQueryData(["license"], st);
  const activate = useMutation({
    mutationFn: (k: string) => api.setLicenseKey(k),
    onSuccess: (st) => {
      apply(st);
      setKey("");
    },
  });
  const remove = useMutation({
    mutationFn: () => api.removeLicenseKey(),
    onSuccess: (st) => {
      apply(st);
      setConfirmRemove(false);
    },
  });

  if (license.error) return <ErrorBanner error={license.error} onRetry={() => license.refetch()} />;
  const st = license.data;
  if (!st) return <p className="empty">{t("common.loading")}</p>;
  const managed = st.source === "policy";

  return (
    <div className="page">
      <div className="toolbar">
        <h2>{t("license.title")}</h2>
      </div>
      <dl className="stats">
        <div>
          <dt>{t("license.tier")}</dt>
          <dd>{st.tier === "pro" ? "Pro" : "Free"}</dd>
        </div>
        <div>
          <dt>{t("license.licensee")}</dt>
          <dd>{st.licensee ?? "—"}</dd>
        </div>
        <div>
          <dt>{t("license.kind")}</dt>
          <dd>{st.kind ? t(st.kind === "personal" ? "license.kindPersonal" : "license.kindOrganization") : "—"}</dd>
        </div>
        <div>
          <dt>{t("license.expires")}</dt>
          <dd>{st.expires ?? (st.licensee ? t("license.perpetual") : "—")}</dd>
        </div>
      </dl>
      {st.state === "expiring" && <p className="warn">
          {st.daysLeft == null
            ? t("license.expiringUnknown", { date: st.expires })
            : st.daysLeft === 0
              ? t("license.expiringToday", { date: st.expires })
              : t("license.expiring", { count: st.daysLeft, date: st.expires })}
        </p>}
      {st.state === "expired" && <p className="warn">{t("license.expired", { date: st.expires })}</p>}
      {st.state === "invalid" && st.error && <p className="warn">{tr(st.error)}</p>}

      {managed ? (
        <p className="muted">{t("license.managed")}</p>
      ) : (
        <div className="panel">
          <label htmlFor={`${uid}-key`}>{t("license.keyLabel")}</label>
          <textarea id={`${uid}-key`} rows={3} value={key} onChange={(e) => setKey(e.target.value)} />
          <div className="toolbar">
            <button type="button" className="primary" disabled={!key.trim() || activate.isPending} onClick={() => activate.mutate(key.trim())}>
              {t("license.activate")}
            </button>
            {st.source === "user" &&
              (confirmRemove ? (
                <span role="group" aria-label={t("license.removeGroupAria")}>
                  {t("license.removeQuestion")}{" "}
                  <button type="button" className="danger" disabled={remove.isPending} onClick={() => remove.mutate()}>
                    {t("license.removeYes")}
                  </button>{" "}
                  <button type="button" onClick={() => setConfirmRemove(false)}>
                    {t("common.cancel")}
                  </button>
                </span>
              ) : (
                <button type="button" className="danger" onClick={() => setConfirmRemove(true)}>
                  {t("license.removeKey")}
                </button>
              ))}
          </div>
          {activate.error && <ErrorBanner error={activate.error} />}
          {remove.error && <ErrorBanner error={remove.error} />}
        </div>
      )}

      <section>
        <h3>{t("license.whatProGives")}</h3>
        <ul>
          <li>{t("license.proProd")}</li>
        </ul>
      </section>

      {policy.data?.managed && (
        <section>
          <h3>{t("license.policyTitle")}</h3>
          <p className="muted">{t("license.policySource", { source: policy.data.source })}</p>
          {policy.data.errors.map((e, i) => (
            <p key={i} className="warn">
              {tr(e)}
            </p>
          ))}
        </section>
      )}
    </div>
  );
}
