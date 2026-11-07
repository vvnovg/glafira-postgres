import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { McpAuditEntry, McpStatus } from "../../api/types";
import { MCP_AUDIT_KEY, MCP_STATUS_KEY, useMcpStatus } from "../../lib/useMcpStatus";
import { openExplainWithPrefill } from "../../state/workspace";
import { ErrorBanner } from "../common/ErrorBanner";
import { Hint } from "../common/Hint";
import { ApprovedClients } from "./ApprovedClients";
import { ClientSnippets } from "./ClientSnippets";
import { AgentSection } from "./agent/AgentSection";
import { McpAudit } from "./McpAudit";
import { consoleTarget } from "./auditOpen";
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
 * подключений, фрагменты для клиентов, подраздел «Фоновый ассистент» (спец. P4c §8.3), активные сессии, подтверждённые
 * клиенты, журнал. Выключатель включён, пока настройка включена: сервер работает, обслуживается другим окном или упал при
 * запуске (`server` ≠ `stopped`).
 */
/** «Открыть в консоли EXPLAIN» из журнала (спец. P4 §11): консоль подключения с SQL, базой и узлом вызова, без запуска. */
function openInConsole(e: McpAuditEntry): void {
  const r = consoleTarget(e);
  if ("target" in r) openExplainWithPrefill(r.target.connectionId, { sql: r.target.sql, database: r.target.database, member: r.target.member });
}

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
          {/* Подраздел виден и под запретом политики: выключатель там неактивен с пометкой (спец. P4c §8.3). */}
          {/* Без agent в статусе (бэкенд до P4c) подраздела нет: ему нечем работать. */}
          {s.agent && <AgentSection status={s} />}
          <McpSessions status={s} />
          <ApprovedClients status={s} />
          <McpAudit status={s} onOpenInConsole={openInConsole} />
        </>
      )}
    </section>
  );
}
