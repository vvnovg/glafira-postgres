import type { ReadinessItem } from "../api/types";

type Translate = (k: string, o?: Record<string, unknown>) => string;

/**
 * Параметры текста пункта `llmRole` (спец. P4 §4.5): к `role` и `memberships` добавляется `abilities` — что модель смогла бы
 * сделать через `run_select` с этими правами, на языке интерфейса. Остальные пункты — как есть.
 */
export function llmRoleParams(item: ReadinessItem, t: Translate): Record<string, string> {
  if (item.id !== "llmRole" || item.status === "ok") return item.params;
  const rights = item.params.reason === "superuser" ? ["superuser"] : (item.params.memberships ?? "").split(", ").filter(Boolean);
  return { ...item.params, abilities: rights.map((r) => t(`readiness.llmRole.ability.${r}`)).join(", ") };
}
