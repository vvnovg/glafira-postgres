import type { ChapterId } from "../../guide/chapters";
import type { TabSpec } from "../../state/workspace";

/** Глава руководства для экрана (F1): «О программе» → «Обновления и диагностика», остальное — «Начало работы». */
export function helpChapterFor(tab: TabSpec | null): ChapterId {
  return tab?.kind === "about" ? "updates" : "getting-started";
}
