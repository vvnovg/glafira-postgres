import type { ChapterId } from "../../guide/chapters";
import type { TabSpec } from "../../state/workspace";

/**
 * Глава руководства для экрана (F1): форма подключения → «Подключения», вкладки объектов → «Дерево и объекты»,
 * инструменты диагностики, журнал действий, карточка запроса и консоль EXPLAIN → «Диагностика» (глава «Запросы» появится
 * вместе с руководством и переключит две последние вкладки на `queries`), «О программе» → «Обновления и диагностика»,
 * остальное — «Начало работы».
 */
export function helpChapterFor(tab: TabSpec | null): ChapterId {
  switch (tab?.kind) {
    case "about":
      return "updates";
    case "connection-form":
      return "connections";
    case "diag":
    case "actions":
    case "query":
    case "explain":
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
 * Якорь главы для вкладки (F1): у инструмента диагностики — его раздел («Idle in transaction» описан в «Сессиях»),
 * у журнала действий — `journal`, у карточки запроса и консоли EXPLAIN — `queries`.
 */
export function helpAnchorFor(tab: TabSpec | null): string | undefined {
  if (tab?.kind === "actions") return "journal";
  if (tab?.kind === "query" || tab?.kind === "explain") return "queries";
  if (tab?.kind !== "diag") return undefined;
  return tab.tool === "idleTx" ? "sessions" : tab.tool;
}
