import { createContext, useContext } from "react";
import { useTranslation } from "react-i18next";
import type { FolderMenuItem } from "./FolderMenu";

/** Ветка, на которой сфокусировано дерево подключений («Сфокусироваться здесь»). */
export type TreeFocus =
  | { kind: "folder"; folderId: string }
  | { kind: "connection"; connectionId: string }
  | { kind: "database"; connectionId: string; db: string }
  | { kind: "schema"; connectionId: string; db: string; schema: string };

/** Переход в фокус на ветке; `null` — узел нарисован вне дерева панели, фокусироваться некуда (пункта меню нет). */
export const TreeFocusContext = createContext<((focus: TreeFocus) => void) | null>(null);

/** Пункт «Сфокусироваться здесь» для меню узла (пустой список вне дерева панели). */
export function useFocusMenuItem(target: TreeFocus): FolderMenuItem[] {
  const { t } = useTranslation();
  const focusOn = useContext(TreeFocusContext);
  return focusOn ? [{ label: t("sidebar.focus.here"), onSelect: () => focusOn(target) }] : [];
}
