import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionProfile } from "./types";

const sent: unknown[] = [];
vi.mock("@tauri-apps/api/core", () => {
  class Channel<T> {
    onmessage: (m: T) => void = () => {};
  }
  return { Channel, invoke: vi.fn() };
});

import { invoke } from "@tauri-apps/api/core";
import { api } from "./client";

const profile: ConnectionProfile = {
  id: "",
  name: "x",
  color: "#000000",
  environment: "local",
  readOnly: false,
  kind: "server",
  hosts: [{ host: "localhost", port: 5432 }],
  targetServerType: "primary",
  patroni: null,
  pg: { username: "u", database: "postgres", sslMode: "prefer", tls: { caPath: null, clientCert: null }, connectTimeoutS: 10, databases: null },
  ssh: null,
};

describe("testConnection", () => {
  // Тело в фигурных скобках: стрелка, возвращающая мок, сделала бы его хуком очистки, который vitest вызовет после теста.
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("передаёт события канала и возвращает отчёт", async () => {
    vi.mocked(invoke).mockImplementation(async (_cmd, args) => {
      const ch = (args as { channel: { onmessage: (e: unknown) => void } }).channel;
      ch.onmessage({ step: "patroni", member: null, status: "skipped", message: null, error: null });
      return { ok: true, topology: null, readiness: [] };
    });
    const report = await api.testConnection(profile, { pgPassword: "x" }, (e) => sent.push(e));
    expect(report.ok).toBe(true);
    expect(sent).toHaveLength(1);
    expect(vi.mocked(invoke).mock.calls[0][0]).toBe("test_connection");
    expect((vi.mocked(invoke).mock.calls[0][1] as { input: { secrets: unknown } }).input.secrets).toEqual({ pgPassword: "x" });
  });
});

// Имена команд и аргументов должны совпадать с Rust: Tauri переводит camelCase аргументов в snake_case параметров.
describe("команды и аргументы", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue(undefined);
  });

  const cases: [string, () => Promise<unknown>, string, Record<string, unknown> | undefined][] = [
    ["listConnections", () => api.listConnections(), "list_connections", undefined],
    ["saveConnection", () => api.saveConnection(profile, { pgPassword: "x" }), "save_connection", { input: { profile, secrets: { pgPassword: "x" } } }],
    ["deleteConnection", () => api.deleteConnection("c"), "delete_connection", { id: "c" }],
    ["connectionSecrets", () => api.connectionSecrets("c"), "connection_secrets", { id: "c" }],
    ["parseConnectionString", () => api.parseConnectionString("postgres://h"), "parse_connection_string", { text: "postgres://h" }],
    ["connect", () => api.connect("c"), "connect", { id: "c" }],
    ["disconnect", () => api.disconnect("c"), "disconnect", { id: "c" }],
    ["reconnect", () => api.reconnect("c"), "reconnect", { id: "c" }],
    ["getTopology", () => api.getTopology("c"), "get_topology", { id: "c" }],
    ["listTopologyEvents", () => api.listTopologyEvents("c"), "list_topology_events", { id: "c" }],
    ["setCatalogMember", () => api.setCatalogMember("c", null), "set_catalog_member", { id: "c", member: null }],
    ["getMemberFacts", () => api.getMemberFacts("c", "pg-1"), "get_member_facts", { id: "c", member: "pg-1" }],
    ["listDatabases", () => api.listDatabases("c"), "list_databases", { id: "c" }],
    ["listSchemas", () => api.listSchemas("c", "app", true), "list_schemas", { id: "c", db: "app", includeSystem: true }],
    [
      "listObjects",
      () => api.listObjects("c", "app", "public", "tables", null, 0, 200),
      "list_objects",
      { id: "c", db: "app", schema: "public", kind: "tables", filter: null, offset: 0, limit: 200 },
    ],
    [
      "listPartitions",
      () => api.listPartitions("c", "app", "public", "events", 100, 50),
      "list_partitions",
      { id: "c", db: "app", schema: "public", table: "events", offset: 100, limit: 50 },
    ],
    ["listExtensions", () => api.listExtensions("c", "app"), "list_extensions", { id: "c", db: "app" }],
    ["listRoles", () => api.listRoles("c", false), "list_roles", { id: "c", includeSystem: false }],
    ["getUiSettings", () => api.getUiSettings(), "get_ui_settings", undefined],
    ["setUiSettings", () => api.setUiSettings({ showSystemObjects: true }), "set_ui_settings", { settings: { showSystemObjects: true } }],
    ["exportDiagnostics", () => api.exportDiagnostics("/tmp/d.zip"), "export_diagnostics", { path: "/tmp/d.zip" }],
    ["listFolders", () => api.listFolders(), "list_folders", undefined],
    ["createFolder", () => api.createFolder("f", null), "create_folder", { name: "f", parentId: null }],
    ["renameFolder", () => api.renameFolder("f1", "g"), "rename_folder", { id: "f1", name: "g" }],
    ["deleteFolder", () => api.deleteFolder("f1"), "delete_folder", { id: "f1" }],
    [
      "moveItems",
      () => api.moveItems([{ kind: "connection", id: "c" }], "f1", { kind: "folder", id: "f2" }),
      "move_items",
      { items: [{ kind: "connection", id: "c" }], parentId: "f1", before: { kind: "folder", id: "f2" } },
    ],
  ];

  it.each(cases)("%s", async (_name, run, cmd, args) => {
    await run();
    expect(vi.mocked(invoke).mock.calls).toEqual([args === undefined ? [cmd, undefined] : [cmd, args]]);
  });
});
