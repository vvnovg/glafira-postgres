import { useMutation, useQuery } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { McpAuditEntry, McpStatus } from "../../api/types";
import { formatCount, formatTimestamp } from "../../lib/format";
import { MCP_AUDIT_KEY } from "../../lib/useMcpStatus";
import { ErrorBanner } from "../common/ErrorBanner";
import { isConsoleCall, openBlocked } from "./auditOpen";
import { finishedRunText } from "./agent/agentText";
import { Icon } from "../common/Icon";
import { AuditCost } from "./agent/RunCost";

/** Сколько последних записей журнала показывает раздел (спец. P4 §11). */
export const AUDIT_LIMIT = 200;

/** Инструмент вызова или событие сессии и сервера; события, которых эта версия не знает (P4b и новее), — как в файле. */
function whatText(e: McpAuditEntry, t: TFunction): string {
  switch (e.event) {
    case "call":
      return e.tool ?? "";
    case "sessionStarted":
      return t("settings.mcp.audit.events.sessionStarted");
    case "sessionEnded":
      return t("settings.mcp.audit.events.sessionEnded");
    case "approved":
      return t("settings.mcp.audit.events.approved");
    case "rejected":
      return t("settings.mcp.audit.events.rejected");
    case "revoked":
      return t("settings.mcp.audit.events.revoked");
    case "serverStarted":
      return t("settings.mcp.audit.events.serverStarted");
    case "serverStopped":
      return t("settings.mcp.audit.events.serverStopped");
    case "requestCreated":
      return t("settings.mcp.audit.events.requestCreated");
    case "proposalAnswered":
      return t("settings.mcp.audit.events.proposalAnswered");
    case "agentRunStarted":
      return t("settings.mcp.audit.events.agentRunStarted");
    case "agentRunFinished":
      return t("settings.mcp.audit.events.agentRunFinished");
    default:
      return e.event;
  }
}

/**
 * Итог вызова: «успешно» или код ошибки (`not_allowed`, `forbidden_function`…); у конца фонового запуска — его итог, как в
 * «Запусках» (спец. P4c §9.2); у прочих событий — пусто.
 */
function resultText(e: McpAuditEntry, t: TFunction): string {
  if (e.event === "agentRunFinished") return finishedRunText(e, t);
  if (e.outcome === "ok") return t("settings.mcp.audit.ok");
  return e.outcome === "error" ? (e.code ?? "") : "";
}

/**
 * Счётчики приложения: строк `run_select` (P4b) или замаскированных литералов (уровень «Диагностика»). Стоимость конца фонового
 * запуска рисует `AuditCost` в той же ячейке.
 */
function detailsText(e: McpAuditEntry, t: TFunction): string {
  if (e.rows !== undefined) return t("settings.mcp.audit.rows", { n: formatCount(e.rows) });
  if (e.masked !== undefined) return t("settings.mcp.audit.masked", { n: formatCount(e.masked) });
  return "";
}

/**
 * Журнал MCP (спец. P4 §10.3, §11): последние записи `mcp-audit.jsonl`, «Обновить» и «Показать файл»; заголовок — у раздела
 * «Журнал» «Настроек». У успешных `explain_query` (и `run_select` P4b) — «Открыть в консоли EXPLAIN», если раздел передал
 * `onOpenInConsole`; неактивная кнопка называет причину в подсказке.
 */
export function McpAudit({ status, onOpenInConsole }: { status: McpStatus; onOpenInConsole?: (entry: McpAuditEntry) => void }) {
  const { t } = useTranslation();
  const audit = useQuery({ queryKey: MCP_AUDIT_KEY, queryFn: () => api.getMcpAudit(AUDIT_LIMIT) });
  // Уровень доступа подключений нужен только кнопке «Открыть в консоли».
  const connections = useQuery({ queryKey: ["connections"], queryFn: () => api.listConnections(), enabled: onOpenInConsole !== undefined });
  const reveal = useMutation({ mutationFn: () => api.revealMcpAudit() });
  return (
    <>
      <div className="toolbar">
        {/* ⌘R в «Настройках» обновляет журнал, когда открыт его раздел (скрытые разделы не считаются). */}
        <button type="button" data-action="refresh" onClick={() => void audit.refetch()}>
          <Icon name="refresh" />
          {t("settings.mcp.audit.refresh")}
        </button>
        <button type="button" disabled={reveal.isPending} onClick={() => reveal.mutate()}>
          {t("settings.mcp.audit.reveal")}
        </button>
      </div>
      {reveal.error && <ErrorBanner error={reveal.error} />}
      {audit.error && <ErrorBanner error={audit.error} onRetry={() => void audit.refetch()} />}
      {audit.data?.length === 0 && <p className="muted">{t("settings.mcp.audit.empty")}</p>}
      {audit.data && audit.data.length > 0 && (
        <div className="table-scroll">
          <table aria-label={t("settings.mcp.audit.title")}>
            <thead>
              <tr>
                <th>{t("settings.mcp.audit.colTime")}</th>
                <th>{t("settings.mcp.audit.colClient")}</th>
                <th>{t("settings.mcp.audit.colWhat")}</th>
                <th>{t("settings.mcp.audit.colConnection")}</th>
                <th>{t("settings.mcp.audit.colResult")}</th>
                <th>{t("settings.mcp.audit.colMs")}</th>
                <th>{t("settings.mcp.audit.colDetails")}</th>
              </tr>
            </thead>
            <tbody>
              {audit.data.map((e, i) => {
                const blocked = onOpenInConsole && isConsoleCall(e) ? openBlocked(e, status, connections.data, connections.isError) : undefined;
                return (
                  <tr key={`${e.ts}-${i}`}>
                    <td>{formatTimestamp(e.ts)}</td>
                    <td>{e.client ?? ""}</td>
                    <td>{whatText(e, t)}</td>
                    <td>{e.connection?.name ?? ""}</td>
                    <td>{resultText(e, t)}</td>
                    <td>{e.ms ?? ""}</td>
                    <td>
                      {detailsText(e, t)}
                      <AuditCost entry={e} />
                      {onOpenInConsole && blocked !== undefined && (
                        <>
                          {" "}
                          <button
                            type="button"
                            className="link"
                            disabled={blocked !== null}
                            title={blocked ? t(blocked) : undefined}
                            onClick={() => onOpenInConsole(e)}
                          >
                            {t("settings.mcp.audit.openInConsole")}
                          </button>
                        </>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}
