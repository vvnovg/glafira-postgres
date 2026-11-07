import { describe, expect, it } from "vitest";
import type { Tab } from "../../../state/workspace";
import { diagLeafIsActive, inCatalog, inConnection, inDiagnostics, objectKindOfTab } from "./treeActive";

const table: Tab = { id: "table:c:orders:public.customers", kind: "table", connectionId: "c", database: "orders", schema: "public", name: "customers" };
const matview: Tab = { id: "view:c:orders:public.mv", kind: "view", connectionId: "c", database: "orders", schema: "public", name: "mv", matview: true };
const fn: Tab = { id: "function:c:orders:42", kind: "function", connectionId: "c", database: "orders", oid: 42, name: "f" };
const query: Tab = { id: "query:c:pg-1:orders:7", kind: "query", connectionId: "c", member: "pg-1", database: "orders", queryId: "7" };
const form: Tab = { id: "connection-form:c", kind: "connection-form", connectionId: "c" };

describe("узел активной вкладки (спец. переноса U1 §3.3)", () => {
  it("вид списка по вкладке объекта", () => {
    expect(objectKindOfTab(table)).toBe("tables");
    expect(objectKindOfTab(matview)).toBe("matviews");
    expect(objectKindOfTab({ ...matview, matview: false } as Tab)).toBe("views");
    expect(objectKindOfTab(fn)).toBe("functions");
    expect(objectKindOfTab(query)).toBeNull();
  });

  it("ветки каталога: подключение, база, схема, вид; у функции схемы нет", () => {
    expect(inCatalog(table, "c")).toBe(true);
    expect(inCatalog(table, "other")).toBe(false);
    expect(inCatalog(table, "c", { db: "orders", schema: "public", kind: "tables" })).toBe(true);
    expect(inCatalog(table, "c", { db: "orders", schema: "public", kind: "views" })).toBe(false);
    expect(inCatalog(table, "c", { db: "analytics" })).toBe(false);
    expect(inCatalog(matview, "c", { db: "orders", schema: "public", kind: "matviews" })).toBe(true);
    expect(inCatalog(fn, "c", { db: "orders" })).toBe(true);
    expect(inCatalog(fn, "c", { db: "orders", schema: "public" })).toBe(false);
    expect(inCatalog(query, "c")).toBe(false);
    expect(inCatalog(null, "c")).toBe(false);
  });

  it("«Диагностика»: инструмент, карточка запроса — «Запросы», консоль и журнал", () => {
    const locks: Tab = { id: "diag:c:locks", kind: "diag", connectionId: "c", tool: "locks" };
    expect(diagLeafIsActive(locks, "c", { kind: "diag", tool: "locks" })).toBe(true);
    expect(diagLeafIsActive(locks, "c", { kind: "diag", tool: "pulse" })).toBe(false);
    expect(diagLeafIsActive(query, "c", { kind: "diag", tool: "queries" })).toBe(true);
    expect(diagLeafIsActive({ id: "explain:c", kind: "explain", connectionId: "c" }, "c", { kind: "explain" })).toBe(true);
    expect(diagLeafIsActive({ id: "actions:c", kind: "actions", connectionId: "c" }, "c", { kind: "explain" })).toBe(false);
    expect(inDiagnostics(query, "c")).toBe(true);
    expect(inDiagnostics(table, "c")).toBe(false);
  });

  it("форма подключения своей строки в дереве не имеет", () => {
    expect(inConnection(form, "c")).toBe(false);
    expect(inConnection(table, "c")).toBe(true);
  });
});
