import type { LlmBundle, LlmProposal, OptimizationRequest } from "../api/types";

/** Снимок `LlmBundle` консоли: план есть, статистики нет. */
export function llmBundle(patch: Partial<LlmBundle> = {}): LlmBundle {
  return {
    serverVersionNum: 170_004,
    pgssVersion: "1.11",
    inRecovery: false,
    member: "pg-1",
    database: "perf",
    text: "SELECT count(*) FROM shop.orders WHERE status = 'disputed'",
    textTruncated: false,
    stats: null,
    planText: "Aggregate  (cost=10.00..10.01 rows=1 width=8)\n  ->  Seq Scan on orders\n",
    tables: [],
    tablesOmitted: 0,
    warnings: [],
    ...patch,
  };
}

/** Запрос на оптимизацию из консоли подключения `c`, ждёт ответа. */
export function optimizationRequest(patch: Partial<OptimizationRequest> = {}): OptimizationRequest {
  return {
    id: "r3a9f1c",
    connectionId: "c",
    member: "pg-1",
    database: "perf",
    source: { kind: "console" },
    sql: "SELECT count(*) FROM shop.orders WHERE status = 'disputed'",
    params: [],
    bundle: llmBundle(),
    createdAtMs: 1_700_000_000_000,
    status: "waiting",
    proposalIds: [],
    ...patch,
  };
}

/** Предложение модели на запрос `r3a9f1c`: новое, без индексов и сравнения. */
export function llmProposal(patch: Partial<LlmProposal> = {}): LlmProposal {
  return {
    id: "p1b2c3d",
    source: "mcp",
    client: "qwen-code",
    requestId: "r3a9f1c",
    connectionId: "c",
    database: "perf",
    member: "pg-1",
    originalSql: "SELECT count(*) FROM shop.orders WHERE status = 'disputed'",
    originalQueryId: null,
    rewrittenSql: "SELECT count(*) FROM shop.orders o WHERE o.status = 'disputed'",
    indexes: [],
    rationale: "Filter on an indexed column",
    expectedEffect: null,
    status: "new",
    reason: null,
    comparison: null,
    createdAtMs: 1_700_000_000_000,
    answeredAtMs: null,
    ...patch,
  };
}
