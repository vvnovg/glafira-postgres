import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { McpSessionSnapshot, McpStatus } from "../../api/types";
import { formatTimestamp } from "../../lib/format";
import { MCP_AUDIT_KEY, MCP_STATUS_KEY } from "../../lib/useMcpStatus";
import { ErrorBanner } from "../common/ErrorBanner";
import { clientLabel } from "./text";

/**
 * Подпись сессии: сессия фонового запуска (спец. P4c §8.2) — «Фоновый ассистент · запрос r…» (у «Проверить» —
 * «… · проверка»; запуск уже кончился — просто «Фоновый ассистент»), остальные — клиент и версия.
 */
function sessionLabel(x: McpSessionSnapshot, status: McpStatus, t: TFunction): string {
  if (!x.runId) return clientLabel(x.clientName, x.clientVersion, t("mcp.unknownClient"));
  const running = status.agent?.running;
  if (!running || running.runId !== x.runId) return t("settings.mcp.sessions.runOther");
  return running.requestId ? t("settings.mcp.sessions.run", { id: running.requestId }) : t("settings.mcp.sessions.runCheck");
}

/**
 * Активные сессии MCP (спец. P4 §11): клиент, начало, число вызовов; «Отключить» и «Отключить всех». Сессия фонового
 * запуска подписана «Фоновый ассистент · запрос r…»; «Отключить» закрывает её, и агент заканчивается с ошибками вызовов —
 * остановить запуск можно в «Запусках» (спец. P4c §8.2).
 */
export function McpSessions({ status }: { status: McpStatus }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const disconnect = useMutation({
    mutationFn: (sessionId?: string) => api.disconnectMcpSessions(sessionId),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: MCP_STATUS_KEY, exact: true });
      void qc.invalidateQueries({ queryKey: MCP_AUDIT_KEY });
    },
  });
  const pending = new Set(status.pending.map((p) => p.sessionId));
  return (
    <>
      <h4>{t("settings.mcp.sessions.title")}</h4>
      {status.sessions.length === 0 ? (
        <p className="muted">{t("settings.mcp.sessions.none")}</p>
      ) : (
        <>
          <table aria-label={t("settings.mcp.sessions.title")}>
            <thead>
              <tr>
                <th>{t("settings.mcp.sessions.colClient")}</th>
                <th>{t("settings.mcp.sessions.colSince")}</th>
                <th>{t("settings.mcp.sessions.colCalls")}</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {status.sessions.map((x) => {
                const client = sessionLabel(x, status, t);
                return (
                  <tr key={x.id}>
                    <td>
                      {client}
                      {pending.has(x.id) && <span className="muted"> · {t("settings.mcp.sessions.waiting")}</span>}
                    </td>
                    <td>{formatTimestamp(x.startedMs)}</td>
                    <td>{x.calls}</td>
                    <td>
                      <button
                        type="button"
                        aria-label={t("settings.mcp.sessions.disconnectAria", { client })}
                        disabled={disconnect.isPending}
                        onClick={() => disconnect.mutate(x.id)}
                      >
                        {t("settings.mcp.sessions.disconnect")}
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          <button type="button" disabled={disconnect.isPending} onClick={() => disconnect.mutate(undefined)}>
            {t("settings.mcp.sessions.disconnectAll")}
          </button>
        </>
      )}
      {disconnect.error && <ErrorBanner error={disconnect.error} />}
    </>
  );
}
