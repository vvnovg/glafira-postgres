import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ConnectionProfile,
  Confirmation,
  PatroniAction,
  SessionActionExecute,
  SessionActionRequest,
  SlotDropRequest,
  WatchEvent,
} from "./types";

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

const sessionReq: SessionActionRequest = { member: "pg-1", kind: "terminate", target: { kind: "idleInTransaction", olderThanS: 300 } };
const sessionExec: SessionActionExecute = { ...sessionReq, sessions: [{ pid: 42, backendStartUs: 1 }] };
const slotReq: SlotDropRequest = { member: "pg-1", slot: "it_dead_slot" };
const pause: PatroniAction = { kind: "pause", paused: true };
const confirmation: Confirmation = { confirm: "billing-prod", typedName: null };

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

describe("startSampler", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("события канала доходят до onEvent, команда — start_sampler, итог — id сессии", async () => {
    const got: unknown[] = [];
    vi.mocked(invoke).mockImplementation(async (_cmd, args) => {
      const ch = (args as { channel: { onmessage: (e: unknown) => void } }).channel;
      ch.onmessage({ kind: "tick", atMs: 5, active: 2, waiting: 1 });
      ch.onmessage({ kind: "stopped" });
      return "session-1";
    });
    const id = await api.startSampler("c", "pg-1", (e) => got.push(e));
    expect(id).toBe("session-1");
    expect(got).toEqual([{ kind: "tick", atMs: 5, active: 2, waiting: 1 }, { kind: "stopped" }]);
    const [cmd, args] = vi.mocked(invoke).mock.calls[0] as [string, { id: string; member: string | null; channel: unknown }];
    expect(cmd).toBe("start_sampler");
    expect(args).toMatchObject({ id: "c", member: "pg-1" });
    expect(args.channel).toBeDefined();
  });

  it("узел «за лидером» уходит как null; ошибка команды приходит как AppError", async () => {
    vi.mocked(invoke).mockRejectedValue({ code: "NOT_FOUND", message: "x", retryable: false });
    await expect(api.startSampler("c", null, () => {})).rejects.toMatchObject({ code: "NOT_FOUND" });
    expect(vi.mocked(invoke).mock.calls[0][1]).toMatchObject({ id: "c", member: null });
  });
});

describe("executePatroniAction", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("события наблюдения приходят в onEvent, итог содержит исход и причину", async () => {
    const got: WatchEvent[] = [];
    vi.mocked(invoke).mockImplementation(async (_cmd, args) => {
      const ch = (args as { channel: { onmessage: (e: WatchEvent) => void } }).channel;
      ch.onmessage({ step: "accepted", status: "running", atMs: 1, detail: null });
      return { response: "ok", watch: "w-1", journalWritten: true, outcomeUnknown: false, error: null };
    });
    const r = await api.executePatroniAction("c", pause, confirmation, (e) => got.push(e));
    expect(r.watch).toBe("w-1");
    expect(r.outcomeUnknown).toBe(false);
    expect(got.map((e) => e.step)).toEqual(["accepted"]);
    expect(vi.mocked(invoke).mock.calls[0][0]).toBe("execute_patroni_action");
    expect(vi.mocked(invoke).mock.calls[0][1]).toMatchObject({ id: "c", action: pause, confirmation });
  });

  it("ошибка команды приходит как AppError", async () => {
    vi.mocked(invoke).mockRejectedValue({ code: "CONFIRMATION_REQUIRED", message: "x", retryable: false });
    await expect(api.executePatroniAction("c", pause, confirmation, () => {})).rejects.toMatchObject({ code: "CONFIRMATION_REQUIRED" });
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
    ["trustSshHostKey", () => api.trustSshHostKey("bastion", "SHA256:x"), "trust_ssh_host_key", { target: "bastion", fingerprint: "SHA256:x" }],
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
    ["getTable", () => api.getTable("c", "app", "public", "orders"), "get_table", { id: "c", db: "app", schema: "public", name: "orders" }],
    ["getView", () => api.getView("c", "app", "public", "v_orders"), "get_view", { id: "c", db: "app", schema: "public", name: "v_orders" }],
    ["getFunction", () => api.getFunction("c", "app", 16401), "get_function", { id: "c", db: "app", oid: 16401 }],
    ["getSequence", () => api.getSequence("c", "app", "public", "orders_id_seq"), "get_sequence", { id: "c", db: "app", schema: "public", name: "orders_id_seq" }],
    ["listExtensions", () => api.listExtensions("c", "app"), "list_extensions", { id: "c", db: "app" }],
    ["listRoles", () => api.listRoles("c", false), "list_roles", { id: "c", includeSystem: false }],
    ["getUiSettings", () => api.getUiSettings(), "get_ui_settings", undefined],
    ["setUiSettings", () => api.setUiSettings({ showSystemObjects: true }), "set_ui_settings", { settings: { showSystemObjects: true } }],
    ["exportDiagnostics", () => api.exportDiagnostics("/tmp/d.zip"), "export_diagnostics", { path: "/tmp/d.zip" }],
    ["runDiagnostic", () => api.runDiagnostic("c", { tool: "pulse", member: null }), "run_diagnostic", { id: "c", request: { tool: "pulse", member: null } }],
    ["stopSession", () => api.stopSession("s"), "stop_session", { sessionId: "s" }],
    ["previewSessionAction", () => api.previewSessionAction("c", sessionReq), "preview_session_action", { id: "c", request: sessionReq }],
    ["executeSessionAction", () => api.executeSessionAction("c", sessionExec, confirmation), "execute_session_action", { id: "c", request: sessionExec, confirmation }],
    ["previewDropSlot", () => api.previewDropSlot("c", slotReq), "preview_drop_slot", { id: "c", request: slotReq }],
    ["dropSlot", () => api.dropSlot("c", slotReq, confirmation), "drop_slot", { id: "c", request: slotReq, confirmation }],
    ["previewPatroniAction", () => api.previewPatroniAction("c", pause), "preview_patroni_action", { id: "c", action: pause }],
    ["listActions", () => api.listActions("c", 200), "list_actions", { connectionId: "c", limit: 200 }],
    ["listActions (все подключения)", () => api.listActions(null, 50), "list_actions", { connectionId: null, limit: 50 }],
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
