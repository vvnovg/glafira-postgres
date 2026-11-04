import { useTranslation } from "react-i18next";
import { useMcpStatus } from "../../lib/useMcpStatus";
import { useWorkspace } from "../../state/workspace";

/**
 * Индикатор внизу боковой панели (спец. P4 §11): «LLM: <клиент>» или «LLM: N клиентов», пока есть сессии MCP; во время
 * вызова подсвечен. Клиенты считаются по имени: две сессии одного клиента — один клиент. Щелчок открывает раздел MCP.
 */
export function LlmIndicator() {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const { status } = useMcpStatus();
  if (!status || status.sessions.length === 0) return null;
  const names = [...new Set(status.sessions.map((x) => x.clientName ?? t("mcp.unknownClient")))];
  const active = status.sessions.some((x) => x.active);
  const label = names.length === 1 ? t("mcp.indicator.one", { client: names[0] }) : t("mcp.indicator.clients", { count: names.length });
  return (
    <button
      type="button"
      className={active ? "link llm-indicator active" : "link llm-indicator"}
      title={t("mcp.indicator.title")}
      onClick={() => open({ kind: "settings", section: "mcp" })}
    >
      {label}
    </button>
  );
}
