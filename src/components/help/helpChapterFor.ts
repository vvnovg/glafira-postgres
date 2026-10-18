import type { ChapterId } from "../../guide/chapters";
import type { TabSpec } from "../../state/workspace";

/** Глава руководства для экрана (F1). В P1a — одна глава; P1b добавит «Подключения» и «Дерево». */
export function helpChapterFor(_tab: TabSpec | null): ChapterId {
  return "getting-started";
}
