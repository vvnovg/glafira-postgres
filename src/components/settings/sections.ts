import type { ChapterId } from "../../guide/chapters";
import type { SettingsSection } from "../../state/workspace";

/** Разделы группы «LLM-ассистенты (MCP)» в порядке навигации. */
export const MCP_SECTIONS = ["mcp", "agent", "sessions", "audit"] as const satisfies readonly SettingsSection[];

/** Все разделы «Настроек» в порядке навигации. */
export const SETTINGS_SECTIONS = ["language", "display", ...MCP_SECTIONS] as const satisfies readonly SettingsSection[];

/**
 * Подпись пункта навигации — она же заголовок раздела. Ключи — литералами (правило словарей). Где подпись совпадает с
 * названием, на которое ссылается руководство (`[[ui:…]]`), — тот же ключ, чтобы названия не разошлись.
 */
export const SECTION_LABELS: Record<SettingsSection, string> = {
  language: "settings.language.section",
  display: "settings.display.section",
  mcp: "settings.nav.mcpServer",
  agent: "settings.mcp.agent.title",
  sessions: "settings.nav.sessions",
  audit: "settings.mcp.audit.title",
};

/**
 * F1 (спец. P4 §11, P4c §8.3) — по выбранному разделу: «Язык» и «Отображение» описаны в «Начале работы», разделы MCP — в
 * главе «LLM-ассистенты (MCP)»: сервер — «Включение» (за ним «Подключение клиента»), фоновый ассистент — `background`,
 * сессии, клиенты и журнал — «Сессии, отзыв и журнал».
 */
export const SECTION_HELP: Record<SettingsSection, { chapter: ChapterId; anchor?: string }> = {
  language: { chapter: "getting-started" },
  display: { chapter: "getting-started" },
  mcp: { chapter: "llm-mcp", anchor: "enable" },
  agent: { chapter: "llm-mcp", anchor: "background" },
  sessions: { chapter: "llm-mcp", anchor: "sessions-log" },
  audit: { chapter: "llm-mcp", anchor: "sessions-log" },
};
