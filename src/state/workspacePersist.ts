import { useEffect, useState } from "react";
import type { DiagTool } from "../api/types";
import { type SettingsSection, type Tab, type TabSpec, tabId, useWorkspace } from "./workspace";

/**
 * Открытые вкладки и активная переживают перезапуск (смоук P4c, R18): рабочая область пишется в localStorage при каждом
 * изменении, а при запуске восстанавливается, когда прочитан список подключений, — только вкладки, чьё подключение ещё
 * есть. Сохраняется спека без одноразовых полей: подстановки консоли EXPLAIN (её текст, база и узел — в черновике
 * консоли, `explainDraft.ts`) и формы подключения (несохранённый ввод всё равно теряется). Испорченное значение или
 * другая версия схемы — пустая рабочая область, без ошибки; испорченная запись одной вкладки пропускается.
 */
const KEY = "glafira.workspace";
const VERSION = 1;

/** Все инструменты диагностики: вкладка с неизвестным (переименованным) инструментом не восстанавливается. */
const DIAG_TOOLS: Record<DiagTool, true> = {
  pulse: true,
  sessions: true,
  idleTx: true,
  waits: true,
  locks: true,
  horizon: true,
  replication: true,
  slots: true,
  failover: true,
  health: true,
  queries: true,
};
const SETTINGS_SECTIONS: Record<SettingsSection, true> = { language: true, display: true, mcp: true, agent: true, sessions: true, audit: true };

/** Объект без полей со значением undefined: сохранённое и восстановленное не тащат пустых ключей. */
function clean(spec: TabSpec): TabSpec {
  return Object.fromEntries(Object.entries(spec).filter(([, v]) => v !== undefined)) as TabSpec;
}

/** Спека вкладки для сохранения — только известные поля вида вкладки; `null` — вкладка не сохраняется. */
function savedSpec(t: TabSpec): TabSpec | null {
  switch (t.kind) {
    case "connection-form":
      return null;
    case "about":
      return { kind: "about" };
    case "help":
      return clean({ kind: "help", chapter: t.chapter, anchor: t.anchor });
    case "settings":
      return clean({ kind: "settings", section: t.section });
    case "overview":
      return clean({ kind: "overview", connectionId: t.connectionId, member: t.member });
    case "table":
    case "sequence":
      return { kind: t.kind, connectionId: t.connectionId, database: t.database, schema: t.schema, name: t.name };
    case "view":
      return { kind: "view", connectionId: t.connectionId, database: t.database, schema: t.schema, name: t.name, matview: t.matview };
    case "function":
      return { kind: "function", connectionId: t.connectionId, database: t.database, oid: t.oid, name: t.name };
    case "diag":
      return clean({ kind: "diag", connectionId: t.connectionId, tool: t.tool, member: t.member });
    case "query":
      return clean({ kind: "query", connectionId: t.connectionId, member: t.member, database: t.database, queryId: t.queryId, title: t.title });
    case "explain":
      return { kind: "explain", connectionId: t.connectionId };
    case "actions":
      return { kind: "actions", connectionId: t.connectionId };
  }
}

const isStr = (v: unknown): v is string => typeof v === "string";
const isOptStr = (v: unknown) => v === undefined || isStr(v);
/** Ключ словаря, а не унаследованное свойство (`toString`); `Object.hasOwn` нет в lib ES2020. */
const known = (dict: object, k: string) => Object.prototype.hasOwnProperty.call(dict, k);

