import type { AgentStatus, McpAuditEntry, McpClientConfigs, McpStatus } from "../api/types";

/** Фоновый ассистент в статусе MCP: политика разрешает, режим выключен, агент не настроен, очереди нет. */
export function agentStatus(patch: Partial<AgentStatus> = {}): AgentStatus {
  return { policyEnabled: true, enabled: false, configured: false, preset: "qwen", running: null, queued: 0, ...patch };
}

/** Статус MCP для тестов: сервер работает, политика разрешает, открыто два подключения, сессий и запросов нет. */
export function mcpStatus(patch: Partial<McpStatus> = {}): McpStatus {
  return {
    server: "running",
    error: null,
    enabled: patch.server !== "stopped",
    sessions: [],
    pending: [],
    approvedClients: [],
    openConnections: 2,
    dataConnections: 0,
    policyEnabled: true,
    maxAccess: "data",
    agent: agentStatus(),
    ...patch,
  };
}

const EXE = "/Applications/Glafira for PostgreSQL.app/Contents/MacOS/glafira-pg";

/** Фрагменты конфигураций клиентов: путь с пробелами, как у установленного приложения на macOS. */
export const MCP_CONFIGS: McpClientConfigs = {
  command: EXE,
  args: ["mcp"],
  qwenCode: `qwen mcp add glafira-pg "${EXE}" mcp`,
  qwenSettings: `{\n  "mcpServers": {\n    "glafira-pg": { "command": "${EXE}", "args": ["mcp"] }\n  }\n}`,
  claudeCode: `claude mcp add glafira-pg -- "${EXE}" mcp`,
  claudeDesktop: `{\n  "mcpServers": {\n    "glafira-pg": { "command": "${EXE}", "args": ["mcp"] }\n  }\n}`,
  cursor: `{\n  "mcpServers": {\n    "glafira-pg": { "command": "${EXE}", "args": ["mcp"] }\n  }\n}`,
};

/** Успешный вызов explain_query в журнале: подключение c1 «perf», база perf, узел pg-2. */
export function explainCall(patch: Partial<McpAuditEntry> = {}): McpAuditEntry {
  return {
    ts: 1_700_000_000_000,
    event: "call",
    session: "s1",
    client: "qwen-code",
    tool: "explain_query",
    connection: { id: "c1", name: "perf" },
    member: "pg-2",
    args: { connectionId: "c1", member: "pg-2", database: "perf", sql: "SELECT * FROM shop.orders WHERE status = 'new'" },
    outcome: "ok",
    code: null,
    bytes: 2048,
    ms: 41,
    ...patch,
  };
}
