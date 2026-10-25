import { create } from "zustand";

import type { DiagTool } from "../api/types";

export type TabSpec =
  /** folderId — папка нового подключения («Новое подключение здесь»); copyOf — id подключения, копия которого создаётся. */
  | { kind: "connection-form"; connectionId: string | null; folderId?: string | null; copyOf?: string }
  /** Обзор подключения; member — узел, выбранный в дереве (одна вкладка на подключение). */
  | { kind: "overview"; connectionId: string; member?: string }
  | { kind: "about" }
  /** Справка: глава и раздел руководства (F1, ссылки). */
  | { kind: "help"; chapter?: string; anchor?: string }
  | { kind: "settings" }
  /** Объекты базы из дерева каталога. Таблица, представление и последовательность — по схеме и имени, функция — по oid (имена перегружены). */
  | { kind: "table"; connectionId: string; database: string; schema: string; name: string }
  | { kind: "view"; connectionId: string; database: string; schema: string; name: string; matview: boolean }
  | { kind: "function"; connectionId: string; database: string; oid: number; name: string }
  | { kind: "sequence"; connectionId: string; database: string; schema: string; name: string }
  /** Инструмент диагностики; member — закреплённый узел, нет — за лидером (одна вкладка на подключение и инструмент). */
  | { kind: "diag"; connectionId: string; tool: DiagTool; member?: string | null }
  /** Журнал действий подключения (одна вкладка на подключение). */
  | { kind: "actions"; connectionId: string };

export type Tab = TabSpec & { id: string };

export function tabId(spec: TabSpec): string {
  switch (spec.kind) {
    case "connection-form":
      if (spec.copyOf) return `connection-form:copy:${spec.copyOf}`;
      return `connection-form:${spec.connectionId ?? "new"}`;
    case "overview":
      return `overview:${spec.connectionId}`;
    case "about":
      return "about";
    case "help":
      return "help";
    case "settings":
      return "settings";
    case "table":
    case "view":
    case "sequence":
      return `${spec.kind}:${spec.connectionId}:${spec.database}:${spec.schema}.${spec.name}`;
    case "function":
      return `function:${spec.connectionId}:${spec.database}:${spec.oid}`;
    case "diag":
      return `diag:${spec.connectionId}:${spec.tool}`;
    case "actions":
      return `actions:${spec.connectionId}`;
  }
}

interface WorkspaceState {
  tabs: Tab[];
  activeId: string | null;
  open: (spec: TabSpec) => void;
  close: (id: string) => void;
  /** Закрывает все вкладки подключения (например, после его удаления). */
  closeForConnection: (connectionId: string) => void;
  activate: (id: string) => void;
}

export const useWorkspace = create<WorkspaceState>((set) => ({
  tabs: [],
  activeId: null,
  open: (spec) =>
    set((s) => {
      const id = tabId(spec);
      const tab = { ...spec, id } as Tab;
      // Уже открытая вкладка получает новый spec (например, другую главу справки).
      if (s.tabs.some((t) => t.id === id)) return { tabs: s.tabs.map((t) => (t.id === id ? tab : t)), activeId: id };
      return { tabs: [...s.tabs, tab], activeId: id };
    }),
  close: (id) =>
    set((s) => {
      const idx = s.tabs.findIndex((t) => t.id === id);
      if (idx < 0) return s;
      const tabs = s.tabs.filter((t) => t.id !== id);
      const activeId = s.activeId !== id ? s.activeId : (tabs[idx] ?? tabs[idx - 1])?.id ?? null;
      return { tabs, activeId };
    }),
  closeForConnection: (connectionId) =>
    set((s) => {
      const firstIdx = s.tabs.findIndex((t) => ("connectionId" in t ? t.connectionId : null) === connectionId);
      if (firstIdx < 0) return s;
      const tabs = s.tabs.filter((t) => ("connectionId" in t ? t.connectionId : null) !== connectionId);
      const activeId = tabs.some((t) => t.id === s.activeId)
        ? s.activeId
        : (tabs[firstIdx] ?? tabs[tabs.length - 1])?.id ?? null;
      return { tabs, activeId };
    }),
  activate: (id) => set({ activeId: id }),
}));