/** Запись из хранилища — спека вкладки, если поля её вида на месте и нужного типа; иначе `null`. */
function parseSpec(x: unknown): TabSpec | null {
  if (typeof x !== "object" || x === null || Array.isArray(x)) return null;
  const o = x as Record<string, unknown>;
  const conn = isStr(o.connectionId);
  const ok = (() => {
    switch (o.kind) {
      case "about":
        return true;
      case "help":
        return isOptStr(o.chapter) && isOptStr(o.anchor);
      case "settings":
        return o.section === undefined || (isStr(o.section) && known(SETTINGS_SECTIONS, o.section));
      case "overview":
        return conn && isOptStr(o.member);
      case "table":
      case "sequence":
        return conn && isStr(o.database) && isStr(o.schema) && isStr(o.name);
      case "view":
        return conn && isStr(o.database) && isStr(o.schema) && isStr(o.name) && typeof o.matview === "boolean";
      case "function":
        return conn && isStr(o.database) && typeof o.oid === "number" && Number.isFinite(o.oid) && isStr(o.name);
      case "diag":
        return conn && isStr(o.tool) && known(DIAG_TOOLS, o.tool) && (o.member === undefined || o.member === null || isStr(o.member));
      case "query":
        return conn && isStr(o.member) && isStr(o.database) && isStr(o.queryId) && isOptStr(o.title);
      case "explain":
      case "actions":
        return conn;
      default:
        // Форма подключения не сохраняется; неизвестный вид — запись другой версии.
        return false;
    }
  })();
  return ok ? savedSpec(o as unknown as TabSpec) : null;
}

/** Сохранённая рабочая область; не читается — пустая. */
function readSaved(): { tabs: TabSpec[]; active: string | null } {
  const empty = { tabs: [], active: null };
  try {
    const raw = localStorage.getItem(KEY);
    if (raw === null) return empty;
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return empty;
    const o = parsed as Record<string, unknown>;
    if (o.v !== VERSION || !Array.isArray(o.tabs)) return empty;
    return { tabs: o.tabs.map(parseSpec).filter((t): t is TabSpec => t !== null), active: isStr(o.active) ? o.active : null };
  } catch {
    return empty;
  }
}

function writeSaved(tabs: Tab[], activeId: string | null) {
  const saved: { spec: TabSpec; id: string }[] = [];
  for (const t of tabs) {
    const spec = savedSpec(t);
    if (spec) saved.push({ spec, id: t.id });
  }
  const active = saved.some((s) => s.id === activeId) ? activeId : null;
  try {
    localStorage.setItem(KEY, JSON.stringify({ v: VERSION, tabs: saved.map((s) => s.spec), active }));
  } catch {
    // Хранилище недоступно — вкладки живут до перезапуска.
  }
}

/**
 * Восстановить сохранённые вкладки, чьё подключение есть в `connectionIds`. Вкладки, открытые до восстановления, остаются
 * (после восстановленных) и активная — их; иначе активна сохранённая, если она восстановлена, или первая восстановленная.
 */
export function restoreWorkspace(connectionIds: ReadonlySet<string>): void {
  const saved = readSaved();
  useWorkspace.setState((s) => {
    const seen = new Set(s.tabs.map((t) => t.id));
    const restored: Tab[] = [];
    for (const spec of saved.tabs) {
      const conn = "connectionId" in spec ? spec.connectionId : null;
      if (conn !== null && !connectionIds.has(conn)) continue;
      const id = tabId(spec);
      if (seen.has(id)) continue;
      seen.add(id);
      restored.push({ ...spec, id } as Tab);
    }
    if (restored.length === 0) return s;
    const tabs = [...restored, ...s.tabs];
    const activeId = s.activeId ?? (restored.some((t) => t.id === saved.active) ? saved.active : restored[0].id);
    return { tabs, activeId };
  });
}

/** Писать рабочую область при каждом изменении вкладок или активной; возвращает отписку. */
export function persistWorkspace(): () => void {
  return useWorkspace.subscribe((s, prev) => {
    if (s.tabs !== prev.tabs || s.activeId !== prev.activeId) writeSaved(s.tabs, s.activeId);
  });
}

/**
 * Восстановление и запись рабочей области в `Layout`. `connections` — список подключений; `undefined` — ещё читается:
 * до первого ответа ничего не восстанавливается и не пишется (открытая за это время вкладка сохранённое не затирает).
 * Восстановление одно на запуск; сразу после него сохранённое приводится к рабочей области.
 */
export function useWorkspacePersistence(connections: readonly { id: string }[] | undefined): void {
  const [restored, setRestored] = useState(false);
  useEffect(() => {
    if (restored || connections === undefined) return;
    restoreWorkspace(new Set(connections.map((c) => c.id)));
    setRestored(true);
  }, [restored, connections]);
  useEffect(() => {
    if (!restored) return;
    const s = useWorkspace.getState();
    writeSaved(s.tabs, s.activeId);
    return persistWorkspace();
  }, [restored]);
}
