import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { McpStatus } from "../../api/types";
import { formatTimestamp } from "../../lib/format";
import { MCP_AUDIT_KEY, MCP_STATUS_KEY } from "../../lib/useMcpStatus";
import { ErrorBanner } from "../common/ErrorBanner";
import { clientLabel } from "./text";

/** Активные сессии MCP (спец. P4 §11): клиент, начало, число вызовов; «Отключить» и «Отключить всех». */
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
                const client = clientLabel(x.clientName, x.clientVersion, t("mcp.unknownClient"));
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
