import { describe, expect, it } from "vitest";
import { connection } from "../../test/fixtures";
import { explainCall, mcpStatus } from "../../test/mcp";
import { consoleTarget, isConsoleCall, openBlocked, SQL_AUDIT_LIMIT_BYTES } from "./auditOpen";

const conns = [connection({ id: "c1", name: "perf", llmAccess: "diagnostics" }), connection({ id: "c2", name: "closed", llmAccess: "off" })];

describe("auditOpen", () => {
  it("кнопка — только у успешных explain_query и run_select", () => {
    expect(isConsoleCall(explainCall())).toBe(true);
    expect(isConsoleCall(explainCall({ tool: "run_select" }))).toBe(true);
    expect(isConsoleCall(explainCall({ outcome: "error", code: "forbidden_function" }))).toBe(false);
    expect(isConsoleCall(explainCall({ tool: "hypothetical_index" }))).toBe(false);
    expect(isConsoleCall({ ts: 1, event: "approved", session: "s1", client: "qwen-code" })).toBe(false);
  });

  it("цель — SQL, база и узел из аргументов, подключение — из записи", () => {
    expect(consoleTarget(explainCall())).toEqual({
      target: { connectionId: "c1", database: "perf", sql: "SELECT * FROM shop.orders WHERE status = 'new'", member: "pg-2" },
    });
    // Узла в аргументах нет — узел записи; нет и его — «за лидером» (поля member нет).
    const noMember = { connectionId: "c1", database: "perf", sql: "SELECT 1" };
    expect(consoleTarget(explainCall({ args: noMember, member: "pg-3" }))).toEqual({ target: { connectionId: "c1", database: "perf", sql: "SELECT 1", member: "pg-3" } });
    expect(consoleTarget(explainCall({ args: noMember, member: null }))).toEqual({ target: { connectionId: "c1", database: "perf", sql: "SELECT 1" } });
  });

  it("вызов без database (в том числе пустой или из пробелов) открывается без базы: консоль возьмёт базу профиля", () => {
    const want = { target: { connectionId: "c1", sql: "SELECT 1" } };
    for (const database of [undefined, "", "   "]) {
      const args: Record<string, unknown> = { connectionId: "c1", sql: "SELECT 1" };
      if (database !== undefined) args.database = database;
      expect(consoleTarget(explainCall({ args, member: null }))).toEqual(want);
    }
    // Если модель прислала базу — берётся она (без пробелов по краям).
    expect(consoleTarget(explainCall({ args: { connectionId: "c1", database: " perf ", sql: "SELECT 1" }, member: null }))).toEqual({
      target: { connectionId: "c1", database: "perf", sql: "SELECT 1" },
    });
    const s = mcpStatus();
    const noDb = explainCall({ args: { connectionId: "c1", sql: "SELECT 1" } });
    expect(openBlocked(noDb, s, conns, false)).toBeNull();
    expect(openBlocked(noDb, s, undefined, true)).toBe("settings.mcp.audit.openConnectionsFailed");
    expect(openBlocked(noDb, s, undefined, false)).toBe("");
    // Подключения больше нет — подсказка про подключение, а не про усечённые аргументы.
    const gone = explainCall({ args: { connectionId: "c9", sql: "SELECT 1" }, connection: { id: "c9", name: "gone" } });
    expect(openBlocked(gone, s, conns, false)).toBe("settings.mcp.audit.openNoConnection");
  });

  it("усечённые аргументы, SQL на пределе журнала и вызов по queryId не открываются", () => {
    const cut = { blocked: "settings.mcp.audit.openArgsCut" };
    expect(consoleTarget(explainCall({ args: '{"connectionId":"c1","sql":"SELECT …' }))).toEqual(cut);
    expect(consoleTarget(explainCall({ args: { connectionId: "c1", database: "perf", sql: "SELECT 1", _truncated: true, _rest: "{…" } }))).toEqual(cut);
    expect(consoleTarget(explainCall({ args: { connectionId: "c1", database: "perf", sql: "x".repeat(SQL_AUDIT_LIMIT_BYTES) } }))).toEqual(cut);
    // Предел считается в байтах UTF-8, как в журнале: «ж» — два байта.
    expect(consoleTarget(explainCall({ args: { connectionId: "c1", database: "perf", sql: "ж".repeat(SQL_AUDIT_LIMIT_BYTES / 2) } }))).toEqual(cut);
    expect(consoleTarget(explainCall({ args: { connectionId: "c1", database: "perf", queryId: "-42" } }))).toEqual({ blocked: "settings.mcp.audit.openNoSql" });
  });

  it("причины по порядку: аргументы, список подключений, подключение, уровень доступа и политика", () => {
    const s = mcpStatus();
    expect(openBlocked(explainCall(), s, conns, false)).toBeNull();
    expect(openBlocked(explainCall({ args: "cut" }), s, undefined, false)).toBe("settings.mcp.audit.openArgsCut");
    expect(openBlocked(explainCall(), s, undefined, false)).toBe("");
    expect(openBlocked(explainCall(), s, undefined, true)).toBe("settings.mcp.audit.openConnectionsFailed");
    expect(openBlocked(explainCall({ connection: { id: "c9", name: "gone" } }), s, conns, false)).toBe("settings.mcp.audit.openNoConnection");
    expect(openBlocked(explainCall({ connection: { id: "c2", name: "closed" } }), s, conns, false)).toBe("settings.mcp.audit.openOff");
    expect(openBlocked(explainCall(), mcpStatus({ policyEnabled: false }), conns, false)).toBe("settings.mcp.audit.openOff");
  });
});
