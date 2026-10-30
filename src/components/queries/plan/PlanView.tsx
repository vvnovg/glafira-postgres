import { useState } from "react";
import type { ExplainTier, PlanNode, PlanTree } from "../../../api/types";
import { PlanNodeDetails } from "./PlanNodeDetails";
import { PlanSummary } from "./PlanSummary";
import { PlanTreeView } from "./PlanTree";

function findNode(root: PlanNode, id: number | null): PlanNode | null {
  if (id === null) return null;
  if (root.id === id) return root;
  for (const c of root.children) {
    const f = findNode(c, id);
    if (f) return f;
  }
  return null;
}

/**
 * Показ одного плана: сводка, дерево и панель выбранного узла. Выбор живёт здесь и относится к плану, для которого сделан:
 * при смене объекта `tree` (новый запуск, другой план истории, новый разбор) он сбрасывается — номера узлов другого плана другие.
 * Один и тот же объект `tree` между перерисовками обязателен: дерево по его смене сбрасывает и развёртку. `member` — узел,
 * где план получен, если он не узел вкладки (сводка его называет).
 */
export function PlanView({ tree, tier, warnings, raw, elapsedMs, member }: { tree: PlanTree; tier?: ExplainTier; warnings?: string[]; raw?: unknown; elapsedMs?: number; member?: string }) {
  const [pick, setPick] = useState<{ tree: PlanTree; id: number } | null>(null);
  // Другой план — выбор забыт совсем (возврат к прежнему плану истории не воскрешает его): сброс при отрисовке, без эффекта.
  if (pick && pick.tree !== tree) setPick(null);
  const selectedId = pick && pick.tree === tree ? pick.id : null;
  const node = findNode(tree.root, selectedId);
  return (
    <>
      <PlanSummary tree={tree} tier={tier} warnings={warnings} raw={raw} elapsedMs={elapsedMs} member={member} />
      <PlanTreeView tree={tree} selectedId={selectedId} onSelect={(id) => setPick({ tree, id })} />
      {node && <PlanNodeDetails node={node} />}
    </>
  );
}
