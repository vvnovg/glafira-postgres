import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  ActionRecord,
  ActionResult,
  AgentDetected,
  AgentModels,
  AgentPreset,
  AgentRun,
  AgentSettings,
  AppError,
  AppInfo,
  Confirmation,
  ConnectionFolder,
  Comparison,
  ConnectionProfile,
  CreateRequestArgs,
  DatabaseInfo,
  DiagReport,
  DiagRequest,
  ExplainRequest,
  ExplainResult,
  HypoRequest,
  HypoResult,
  ExportSummary,
  ExtensionInfo,
  FunctionDetails,
  ImportPreview,
  LanguageSettings,
  LlmBundle,
  LlmProposal,
  LlmSource,
  McpAuditEntry,
  McpClientConfigs,
  McpStatus,
  MemberFacts,
  ObjectKind,
  ObjectPage,
  OptimizationRequest,
  ParsedConnection,
  PatroniAction,
  PatroniActionPreview,
  PatroniActionResult,
  ParsedPlan,
  PlanTree,
  PolicyStatus,
  ProposalFilter,
  QueryDetail,
  QueryKey,
  QueryText,
  RoleInfo,
  RunFilter,
  SamplerEvent,
  SchemaInfo,
  SecretsInput,
  SecretsSet,
  SequenceDetails,
  SessionActionExecute,
  SessionActionPreview,
  SessionActionRequest,
  SessionActionResult,
  SnapshotInfo,
  SlotDropPreview,
  SlotDropRequest,
  StatementsEvent,
  TableDetails,
  TestEvent,
  TestReport,
  TopologyEvent,
  TopologyView,
  TreeItemRef,
  UiSettings,
  UpdateCheck,
  ViewDetails,
  WatchEvent,
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
  getPolicyStatus: () => call<PolicyStatus>("get_policy_status"),
  exportDiagnostics: (path: string) => call<void>("export_diagnostics", { path }),
  checkUpdate: () => call<UpdateCheck>("check_update"),
  installUpdate: () => call<void>("install_update"),
  getMcpStatus: () => call<McpStatus>("get_mcp_status"),
  /** Включить или выключить MCP-сервер; ответ — статус после запуска или остановки. */
  setMcpEnabled: (on: boolean) => call<McpStatus>("set_mcp_enabled", { on }),
  /**
   * Ответ на запрос подтверждения клиента; новый статус приходит событием `mcp://state`.
   */
  answerMcpApproval: (sessionId: string, approve: boolean) => call<void>("answer_mcp_approval", { sessionId, approve }),
  /** Забыть клиента и закрыть его сессии. */
  revokeMcpClient: (name: string) => call<void>("revoke_mcp_client", { name }),
  /** Закрыть одну сессию или все (без id). */
  disconnectMcpSessions: (sessionId?: string) => call<void>("disconnect_mcp_sessions", { sessionId: sessionId ?? null }),
  /** Последние записи журнала MCP, новые первыми. */
  getMcpAudit: (limit: number) => call<McpAuditEntry[]>("get_mcp_audit", { limit }),
  getMcpClientConfigs: () => call<McpClientConfigs>("get_mcp_client_configs"),
  /** Показать `mcp-audit.jsonl` в файловом менеджере. */
  revealMcpAudit: () => call<void>("reveal_mcp_audit"),

  listConnections: () => call<ConnectionProfile[]>("list_connections"),
  saveConnection: (profile: ConnectionProfile, secrets: SecretsInput) =>
    call<ConnectionProfile>("save_connection", { input: { profile, secrets } }),
  /** Какие секреты сохранены (без значений): «оставить сохранённый» — только там, где он есть. */
  connectionSecrets: (id: string) => call<SecretsSet>("connection_secrets", { id }),
  deleteConnection: (id: string) => call<void>("delete_connection", { id }),
  parseConnectionString: (text: string) => call<ParsedConnection>("parse_connection_string", { text }),
  /** Доверие ключу хоста ssh (после `errors.ssh.hostKeyUnknown`) — до выхода из приложения; затем повтор. */
  trustSshHostKey: (target: string, fingerprint: string) => call<void>("trust_ssh_host_key", { target, fingerprint }),
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
  /** Закрыть и открыть заново: запомненный отказ во входе сбрасывается, секреты читаются заново. */
  reconnect: (id: string) => call<TopologyView>("reconnect", { id }),
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
  getTable: (id: string, db: string, schema: string, name: string) => call<TableDetails>("get_table", { id, db, schema, name }),
  getView: (id: string, db: string, schema: string, name: string) => call<ViewDetails>("get_view", { id, db, schema, name }),
  getFunction: (id: string, db: string, oid: number) => call<FunctionDetails>("get_function", { id, db, oid }),
  getSequence: (id: string, db: string, schema: string, name: string) => call<SequenceDetails>("get_sequence", { id, db, schema, name }),
  listExtensions: (id: string, db: string) => call<ExtensionInfo[]>("list_extensions", { id, db }),
  listRoles: (id: string, includeSystem: boolean) => call<RoleInfo[]>("list_roles", { id, includeSystem }),

  runDiagnostic: (id: string, request: DiagRequest) => call<DiagReport>("run_diagnostic", { id, request }),
  /** Подписка на сэмплер узла: события — в `onEvent`, итог — id сессии для `stopSession`. */
  startSampler: (id: string, member: string | null, onEvent: (e: SamplerEvent) => void) => {
    const channel = new Channel<SamplerEvent>();
    channel.onmessage = onEvent;
    return call<string>("start_sampler", { id, member, channel });
  },
  stopSession: (sessionId: string) => call<boolean>("stop_session", { sessionId }),

  queryTexts: (id: string, member: string | null, keys: QueryKey[]) => call<QueryText[]>("query_texts", { id, member, keys }),
  /** Подписка на сэмплер pg_stat_statements узла; итог — id сессии для `stopSession`. */
  startStatementsSampler: (id: string, member: string | null, onEvent: (e: StatementsEvent) => void) => {
    const channel = new Channel<StatementsEvent>();
    channel.onmessage = onEvent;
    return call<string>("start_statements_sampler", { id, member, channel });
  },
  queryDetail: (id: string, member: string, database: string, queryId: string) => call<QueryDetail>("query_detail", { id, member, database, queryId }),
  explain: (id: string, request: ExplainRequest) => call<ExplainResult>("explain", { id, request }),
  /** Проверка гипотезы hypopg: виртуальный индекс в READ ONLY-транзакции, без следа на сервере. */
  hypoCheck: (id: string, request: HypoRequest) => call<HypoResult>("hypo_check", { id, request }),
  parsePlan: (text: string) => call<ParsedPlan>("parse_plan", { text }),
  queryLlmBundle: (id: string, member: string, database: string, source: LlmSource, plan: PlanTree | null) =>
    call<LlmBundle>("query_llm_bundle", { id, member, database, source, plan }),
  /** Запрос на оптимизацию (спец. P4 §8.1): снимок для LLM-ассистента; модель забирает его через MCP. */
  createOptimizationRequest: (id: string, req: CreateRequestArgs) => call<OptimizationRequest>("create_optimization_request", { id, req }),
  /** Отменяет только ждущий запрос; закрытый — ошибка. */
  cancelOptimizationRequest: (requestId: string) => call<OptimizationRequest>("cancel_optimization_request", { requestId }),
  /** Запросы подключения, новые первыми (последние 50 в приложении). */
  listOptimizationRequests: (connectionId: string) => call<OptimizationRequest[]>("list_optimization_requests", { connectionId }),
  /** Предложения по фильтру (подключение сужает всегда, остальные поля — ИЛИ), новые первыми. */
  listLlmProposals: (filter: ProposalFilter) => call<LlmProposal[]>("list_llm_proposals", { filter }),
  /** Ответ пользователя на предложение; причина отклонения — до 500 символов, её видит модель. */
  answerLlmProposal: (id: string, status: "accepted" | "rejected", reason: string | null) =>
    call<LlmProposal>("answer_llm_proposal", { id, status, reason }),
  /** «Сравнить планы»: итог записывается в предложение и возвращается в нём. */
  compareLlmProposal: (id: string) => call<LlmProposal>("compare_llm_proposal", { id }),
  /** Фоновый ассистент (спец. P4c §7): запуск на запрос в очереди Glafira; ответ — запуск `queued`, дальше его ход приходит событиями запусков (`AGENT_RUNS_EVENT`). */
  startAgentRun: (requestId: string) => call<AgentRun>("start_agent_run", { requestId }),
  /** «Остановить» в списке запусков: запрос остаётся ждущим. */
  stopAgentRun: (runId: string) => call<AgentRun>("stop_agent_run", { runId }),
  /** Запуски по фильтру (последние 50); пустой фильтр — все. */
  listAgentRuns: (filter: RunFilter = {}) => call<AgentRun[]>("list_agent_runs", { filter }),
  /** Хвост журнала запуска (до 256 КБ). */
  readAgentLog: (runId: string) => call<string>("read_agent_log", { runId }),
  /** Показать файл журнала запуска в файловом менеджере. */
  revealAgentLog: (runId: string) => call<void>("reveal_agent_log", { runId }),
  /** Настройки подраздела «Фоновый ассистент» (`settings.json` → `mcp.agent`). */
  getAgentSettings: () => call<AgentSettings>("get_agent_settings"),
  /** Сохраняет настройки целиком; ответ — сохранённые (проверенные Rust) настройки. Политика запрещает только включение. */
  setAgentSettings: (settings: AgentSettings) => call<AgentSettings>("set_agent_settings", { settings }),
  /**
   * «Найти»: путь и версия программы пресета (логин-оболочка, PATH, известные места). С `path` — только версия этой программы
   * (путь, указанный руками).
   */
  detectAgentCli: (preset: AgentPreset, path?: string) => call<AgentDetected>("detect_agent_cli", path === undefined ? { preset } : { preset, path }),
  /** Модели для поля «Модель»; список зависит от сохранённых «Через Ollama» и адреса Ollama. */
  listAgentModels: (preset: AgentPreset) => call<AgentModels>("list_agent_models", { preset }),
  /** «Проверить»: короткий запуск без подключения; ответ — запуск с kind check, итог приходит событием запусков. */
  checkAgent: () => call<AgentRun>("check_agent"),
  /** «Выгрузить модель»: keep_alive 0 у модели фонового ассистента в локальной Ollama. */
  unloadAgentModel: () => call<void>("unload_agent_model"),

  /** Снимок pgss узла на диск пользователя; второй снимок узла в ту же секунду — `errors.snapshots.exists`. */
  snapshotTake: (id: string, member: string) => call<SnapshotInfo>("snapshot_take", { id, member }),
  /** Снимки подключения, новые первыми; подключение может быть закрыто. */
  snapshotList: (id: string) => call<SnapshotInfo[]>("snapshot_list", { id }),
  /** `false` — файла уже не было. */
  snapshotDelete: (id: string, file: string) => call<boolean>("snapshot_delete", { id, file }),
  snapshotCompare: (id: string, from: string, to: string) => call<Comparison>("snapshot_compare", { id, from, to }),

  previewSessionAction: (id: string, request: SessionActionRequest) =>
    call<SessionActionPreview>("preview_session_action", { id, request }),
  executeSessionAction: (id: string, request: SessionActionExecute, confirmation: Confirmation) =>
    call<SessionActionResult>("execute_session_action", { id, request, confirmation }),
  previewDropSlot: (id: string, request: SlotDropRequest) => call<SlotDropPreview>("preview_drop_slot", { id, request }),
  dropSlot: (id: string, request: SlotDropRequest, confirmation: Confirmation) =>
    call<ActionResult>("drop_slot", { id, request, confirmation }),
  previewPatroniAction: (id: string, action: PatroniAction) =>
    call<PatroniActionPreview>("preview_patroni_action", { id, action }),
  /** Действие Patroni; события наблюдения (если оно открылось) — в `onEvent`, итог — `watch` для `stopSession`. */
  executePatroniAction: (id: string, action: PatroniAction, confirmation: Confirmation, onEvent: (e: WatchEvent) => void) => {
    const channel = new Channel<WatchEvent>();
    channel.onmessage = onEvent;
    return call<PatroniActionResult>("execute_patroni_action", { id, action, confirmation, channel });
  },
  /** Журнал действий (новые первыми): одного подключения или всех (`null`); `limit` не больше 1000. */
  listActions: (connectionId: string | null, limit: number) => call<ActionRecord[]>("list_actions", { connectionId, limit }),

  getUiSettings: () => call<UiSettings>("get_ui_settings"),
  setUiSettings: (settings: UiSettings) => call<UiSettings>("set_ui_settings", { settings }),
};
