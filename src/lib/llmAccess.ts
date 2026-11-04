import type { ConnectionProfile, LlmAccess, McpStatus } from "../api/types";

/**
 * Действующий уровень доступа модели к подключению (спец. P4 §4.1–4.2): сохранённый `llmAccess` под политикой. MCP
 * запрещён политикой — `off`. Статус MCP неизвестен (ещё грузится) — уровень профиля. Потолок `mcpMaxAccess` в P4a уровень
 * не снижает: ниже «Диагностики» он не бывает; P4b добавит здесь уровень `data` и его потолок.
 */
export function effectiveLlmAccess(profile: Pick<ConnectionProfile, "llmAccess">, status: McpStatus | undefined): LlmAccess {
  if (status && !status.policyEnabled) return "off";
  return profile.llmAccess ?? "off";
}
