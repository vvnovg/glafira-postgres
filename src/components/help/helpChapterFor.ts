import type { ChapterId } from "../../guide/chapters";
import type { TabSpec } from "../../state/workspace";

/**
 * Глава руководства для экрана (F1): форма подключения → «Подключения», вкладки объектов → «Дерево и объекты»,
 * инструмент «Запросы», карточка запроса и консоль EXPLAIN → «Запросы», остальные инструменты диагностики и журнал
 * действий → «Диагностика», «О программе» → «Обновления и диагностика», остальное — «Начало работы».
 */
export function helpChapterFor(tab: TabSpec | null): ChapterId {
  switch (tab?.kind) {
    case "about":
      return "updates";
    case "connection-form":
      return "connections";
    case "diag":
      return tab.tool === "queries" ? "queries" : "diagnostics";
    case "query":
    case "explain":
      return "queries";
    case "actions":
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

/**
 * Якорь главы для вкладки (F1): у инструмента диагностики — его раздел («Idle in transaction» описан в «Сессиях»;
 * «Запросы» — `top` главы «Запросы»), у журнала действий — `journal`, у карточки запроса — `card`, у консоли EXPLAIN — `console`.
 */
export function helpAnchorFor(tab: TabSpec | null): string | undefined {
  if (tab?.kind === "actions") return "journal";
  if (tab?.kind === "query") return "card";
  if (tab?.kind === "explain") return "console";
  if (tab?.kind !== "diag") return undefined;
  if (tab.tool === "queries") return "top";
  return tab.tool === "idleTx" ? "sessions" : tab.tool;
}
