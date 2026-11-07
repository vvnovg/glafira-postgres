import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import type { ReactNode } from "react";
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
 * «Открыть в консоли EXPLAIN» из журнала (спец. P4 §11): консоль подключения с SQL, базой и узлом вызова; «План» строится сразу,
 * без выполнения (`autoPlan`, итоговая волна правок смоука P4c — как «Открыть в консоли» предложения).
 */
function openInConsole(e: McpAuditEntry): void {
  const r = consoleTarget(e);
  if ("target" in r) openExplainWithPrefill(r.target.connectionId, { sql: r.target.sql, database: r.target.database, member: r.target.member, autoPlan: true });
}

/** Раздел группы MCP по статусу шлюза: пока статус читается — «Загрузка…», не прочитался — ошибка с «Повторить». */
function WithMcpStatus({ children }: { children: (s: McpStatus) => ReactNode }) {
  const { t } = useTranslation();
  const { status, error, refetch } = useMcpStatus();
  if (error) return <ErrorBanner error={error} onRetry={refetch} />;
  if (!status) return <p className="empty">{t("common.loading")}</p>;
  return <>{children(status)}</>;
}

/**
 * «Настройки» → «LLM-ассистенты (MCP)» → «Сервер и подключение клиентов» (спец. P4 §11, смоук P4c R9): что такое MCP,
 * блок сервера — выключатель с пометкой политики, статус, счётчики открытых подключений — и блок «Подключить клиент».
 * Выключатель включён, пока настройка включена: сервер работает, обслуживается другим окном или упал при запуске
 * (`server` ≠ `stopped`).
 */
export function McpServerSection() {
  return <WithMcpStatus>{(s) => <McpServer status={s} />}</WithMcpStatus>;
}

function McpServer({ status: s }: { status: McpStatus }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toggle = useMutation({
    mutationFn: (on: boolean) => api.setMcpEnabled(on),
    onSuccess: (st) => {
      qc.setQueryData(MCP_STATUS_KEY, st);
      void qc.invalidateQueries({ queryKey: MCP_AUDIT_KEY });
    },
  });
  return (
    <>
      <p className="muted">{t("settings.mcp.intro")}</p>
      <div className="settings-block">
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
          </>
        )}
      </div>
      {s.policyEnabled && (
        <div className="settings-block">
          <ClientSnippets />
        </div>
      )}
    </>
  );
}

/**
 * «Фоновый ассистент» (спец. P4c §8.3): раздел виден и под запретом политики — выключатель там неактивен с пометкой. Без
 * `agent` в статусе (бэкенд до P4c) раздел пуст, а пункта навигации нет: ассистенту нечем работать.
 */
export function McpAgentSection() {
  return <WithMcpStatus>{(s) => (s.agent ? <AgentSection status={s} /> : null)}</WithMcpStatus>;
}

/** «Сессии и клиенты»: активные сессии и подтверждённые клиенты — отдельными блоками. */
export function McpSessionsSection() {
  return (
    <WithMcpStatus>
      {(s) => (
        <>
          <div className="settings-block">
            <McpSessions status={s} />
          </div>
          <div className="settings-block">
            <ApprovedClients status={s} />
          </div>
        </>
      )}
    </WithMcpStatus>
  );
}

/** «Журнал»: журнал MCP с «Открыть в консоли EXPLAIN»; `shown` — раздел виден (перечитывается при показе, смоук P4c R24). */
export function McpAuditSection({ shown = true }: { shown?: boolean }) {
  return (
    <WithMcpStatus>
      {(s) => (
        <div className="settings-block">
          <McpAudit status={s} onOpenInConsole={openInConsole} shown={shown} />
        </div>
      )}
    </WithMcpStatus>
  );
}
