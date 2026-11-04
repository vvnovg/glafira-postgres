import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { McpStatus } from "../../api/types";
import { MCP_AUDIT_KEY, MCP_STATUS_KEY, useMcpStatus } from "../../lib/useMcpStatus";
import { ErrorBanner } from "../common/ErrorBanner";
import { Hint } from "../common/Hint";
import { ApprovedClients } from "./ApprovedClients";
import { ClientSnippets } from "./ClientSnippets";
import { McpAudit } from "./McpAudit";
import { McpSessions } from "./McpSessions";

/** Строка статуса сервера: работает, выключен, работает в другом окне Glafira, ошибка с текстом из Rust. */
function stateText(s: McpStatus, t: TFunction): string {
  switch (s.server) {
    case "running":
      return t("settings.mcp.state.running");
    case "stopped":
      return t("settings.mcp.state.stopped");
    case "otherInstance":
      return t("settings.mcp.state.otherInstance");
    case "error":
      return t("settings.mcp.state.error", { message: s.error ?? "" });
  }
}

/**
 * «Настройки» → «LLM-ассистенты (MCP)» (спец. P4 §11): выключатель с пометкой политики, статус, счётчики открытых
 * подключений, фрагменты для клиентов, активные сессии, подтверждённые клиенты, журнал. Выключатель включён, пока
 * настройка включена: сервер работает, обслуживается другим окном или упал при запуске (`server` ≠ `stopped`).
 */
export function McpSection() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { status: s, error, refetch } = useMcpStatus();
  const toggle = useMutation({
    mutationFn: (on: boolean) => api.setMcpEnabled(on),
    onSuccess: (st) => {
      qc.setQueryData(MCP_STATUS_KEY, st);
      void qc.invalidateQueries({ queryKey: MCP_AUDIT_KEY });
    },
  });
  return (
    <section aria-labelledby="settings-mcp-title" className="mcp-section">
      <h3 id="settings-mcp-title">{t("settings.mcp.section")}</h3>
      <p className="muted">{t("settings.mcp.intro")}</p>
      {error ? (
        <ErrorBanner error={error} onRetry={refetch} />
      ) : !s ? (
        <p className="empty">{t("common.loading")}</p>
      ) : (
        <>
          <div className="form">
            <span className="field-label">
              <label htmlFor="settings-mcp-enabled">{t("settings.mcp.enable")}</label>
              <Hint k="hints.settings.mcpEnable" label={t("settings.mcp.enable")} />
            </span>
            <span>
              <input
                id="settings-mcp-enabled"
                type="checkbox"
                checked={s.policyEnabled && s.enabled}
                disabled={!s.policyEnabled || toggle.isPending}
                onChange={(e) => toggle.mutate(e.target.checked)}
              />
            </span>
          </div>
          {!s.policyEnabled && <p className="muted">{t("settings.mcp.policy")}</p>}
          <p role="status">{stateText(s, t)}</p>
          {toggle.error && <ErrorBanner error={toggle.error} />}
          {s.policyEnabled && (
            <>
              <p>
                {s.dataConnections > 0
                  ? t("settings.mcp.openData", { n: s.openConnections, m: s.dataConnections })
                  : t("settings.mcp.open", { n: s.openConnections })}
              </p>
              <p className="muted">{t("settings.mcp.openHint")}</p>
              {s.maxAccess === "diagnostics" && <p className="muted">{t("settings.mcp.dataPolicy")}</p>}
              <ClientSnippets />
            </>
          )}
          <McpSessions status={s} />
          <ApprovedClients status={s} />
          <McpAudit status={s} />
        </>
      )}
    </section>
  );
}
