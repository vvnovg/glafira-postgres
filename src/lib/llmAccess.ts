import type { ConnectionProfile, LlmAccess, McpStatus } from "../api/types";

/**
 * Разрешает ли потолок политики уровень «Диагностика и данные» (спец. P4 §4.2, §11). Строго: `undefined` — статус MCP ещё
 * не пришёл или не загрузился (третий уровень неактивен без пометки), `false` — потолок `diagnostics`.
 */
export function dataAllowed(status: McpStatus | undefined): boolean | undefined {
  return status === undefined ? undefined : status.maxAccess === "data";
}

/**
 * Действующий уровень доступа модели к подключению (спец. P4 §4.1–4.2): сохранённый `llmAccess` под политикой. MCP
 * запрещён политикой — `off`. Статус MCP неизвестен — уровень профиля, но `data` только при загруженном статусе с потолком
 * `data`, иначе — как «Диагностика».
 */
export function effectiveLlmAccess(profile: Pick<ConnectionProfile, "llmAccess">, status: McpStatus | undefined): LlmAccess {
  if (status && !status.policyEnabled) return "off";
  const level = profile.llmAccess ?? "off";
  return level === "data" && dataAllowed(status) !== true ? "diagnostics" : level;
}
