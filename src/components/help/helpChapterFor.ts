import type { ChapterId } from "../../guide/chapters";
import type { TabSpec } from "../../state/workspace";

/**
 * Глава руководства для экрана (F1): форма подключения → «Подключения», вкладки объектов → «Дерево и объекты»,
 * инструменты диагностики → «Диагностика», «О программе» → «Обновления и диагностика», остальное — «Начало работы».
 */
export function helpChapterFor(tab: TabSpec | null): ChapterId {
  switch (tab?.kind) {
    case "about":
      return "updates";
    case "connection-form":
      return "connections";
    case "diag":
      return "diagnostics";
    case "table":
    case "view":
    case "function":
    case "sequence":
      return "objects";
    default:
      return "getting-started";
  }
}

/** Якорь главы для вкладки (F1): у инструмента диагностики — его раздел; «Idle in transaction» описан в «Сессиях». */
export function helpAnchorFor(tab: TabSpec | null): string | undefined {
  if (tab?.kind !== "diag") return undefined;
  return tab.tool === "idleTx" ? "sessions" : tab.tool;
}
