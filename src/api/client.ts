import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  AppError,
  AppInfo,
  ConnectionFolder,
  ConnectionProfile,
  DatabaseInfo,
  ExportSummary,
  ExtensionInfo,
  ImportPreview,
  LanguageSettings,
  LicenseStatus,
  MemberFacts,
  ObjectKind,
  ObjectPage,
  ParsedConnection,
  PolicyStatus,
  RoleInfo,
  SchemaInfo,
  SecretsInput,
  TestEvent,
  TestReport,
  TopologyEvent,
  TopologyView,
  TreeItemRef,
  UiSettings,
} from "./types";

export function isAppError(e: unknown): e is AppError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}

export function toAppError(e: unknown): AppError {
  // Ошибка из Rust приходит целиком: key/params/details сохраняются как есть.
  if (isAppError(e)) return e;
  if (e instanceof Error) return { code: "INTERNAL", message: e.message, retryable: false };
  return { code: "INTERNAL", message: String(e), retryable: false };
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toAppError(e);
  }
}

export const api = {
  getAppInfo: () => call<AppInfo>("get_app_info"),
  getLanguageSettings: () => call<LanguageSettings>("get_language_settings"),
  setLanguage: (lang: string | null) => call<LanguageSettings>("set_language", { lang }),
  getLicenseStatus: () => call<LicenseStatus>("get_license_status"),
  setLicenseKey: (key: string) => call<LicenseStatus>("set_license_key", { key }),
  removeLicenseKey: () => call<LicenseStatus>("remove_license_key"),
  getPolicyStatus: () => call<PolicyStatus>("get_policy_status"),
  exportDiagnostics: (path: string) => call<void>("export_diagnostics", { path }),

  listConnections: () => call<ConnectionProfile[]>("list_connections"),
  saveConnection: (profile: ConnectionProfile, secrets: SecretsInput) =>
    call<ConnectionProfile>("save_connection", { input: { profile, secrets } }),
  deleteConnection: (id: string) => call<void>("delete_connection", { id }),
  parseConnectionString: (text: string) => call<ParsedConnection>("parse_connection_string", { text }),
  /** Шаги проверки приходят в `onEvent` по мере выполнения; итог — отчёт. */
  testConnection: (profile: ConnectionProfile, secrets: SecretsInput, onEvent: (e: TestEvent) => void) => {
    const channel = new Channel<TestEvent>();
    channel.onmessage = onEvent;
    return call<TestReport>("test_connection", { input: { profile, secrets }, channel });
  },

  listFolders: () => call<ConnectionFolder[]>("list_folders"),
  createFolder: (name: string, parentId: string | null) => call<ConnectionFolder>("create_folder", { name, parentId }),
  renameFolder: (id: string, name: string) => call<ConnectionFolder>("rename_folder", { id, name }),
  deleteFolder: (id: string) => call<void>("delete_folder", { id }),
  moveItems: (items: TreeItemRef[], parentId: string | null, before: TreeItemRef | null) =>
    call<void>("move_items", { items, parentId, before }),
  // Обмен подключениями без секретов: файл `glafira-postgres-connections`.
  exportConnections: (path: string, folderId: string | null) =>
    call<ExportSummary>("export_connections", { path, folderId }),
  previewImport: (path: string, targetFolderId: string | null, updateExisting: boolean) =>
    call<ImportPreview>("preview_import", { path, targetFolderId, updateExisting }),
  applyImport: (path: string, targetFolderId: string | null, updateExisting: boolean) =>
    call<ImportPreview>("apply_import", { path, targetFolderId, updateExisting }),

  connect: (id: string) => call<TopologyView>("connect", { id }),
  disconnect: (id: string) => call<void>("disconnect", { id }),
  getTopology: (id: string) => call<TopologyView>("get_topology", { id }),
  listTopologyEvents: (id: string) => call<TopologyEvent[]>("list_topology_events", { id }),
  setCatalogMember: (id: string, member: string | null) => call<TopologyView>("set_catalog_member", { id, member }),
  getMemberFacts: (id: string, member: string) => call<MemberFacts>("get_member_facts", { id, member }),

  listDatabases: (id: string) => call<DatabaseInfo[]>("list_databases", { id }),
  listSchemas: (id: string, db: string, includeSystem: boolean) =>
    call<SchemaInfo[]>("list_schemas", { id, db, includeSystem }),
  listObjects: (id: string, db: string, schema: string, kind: ObjectKind, filter: string | null, offset: number, limit: number) =>
    call<ObjectPage>("list_objects", { id, db, schema, kind, filter, offset, limit }),
  listPartitions: (id: string, db: string, schema: string, table: string, offset: number, limit: number) =>
    call<ObjectPage>("list_partitions", { id, db, schema, table, offset, limit }),
  listExtensions: (id: string, db: string) => call<ExtensionInfo[]>("list_extensions", { id, db }),
  listRoles: (id: string, includeSystem: boolean) => call<RoleInfo[]>("list_roles", { id, includeSystem }),

  getUiSettings: () => call<UiSettings>("get_ui_settings"),
  setUiSettings: (settings: UiSettings) => call<UiSettings>("set_ui_settings", { settings }),
};
