import type { ConnectionProfile, McpAuditEntry, McpStatus } from "../../api/types";
import { effectiveLlmAccess } from "../../lib/llmAccess";

/**
 * Вызовы, которые можно открыть в консоли EXPLAIN (спец. P4 §11): `explain_query` (P4a) и `run_select` (P4b). Консоль
 * получает тот же SQL, базу и узел и сразу строит «План» без выполнения (итоговая волна правок смоука P4c).
 */
const CONSOLE_TOOLS = new Set(["explain_query", "run_select"]);

/** Поле `sql` журнал пишет целиком до 16 КБ (`MAX_SQL_AUDIT_BYTES`): текст такой длины мог быть обрезан. */
export const SQL_AUDIT_LIMIT_BYTES = 16_384;

/** Что подставить в консоль: подключение, база, SQL и узел вызова (без узла — «за лидером»). */
export interface ConsoleTarget {
  connectionId: string;
  /** Не задана — консоль берёт базу подключения по умолчанию (профиль). */
  database?: string;
  sql: string;
  member?: string;
}

/** Успешный вызов explain_query / run_select: у записи журнала есть кнопка «Открыть в консоли EXPLAIN». */
export function isConsoleCall(e: McpAuditEntry): boolean {
  return e.event === "call" && e.outcome === "ok" && CONSOLE_TOOLS.has(e.tool ?? "");
}

/** Строка без пробелов по краям; пустая и из пробелов — «не задана» (как в бэкенде, `resolve_database`). */
const str = (v: unknown): string | null => {
  if (typeof v !== "string") return null;
  const t = v.trim();
  return t === "" ? null : t;
};

/**
 * Цель «Открыть в консоли» из аргументов вызова или ключ подсказки, почему её нет: усечённые аргументы журнал помечает
 * `_truncated: true`, `sql` длиной в предел мог быть обрезан, вызов по `queryId` текста запроса в журнале не оставил.
 * Вызов без `database` (параметр необязателен; пустая строка — то же) открывается без базы: консоль EXPLAIN берёт
 * базу подключения по умолчанию (профиль, она же `database` в `list_connections`).
 */
export function consoleTarget(e: McpAuditEntry): { target: ConsoleTarget } | { blocked: string } {
  const a = e.args;
  if (!a || typeof a !== "object" || Array.isArray(a)) return { blocked: "settings.mcp.audit.openArgsCut" };
  const args = a as Record<string, unknown>;
  // Формат журнала (задача 4): обрезанные аргументы помечены `_truncated: true`.
  if (args._truncated === true) return { blocked: "settings.mcp.audit.openArgsCut" };
  const sql = str(args.sql);
  if (sql === null) return { blocked: str(args.queryId) ? "settings.mcp.audit.openNoSql" : "settings.mcp.audit.openArgsCut" };
  if (new TextEncoder().encode(sql).length >= SQL_AUDIT_LIMIT_BYTES) return { blocked: "settings.mcp.audit.openArgsCut" };
  const connectionId = e.connection?.id ?? str(args.connectionId);
  if (connectionId === null) return { blocked: "settings.mcp.audit.openArgsCut" };
  const database = str(args.database) ?? undefined;
  const member = str(args.member) ?? e.member ?? undefined;
  return { target: { connectionId, ...(database ? { database } : {}), sql, ...(member ? { member } : {}) } };
}

/**
 * Почему «Открыть в консоли EXPLAIN» неактивна: ключ подсказки; "" — список подключений ещё грузится (кнопка неактивна
 * без подсказки); null — можно открыть. Порядок — как в спеке §11: аргументы, подключение, уровень доступа.
 */
export function openBlocked(
  e: McpAuditEntry,
  status: McpStatus,
  connections: ConnectionProfile[] | undefined,
  connectionsFailed: boolean,
): string | null {
  const t = consoleTarget(e);
  if ("blocked" in t) return t.blocked;
  if (!connections) return connectionsFailed ? "settings.mcp.audit.openConnectionsFailed" : "";
  const conn = connections.find((c) => c.id === t.target.connectionId);
  if (!conn) return "settings.mcp.audit.openNoConnection";
  return effectiveLlmAccess(conn, status) === "off" ? "settings.mcp.audit.openOff" : null;
}
