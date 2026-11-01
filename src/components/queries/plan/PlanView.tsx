import { useState, type ReactNode } from "react";
import type { ExplainMode, ExplainTier, PlanNode, PlanTree } from "../../../api/types";
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
 * где план получен, если он не узел вкладки (сводка его называет). `afterSummary` — блок между сводкой и деревом (рекомендации);
 * он получает `showNode(ids)`: первый узел выбирается, все `ids` помечаются как относящиеся к совету (дерево раскрывает их
 * предков и прокручивает к выбранному); клик по строке пометку снимает.
 */
export function PlanView({
  tree,
  tier,
  mode,
  warnings,
  raw,
  elapsedMs,
  member,
  extraActions,
  afterSummary,
}: {
  tree: PlanTree;
  tier?: ExplainTier;
  mode?: ExplainMode;
  warnings?: string[];
  raw?: unknown;
  elapsedMs?: number;
  member?: string;
  extraActions?: ReactNode;
  afterSummary?: (showNode: (ids: number[]) => void) => ReactNode;
}) {
  // `reveal` — выбор пришёл не кликом по строке (а «Показать узел»): дерево раскроет предков и прокрутит к узлу.
  const [pick, setPick] = useState<{ tree: PlanTree; id: number; reveal?: boolean; related?: ReadonlySet<number> } | null>(null);
  // Другой план — выбор забыт совсем (возврат к прежнему плану истории не воскрешает его): сброс при отрисовке, без эффекта.
  if (pick && pick.tree !== tree) setPick(null);
  const selectedId = pick && pick.tree === tree ? pick.id : null;
  const node = findNode(tree.root, selectedId);
  return (
    <>
      <PlanSummary tree={tree} tier={tier} mode={mode} warnings={warnings} raw={raw} elapsedMs={elapsedMs} member={member} extraActions={extraActions} />
      {afterSummary?.((ids) => {
        if (ids.length > 0) setPick({ tree, id: ids[0], reveal: true, related: new Set(ids) });
      })}
      <PlanTreeView tree={tree} selectedId={selectedId} reveal={pick?.reveal && pick.tree === tree ? pick : null} related={pick?.tree === tree ? pick.related : undefined} onSelect={(id) => setPick({ tree, id })} />
      {node && <PlanNodeDetails node={node} />}
    </>
  );
}
