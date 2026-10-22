import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useMutation, useQuery } from "@tanstack/react-query";
import { save } from "@tauri-apps/plugin-dialog";
import { api } from "../../api/client";
import { tr } from "../../i18n/text";
import { useDiagnosticsRequest } from "../../state/diagnosticsRequest";
import { ErrorBanner } from "../common/ErrorBanner";

export function AboutPanel() {
  const { t } = useTranslation();
  const info = useQuery({ queryKey: ["app-info"], queryFn: api.getAppInfo });
  const policy = useQuery({ queryKey: ["policy"], queryFn: api.getPolicyStatus });
  const [saved, setSaved] = useState<string | null>(null);
  const diag = useMutation({
    mutationFn: async () => {
      setSaved(null);
      const date = new Date().toISOString().slice(0, 10);
      const path = await save({ defaultPath: `diagnostics-${date}.zip`, filters: [{ name: "ZIP", extensions: ["zip"] }] });
      if (!path) return null;
      await api.exportDiagnostics(path);
      return path;
    },
    onSuccess: (path) => {
      if (path) setSaved(path);
    },
  });
  // «Собрать диагностику…» из меню Help: тот же сбор, что и по кнопке; запрос забирается один раз.
  // Пока сбор идёт, запрос снимается без второго сбора и второго диалога сохранения.
  const diagRequested = useDiagnosticsRequest((s) => s.pending);
  const { mutate: collect, isPending: collecting } = diag;
  useEffect(() => {
    if (diagRequested && useDiagnosticsRequest.getState().consume() && !collecting) collect();
  }, [diagRequested, collect, collecting]);

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
        <div><dt>{t("about.policy")}</dt><dd>{policy.data ? (policy.data.managed ? t("about.policySet") : t("about.policyNone")) : "…"}</dd></div>
      </dl>
      {policy.data?.errors.map((e, i) => (
        <p key={i} className="warn">
          {t("about.policyError", { error: tr(e) })}
        </p>
      ))}
      <p className="muted">{t("about.dataDir")} <code>{i.dataDir}</code></p>
      <p className="muted">{t("about.logDir")} <code>{i.logDir}</code></p>
      <section className="panel">
        <h3>{t("about.diagnostics.title")}</h3>
        <p className="muted">{t("about.diagnostics.hint")}</p>
        <div className="toolbar">
          <button type="button" disabled={diag.isPending} onClick={() => diag.mutate()}>
            {t("about.diagnostics.collect")}
          </button>
        </div>
        {saved && <p role="status">{t("about.diagnostics.saved", { path: saved })}</p>}
        {diag.error && <ErrorBanner error={diag.error} />}
      </section>
    </div>
  );
}
