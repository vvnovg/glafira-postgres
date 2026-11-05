import type { ConnectionProfile, McpAuditEntry, McpStatus } from "../../api/types";
import { effectiveLlmAccess } from "../../lib/llmAccess";

/**
 * Вызовы, которые можно открыть в консоли EXPLAIN (спец. P4 §11): `explain_query` (P4a) и `run_select` (P4b). Консоль
 * получает тот же SQL, базу и узел и ничего не запускает.
 */
const CONSOLE_TOOLS = new Set(["explain_query", "run_select"]);

/** Поле `sql` журнал пишет целиком до 16 КБ (`MAX_SQL_AUDIT_BYTES`): текст такой длины мог быть обрезан. */
export const SQL_AUDIT_LIMIT_BYTES = 16_384;

/** Что подставить в консоль: подключение, база, SQL и узел вызова (без узла — «за лидером»). */
export interface ConsoleTarget {
  connectionId: string;
  database: string;
  sql: string;
  member?: string;
}

/** Успешный вызов explain_query / run_select: у записи журнала есть кнопка «Открыть в консоли EXPLAIN». */
export function isConsoleCall(e: McpAuditEntry): boolean {
  return e.event === "call" && e.outcome === "ok" && CONSOLE_TOOLS.has(e.tool ?? "");
}

/** Внутренний признак: базу вызова берём из списка подключений, а он ещё не загружен (`openBlocked` его не показывает). */
const PENDING_CONNECTIONS = "pending-connections";

const str = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);

/**
 * Цель «Открыть в консоли» из аргументов вызова или ключ подсказки, почему её нет: усечённые аргументы журнал помечает
 * `_truncated: true`, `sql` длиной в предел мог быть обрезан, вызов по `queryId` текста запроса в журнале не оставил.
 * Вызов без `database` (параметр необязателен) открывается с базой подключения по умолчанию (`pg.database` профиля, она же
 * `database` в `list_connections`): журнал хранит то, что прислала модель, поэтому базу берём из списка подключений;
 * пока список не загружен — `PENDING_CONNECTIONS`, нет подключения в списке — «аргументы усечены» (как раньше без базы).
 */
export function consoleTarget(e: McpAuditEntry, connections?: ConnectionProfile[]): { target: ConsoleTarget } | { blocked: string } {
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
  let database = str(args.database);
  if (database === null) {
    if (!connections) return { blocked: PENDING_CONNECTIONS };
    database = str(connections.find((c) => c.id === connectionId)?.pg.database);
    if (database === null) return { blocked: "settings.mcp.audit.openArgsCut" };
  }
  const member = str(args.member) ?? e.member ?? undefined;
  return { target: member ? { connectionId, database, sql, member } : { connectionId, database, sql } };
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
  const t = consoleTarget(e, connections);
  if ("blocked" in t) return t.blocked === PENDING_CONNECTIONS ? (connectionsFailed ? "settings.mcp.audit.openConnectionsFailed" : "") : t.blocked;
  if (!connections) return connectionsFailed ? "settings.mcp.audit.openConnectionsFailed" : "";
  const conn = connections.find((c) => c.id === t.target.connectionId);
  if (!conn) return "settings.mcp.audit.openNoConnection";
  return effectiveLlmAccess(conn, status) === "off" ? "settings.mcp.audit.openOff" : null;
}
