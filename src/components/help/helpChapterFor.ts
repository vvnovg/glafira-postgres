import type { ChapterId } from "../../guide/chapters";
import type { TabSpec } from "../../state/workspace";

/**
 * Глава руководства для экрана (F1): форма подключения → «Подключения», вкладки объектов → «Дерево и объекты»,
 * «О программе» → «Обновления и диагностика», остальное — «Начало работы».
 */
export function helpChapterFor(tab: TabSpec | null): ChapterId {
  switch (tab?.kind) {
    case "about":
      return "updates";
    case "connection-form":
      return "connections";
    case "table":
    case "view":
    case "function":
    case "sequence":
      return "objects";
    default:
      return "getting-started";
  }
}
