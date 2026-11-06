import { useTranslation } from "react-i18next";
import type { ConnectionProfile } from "../../../api/types";
import { effectiveLlmAccess } from "../../../lib/llmAccess";
import { useMcpStatus } from "../../../lib/useMcpStatus";

/**
 * Значок «LLM» в строке подключения (спец. P4 §11) по действующему уровню: контурный — диагностика, заполненный —
 * диагностика и данные. Заполненный — только когда статус MCP пришёл и потолок политики разрешает данные.
 */
export function LlmBadge({ connection }: { connection: Pick<ConnectionProfile, "llmAccess"> }) {
  const { t } = useTranslation();
  const level = effectiveLlmAccess(connection, useMcpStatus().status);
  if (level === "off") return null;
  const title = t(level === "data" ? "sidebar.llmBadgeData" : "sidebar.llmBadge");
  return (
    <span className={level === "data" ? "llm-badge llm-badge--data" : "llm-badge"} role="img" title={title} aria-label={title}>
      LLM
    </span>
  );
}
