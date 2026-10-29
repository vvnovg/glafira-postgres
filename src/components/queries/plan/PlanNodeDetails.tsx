import { useTranslation } from "react-i18next";
import type { Buffers, PlanNode } from "../../../api/types";
import { formatCount, formatMs, formatPercent } from "../../../lib/format";
import { KeyValue, type KeyValueItem } from "../../catalog/KeyValue";
import { nodeLabel } from "./planText";

/** Факт узла: время (если оно есть — при `TIMING OFF` его нет), строки и циклы; у невыполнявшегося узла — пометка вместо нулей. */
function actualText(n: PlanNode, labels: { rowsUnit: string; loops: string; neverExecuted: string }): string | null {
  const a = n.actual;
  if (!a) return null;
  if (n.neverExecuted) return labels.neverExecuted;
  const time = a.startupMs !== null && a.totalMs !== null ? `${formatMs(a.startupMs)}..${formatMs(a.totalMs)}, ` : "";
  return `${time}${formatCount(a.rows)} ${labels.rowsUnit}, ${labels.loops} ${formatCount(a.loops)}`;
}

/** Локальные буферы (временные таблицы сессии) — в том же виде, что общие; без обращений к ним строки нет. */
function localBuffers(b: Buffers | null): string | null {
  if (!b || b.localHit + b.localRead + b.localDirtied + b.localWritten === 0) return null;
  return `local hit ${formatCount(b.localHit)}, read ${formatCount(b.localRead)}, dirtied ${formatCount(b.localDirtied)}, written ${formatCount(b.localWritten)}`;
}

/** Панель выбранного узла: все распознанные поля и `extra` как есть. */
export function PlanNodeDetails({ node: n }: { node: PlanNode }) {
  const { t } = useTranslation();
  // Времена и доля невыполнявшегося узла — нули, не данные: их не показываем.
  const timed = !n.neverExecuted;
  const hash = [
    n.hashBatches !== null && `${t("diag.plan.details.batches")} ${n.hashBatches}`,
    n.hashBuckets !== null && `${t("diag.plan.details.buckets")} ${formatCount(n.hashBuckets)}`,
    n.peakMemoryKb !== null && `${formatCount(n.peakMemoryKb)} kB`,
  ].filter((p): p is string => typeof p === "string");
  const items: KeyValueItem[] = [
    { label: t("diag.plan.details.type"), value: nodeLabel(n) },
    { label: t("diag.plan.details.subplan"), value: n.subplanName },
    { label: t("diag.plan.details.parent"), value: n.parentRelationship },
    { label: t("diag.plan.details.cost"), value: `${formatCount(n.startupCost, 2)}..${formatCount(n.totalCost, 2)}` },
    { label: t("diag.plan.details.planRows"), value: `${formatCount(n.planRows)} × ${n.planWidth} B` },
    { label: t("diag.plan.details.actual"), value: actualText(n, { rowsUnit: t("diag.plan.details.rowsUnit"), loops: t("diag.plan.details.loops"), neverExecuted: t("diag.plan.marks.neverExecuted") }) },
    { label: t("diag.plan.details.inclusive"), value: timed && n.inclusiveMs !== null ? formatMs(n.inclusiveMs) : null },
    { label: t("diag.plan.details.exclusive"), value: timed && n.exclusiveMs !== null ? formatMs(n.exclusiveMs) : null },
    { label: t("diag.plan.details.share"), value: timed && n.sharePct !== null ? formatPercent(n.sharePct) : null },
    { label: t("diag.plan.details.filter"), value: n.filter },
    { label: t("diag.plan.details.indexCond"), value: n.indexCond },
    { label: t("diag.plan.details.recheckCond"), value: n.recheckCond },
    { label: t("diag.plan.details.hashCond"), value: n.hashCond },
    { label: t("diag.plan.details.joinFilter"), value: n.joinFilter },
    { label: t("diag.plan.details.mergeCond"), value: n.mergeCond },
    { label: t("diag.plan.details.removedByFilter"), value: n.rowsRemovedByFilter === null ? null : formatCount(n.rowsRemovedByFilter) },
    { label: t("diag.plan.details.removedByRecheck"), value: n.rowsRemovedByIndexRecheck === null ? null : formatCount(n.rowsRemovedByIndexRecheck) },
    { label: t("diag.plan.details.removedByJoinFilter"), value: n.rowsRemovedByJoinFilter === null ? null : formatCount(n.rowsRemovedByJoinFilter) },
    { label: t("diag.plan.details.heapBlocks"), value: n.exactHeapBlocks === null && n.lossyHeapBlocks === null ? null : `${formatCount(n.exactHeapBlocks ?? 0)} / ${formatCount(n.lossyHeapBlocks ?? 0)}` },
    { label: t("diag.plan.details.heapFetches"), value: n.heapFetches === null ? null : formatCount(n.heapFetches) },
    { label: t("diag.plan.details.sortKey"), value: n.sortKey.length ? n.sortKey.join(", ") : null },
    { label: t("diag.plan.details.groupKey"), value: n.groupKey.length ? n.groupKey.join(", ") : null },
    { label: t("diag.plan.details.sortMethod"), value: n.sortMethod ? `${n.sortMethod}${n.sortSpaceType ? ` (${n.sortSpaceType} ${formatCount(n.sortSpaceKb ?? 0)} kB)` : ""}` : null },
    { label: t("diag.plan.details.hash"), value: hash.length ? hash.join(", ") : null },
    { label: t("diag.plan.details.workers"), value: n.workersPlanned === null ? null : `${n.workersLaunched ?? "—"} / ${n.workersPlanned}` },
    { label: t("diag.plan.details.subplansRemoved"), value: n.subplansRemoved === null ? null : formatCount(n.subplansRemoved) },
    {
      label: t("diag.plan.details.buffers"),
      value: n.buffers
        ? `shared hit ${formatCount(n.buffers.sharedHit)}, read ${formatCount(n.buffers.sharedRead)}, dirtied ${formatCount(n.buffers.sharedDirtied)}, written ${formatCount(n.buffers.sharedWritten)}; temp read ${formatCount(n.buffers.tempRead)}, written ${formatCount(n.buffers.tempWritten)}`
        : null,
    },
    { label: t("diag.plan.details.buffersLocal"), value: localBuffers(n.buffers) },
    { label: t("diag.plan.details.io"), value: n.ioReadMs === null && n.ioWriteMs === null ? null : `${formatMs(n.ioReadMs ?? 0)} / ${formatMs(n.ioWriteMs ?? 0)}` },
    { label: t("diag.plan.details.output"), value: n.output.length ? n.output.join(", ") : null },
  ].filter((i) => i.value !== null && i.value !== undefined);
  const extra = Object.entries(n.extra);
  return (
    <section className="plan-details" aria-label={t("diag.plan.details.title")}>
      <h3>{t("diag.plan.details.title")}</h3>
      <KeyValue items={items} />
      {extra.length > 0 && (
        <details>
          <summary>{t("diag.plan.details.extra", { count: extra.length })}</summary>
          <pre>{JSON.stringify(n.extra, null, 2)}</pre>
        </details>
      )}
    </section>
  );
}
