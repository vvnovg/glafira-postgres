import { create } from "zustand";

import type { DiagTool } from "../api/types";

/**
 * Подстановка в консоль EXPLAIN (спец. P4 §11): текст, база и узел вызова MCP из журнала (P4b — предложения модели).
 * Консоль применяет её один раз на `nonce` и ничего не запускает; узла нет — «за лидером».
 */
export interface ExplainPrefill {
  sql: string;
  /** Нет — база подключения по умолчанию. */
  database?: string;
  member?: string;
  nonce: number;
}

export type TabSpec =
  /** folderId — папка нового подключения («Новое подключение здесь»); copyOf — id подключения, копия которого создаётся. */
  | { kind: "connection-form"; connectionId: string | null; folderId?: string | null; copyOf?: string }
  /** Обзор подключения; member — узел, выбранный в дереве (одна вкладка на подключение). */
  | { kind: "overview"; connectionId: string; member?: string }
  | { kind: "about" }
  /** Справка: глава и раздел руководства (F1, ссылки). */
  | { kind: "help"; chapter?: string; anchor?: string }
  /** Настройки; section — раздел, к которому прокрутить (индикатор LLM открывает раздел MCP). Вкладка одна. */
  | { kind: "settings"; section?: "mcp" }
  /** Объекты базы из дерева каталога. Таблица, представление и последовательность — по схеме и имени, функция — по oid (имена перегружены). */
  | { kind: "table"; connectionId: string; database: string; schema: string; name: string }
  | { kind: "view"; connectionId: string; database: string; schema: string; name: string; matview: boolean }
  | { kind: "function"; connectionId: string; database: string; oid: number; name: string }
  | { kind: "sequence"; connectionId: string; database: string; schema: string; name: string }
  /** Инструмент диагностики; member — закреплённый узел, нет — за лидером (одна вкладка на подключение и инструмент). */
  | { kind: "diag"; connectionId: string; tool: DiagTool; member?: string | null }
  /** Карточка запроса pgss: узел, база и queryId; title — начало текста для заголовка вкладки (не часть id). */
  | { kind: "query"; connectionId: string; member: string; database: string; queryId: string; title?: string }
  /** Консоль EXPLAIN — одна на подключение; member/database/sql — подстановка при открытии из карточки. */
  | { kind: "explain"; connectionId: string; member?: string | null; database?: string; sql?: string; prefill?: ExplainPrefill }
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
    case "query":
      return `query:${spec.connectionId}:${spec.member}:${spec.database}:${spec.queryId}`;
    case "explain":
      return `explain:${spec.connectionId}`;
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
  /**
   * Заголовок вкладки запроса (начало текста): карточка ставит его, когда текст прочитан, а вкладка открыта без него
   * (из находки — только queryId). Тот же заголовок и вкладки других видов не трогаются.
   */
  retitle: (id: string, title: string) => void;
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
  retitle: (id, title) =>
    set((s) => {
      const tab = s.tabs.find((t) => t.id === id);
      if (!tab || tab.kind !== "query" || tab.title === title) return s;
      return { tabs: s.tabs.map((t) => (t.id === id ? { ...tab, title } : t)) };
    }),
}));

let prefillNonce = 0;

/**
 * Открыть консоль EXPLAIN подключения с подстановкой (журнал MCP; панель предложений P4b). Спека вкладки заменяется
 * целиком: member/database/sql прежнего открытия из карточки не остаются. Каждый вызов — новый nonce: консоль подставит
 * текст и тогда, когда он совпадает с прошлой подстановкой, а пользователь успел его поправить.
 */
export function openExplainWithPrefill(connectionId: string, prefill: Omit<ExplainPrefill, "nonce">): void {
  prefillNonce += 1;
  useWorkspace.getState().open({ kind: "explain", connectionId, prefill: { ...prefill, nonce: prefillNonce } });
}
