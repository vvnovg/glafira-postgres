import type { ObjectKind } from "../../../api/types";
import { type Tab, useWorkspace } from "../../../state/workspace";

/**
 * Узел активной вкладки в дереве (спец. переноса U1 §3.3): строка, чья вкладка активна, подсвечена; ветка, внутри которой
 * лежит такая строка, подсвечивается сама, пока свёрнута. Принадлежность ветке считается по полям вкладки (подключение,
 * база, схема, вид), без обхода дерева.
 */

/** Активная вкладка рабочей области; вкладок нет — `null`. */
export function useActiveTab(): Tab | null {
  return useWorkspace((s) => s.tabs.find((x) => x.id === s.activeId) ?? null);
}

/** Вид списка объектов, в котором лежит объект вкладки; не вкладка объекта — `null`. */
export function objectKindOfTab(tab: Tab): ObjectKind | null {
  switch (tab.kind) {
    case "table":
      return "tables";
    case "view":
      return tab.matview ? "matviews" : "views";
    case "sequence":
      return "sequences";
    case "function":
      return "functions";
    default:
      return null;
  }
}

/** Вкладка относится к подключению (форма подключения — нет: у неё своей строки в дереве нет). */
export function inConnection(tab: Tab | null, connectionId: string): boolean {
  return tab !== null && tab.kind !== "connection-form" && "connectionId" in tab && tab.connectionId === connectionId;
}

/**
 * Вкладка объекта каталога внутри ветки: подключение, и, если заданы, база, схема и вид списка. У вкладки функции нет схемы
 * (функция открывается по oid) — в ветку схемы и вида она не попадает.
 */
export function inCatalog(tab: Tab | null, connectionId: string, where: { db?: string; schema?: string; kind?: ObjectKind } = {}): boolean {
  if (tab === null || !inConnection(tab, connectionId)) return false;
  const kind = objectKindOfTab(tab);
  if (kind === null || !("database" in tab)) return false;
  if (where.db !== undefined && tab.database !== where.db) return false;
  if (where.schema !== undefined && !("schema" in tab && tab.schema === where.schema)) return false;
  if (where.kind !== undefined && kind !== where.kind) return false;
  return true;
}

/** Строка «Диагностики», чья вкладка активна: инструмент, «Запросы» для карточки запроса, консоль EXPLAIN, журнал действий. */
export type DiagLeaf = { kind: "diag"; tool: string } | { kind: "explain" } | { kind: "actions" };

export function diagLeafIsActive(tab: Tab | null, connectionId: string, leaf: DiagLeaf): boolean {
  if (tab === null || !inConnection(tab, connectionId)) return false;
  if (leaf.kind === "diag") return (tab.kind === "diag" && tab.tool === leaf.tool) || (tab.kind === "query" && leaf.tool === "queries");
  return tab.kind === leaf.kind;
}

/** Вкладка лежит в группе «Диагностика» подключения. */
export function inDiagnostics(tab: Tab | null, connectionId: string): boolean {
  return inConnection(tab, connectionId) && tab !== null && (tab.kind === "diag" || tab.kind === "query" || tab.kind === "explain" || tab.kind === "actions");
}
