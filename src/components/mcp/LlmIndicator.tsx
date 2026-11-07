import { useTranslation } from "react-i18next";
import { useMcpStatus } from "../../lib/useMcpStatus";
import { useWorkspace } from "../../state/workspace";
import { Icon } from "../common/Icon";
import { TreeRow } from "../connections/tree/TreeRow";

/**
 * Индикатор внизу боковой панели (спец. P4 §11, P4c §8.2): «LLM: <клиент>» или «LLM: N клиентов», пока есть сессии MCP;
 * во время фонового запуска (`agent.running`, в том числе до первого подключения агента) — «LLM: фоновый ассистент». Во
 * время вызова подсвечен. Клиенты считаются по имени: две сессии одного клиента — один клиент; сессии фонового запуска
 * клиентами чата не считаются. Щелчок открывает раздел MCP, а во время запуска — подраздел «Фоновый ассистент».
 */
export function LlmIndicator() {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const { status } = useMcpStatus();
  if (!status) return null;
  // `?.` — статус без поля `agent` (бэкенд без фонового ассистента) не роняет боковую панель.
  const running = status.agent?.running != null;
  const chat = status.sessions.filter((x) => !x.runId);
  if (!running && chat.length === 0) return null;
  const active = status.sessions.some((x) => x.active);
  const names = [...new Set(chat.map((x) => x.clientName ?? t("mcp.unknownClient")))];
  const label = running ? t("mcp.indicator.agent") : names.length === 1 ? t("mcp.indicator.one", { client: names[0] }) : t("mcp.indicator.clients", { count: names.length });
  // Строка нижней панели, как McpIndicator Kafka-версии (спец. U1 §6): точка подсвечивается акцентом во время вызова.
  return (
    <TreeRow
      chevron="none"
      className={active ? "mcp-indicator llm-indicator active" : "mcp-indicator llm-indicator"}
      icon={<Icon name="dot" size={12} />}
      name={label}
      title={running ? t("mcp.indicator.agentTitle") : t("mcp.indicator.title")}
      onActivate={() => open({ kind: "settings", section: running ? "agent" : "mcp" })}
    />
  );
}
