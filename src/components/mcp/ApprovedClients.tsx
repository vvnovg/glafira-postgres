import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { McpStatus } from "../../api/types";
import { formatTimestamp } from "../../lib/format";
import { MCP_AUDIT_KEY, MCP_STATUS_KEY } from "../../lib/useMcpStatus";
import { ErrorBanner } from "../common/ErrorBanner";

/**
 * Подтверждённые клиенты (спец. P4 §4.4, §11): имя, версия, когда впервые и в последний раз; «Отозвать» забывает клиента
 * и закрывает его сессии — при следующем подключении он снова попросит подтверждение.
 */
export function ApprovedClients({ status }: { status: McpStatus }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const revoke = useMutation({
    mutationFn: (name: string) => api.revokeMcpClient(name),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: MCP_STATUS_KEY, exact: true });
      void qc.invalidateQueries({ queryKey: MCP_AUDIT_KEY });
    },
  });
  return (
    <>
      <h4>{t("settings.mcp.clients.title")}</h4>
      {status.approvedClients.length === 0 ? (
        <p className="muted">{t("settings.mcp.clients.none")}</p>
      ) : (
        <table aria-label={t("settings.mcp.clients.title")}>
          <thead>
            <tr>
              <th>{t("settings.mcp.clients.colClient")}</th>
              <th>{t("settings.mcp.clients.colVersion")}</th>
              <th>{t("settings.mcp.clients.colFirstSeen")}</th>
              <th>{t("settings.mcp.clients.colLastSeen")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {status.approvedClients.map((c) => (
              <tr key={c.name}>
                <td>{c.name}</td>
                <td>{c.version ?? ""}</td>
                <td>{formatTimestamp(c.firstSeenMs)}</td>
                <td>{formatTimestamp(c.lastSeenMs)}</td>
                <td>
                  <button
                    type="button"
                    aria-label={t("settings.mcp.clients.revokeAria", { name: c.name })}
                    disabled={revoke.isPending}
                    onClick={() => revoke.mutate(c.name)}
                  >
                    {t("settings.mcp.clients.revoke")}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {revoke.error && <ErrorBanner error={revoke.error} />}
    </>
  );
}
