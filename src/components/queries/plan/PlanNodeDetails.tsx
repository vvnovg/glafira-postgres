import { useTranslation } from "react-i18next";
import type { PlanNode } from "../../../api/types";
import { formatCount, formatMs } from "../../../lib/format";
import { KeyValue, type KeyValueItem } from "../../catalog/KeyValue";
import { nodeLabel } from "./planText";

/** Факт узла: время (если оно есть — при `TIMING OFF` его нет), строки и циклы. */
function actualText(n: PlanNode, rowsUnit: string, loopsLabel: string): string | null {
  const a = n.actual;
  if (!a) return null;
  const time = a.startupMs !== null && a.totalMs !== null ? `${formatMs(a.startupMs)}..${formatMs(a.totalMs)}, ` : "";
  return `${time}${formatCount(a.rows)} ${rowsUnit}, ${loopsLabel} ${formatCount(a.loops)}`;
}

/** Панель выбранного узла: все распознанные поля и `extra` как есть. */
export function PlanNodeDetails({ node: n }: { node: PlanNode }) {
  const { t } = useTranslation();
  const items: KeyValueItem[] = [
    { label: t("diag.plan.details.type"), value: nodeLabel(n) },
    { label: t("diag.plan.details.parent"), value: n.parentRelationship },
    { label: t("diag.plan.details.cost"), value: `${n.startupCost.toFixed(2)}..${n.totalCost.toFixed(2)}` },
    { label: t("diag.plan.details.planRows"), value: `${formatCount(n.planRows)} × ${n.planWidth} B` },
    { label: t("diag.plan.details.actual"), value: actualText(n, t("diag.plan.details.rowsUnit"), t("diag.plan.details.loops")) },
    { label: t("diag.plan.details.exclusive"), value: n.exclusiveMs === null ? null : formatMs(n.exclusiveMs) },
    { label: t("diag.plan.details.filter"), value: n.filter },
    { label: t("diag.plan.details.indexCond"), value: n.indexCond },
    { label: t("diag.plan.details.recheckCond"), value: n.recheckCond },
    { label: t("diag.plan.details.hashCond"), value: n.hashCond },
    { label: t("diag.plan.details.joinFilter"), value: n.joinFilter },
    { label: t("diag.plan.details.mergeCond"), value: n.mergeCond },
    { label: t("diag.plan.details.removedByFilter"), value: n.rowsRemovedByFilter === null ? null : formatCount(n.rowsRemovedByFilter) },
    { label: t("diag.plan.details.removedByRecheck"), value: n.rowsRemovedByIndexRecheck === null ? null : formatCount(n.rowsRemovedByIndexRecheck) },
    { label: t("diag.plan.details.heapFetches"), value: n.heapFetches === null ? null : formatCount(n.heapFetches) },
    { label: t("diag.plan.details.sortKey"), value: n.sortKey.length ? n.sortKey.join(", ") : null },
    { label: t("diag.plan.details.sortMethod"), value: n.sortMethod ? `${n.sortMethod}${n.sortSpaceType ? ` (${n.sortSpaceType} ${formatCount(n.sortSpaceKb ?? 0)} kB)` : ""}` : null },
    { label: t("diag.plan.details.hash"), value: n.hashBatches === null ? null : `${t("diag.plan.details.batches")} ${n.hashBatches}, ${formatCount(n.peakMemoryKb ?? 0)} kB` },
    { label: t("diag.plan.details.workers"), value: n.workersPlanned === null ? null : `${n.workersLaunched ?? "—"} / ${n.workersPlanned}` },
    {
      label: t("diag.plan.details.buffers"),
      value: n.buffers
        ? `shared hit ${formatCount(n.buffers.sharedHit)}, read ${formatCount(n.buffers.sharedRead)}, dirtied ${formatCount(n.buffers.sharedDirtied)}, written ${formatCount(n.buffers.sharedWritten)}; temp read ${formatCount(n.buffers.tempRead)}, written ${formatCount(n.buffers.tempWritten)}`
        : null,
    },
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
