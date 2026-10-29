import type { PlanNode, PlanTree } from "../api/types";

export function planNode(patch: Partial<PlanNode> = {}): PlanNode {
  return {
    id: 0, nodeType: "Seq Scan", parentRelationship: null, subplanName: null, relation: { schema: "shop", name: "orders", alias: "o" }, indexName: null, strategy: null, joinType: null, parallelAware: false,
    startupCost: 0, totalCost: 60, planRows: 500, planWidth: 8, actual: { startupMs: 0.1, totalMs: 30, rows: 5, loops: 1 }, neverExecuted: false,
    workersPlanned: null, workersLaunched: null, rowsRemovedByFilter: 499_995, rowsRemovedByIndexRecheck: null, rowsRemovedByJoinFilter: null, heapFetches: null,
    filter: "(o.status = 'disputed'::text)", indexCond: null, recheckCond: null, hashCond: null, joinFilter: null, mergeCond: null, sortKey: [], groupKey: [],
    sortMethod: null, sortSpaceKb: null, sortSpaceType: null, hashBatches: null, hashBuckets: null, peakMemoryKb: null, exactHeapBlocks: null, lossyHeapBlocks: null, subplansRemoved: null,
    buffers: { sharedHit: 80, sharedRead: 20, sharedDirtied: 0, sharedWritten: 0, localHit: 0, localRead: 0, localDirtied: 0, localWritten: 0, tempRead: 0, tempWritten: 0 },
    ioReadMs: null, ioWriteMs: null, output: [], extra: {}, inclusiveMs: 30, exclusiveMs: 30, sharePct: 54.5, estimateRatio: 0.01, hot: true, misestimated: true, children: [], ...patch,
  };
}

/** Hash Join над Seq Scan и Index Scan, с ANALYZE. */
export function planTree(patch: Partial<PlanTree> = {}): PlanTree {
  const seq = planNode({ id: 1, parentRelationship: "Outer" });
  const idx = planNode({
    id: 2, nodeType: "Index Scan", parentRelationship: "Inner", indexName: "customers_pkey", relation: { schema: "shop", name: "customers", alias: "c" },
    totalCost: 4, planRows: 100, actual: { startupMs: 0.01, totalMs: 0.4, rows: 100, loops: 1 }, rowsRemovedByFilter: null, filter: null, indexCond: "(c.id < 100)",
    buffers: null, inclusiveMs: 0.4, exclusiveMs: 0.4, sharePct: 0.7, estimateRatio: 1, hot: false, misestimated: false,
  });
  const root = planNode({
    id: 0, nodeType: "Hash Join", joinType: "Inner", relation: null, totalCost: 100, planRows: 1000, actual: { startupMs: 1, totalMs: 50, rows: 10, loops: 1 },
    rowsRemovedByFilter: null, filter: null, hashCond: "(o.customer_id = c.id)", buffers: null, inclusiveMs: 50, exclusiveMs: 19.6, sharePct: 35.6, estimateRatio: 0.01, hot: true, misestimated: false, children: [seq, idx],
  });
  return { root, planningMs: 0.5, executionMs: 55, triggers: [], jit: null, settings: [{ name: "work_mem", value: "1MB" }], totalMs: 55, ...patch };
}
