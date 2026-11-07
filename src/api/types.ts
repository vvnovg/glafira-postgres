export type ErrorCode =
  | "CONNECTION_FAILED"
  | "TLS_ERROR"
  | "AUTH_FAILED"
  | "NOT_AUTHORIZED"
  | "TIMEOUT"
  | "INVALID_INPUT"
  | "NOT_FOUND"
  | "PATRONI_ERROR"
  | "NODE_UNAVAILABLE"
  | "SSH_ERROR"
  | "FEATURE_UNAVAILABLE"
  | "RECOVERY_CONFLICT"
  | "READ_ONLY"
  | "CONFIRMATION_REQUIRED"
  | "EXPLAIN_WRITES"
  | "INTERNAL";

/** Текст из Rust: ключ словаря, параметры и английский текст (запасной вариант). */
export interface LocalizedText {
  key: string;
  params: Record<string, string>;
  text: string;
}

export interface AppError {
  code: ErrorCode;
  /** Ключ словаря; у технического текста без перевода — `errors.generic` с `params.message`. */
  key?: string;
  params?: Record<string, string>;
  /** Английский текст (запасной вариант перевода). */
  message: string;
  details?: string;
  retryable: boolean;
}

export type Environment = "local" | "dev" | "test" | "stage" | "prod";
export type ConnectionKind = "server" | "hostList" | "patroni";
/** Уровень доступа LLM-ассистентов к подключению (спец. P4 §4.1); `data` — P4b. */
export type LlmAccess = "off" | "diagnostics" | "data";
/** Потолок уровня доступа LLM по политике администратора (`mcpMaxAccess`). */
export type McpMaxAccess = "diagnostics" | "data";
export type TargetServerType = "primary" | "standby" | "preferStandby" | "any";
export type SslMode = "disable" | "prefer" | "require" | "verify-ca" | "verify-full";
export type CertFormat = "pem" | "pkcs12";

export interface HostPort {
  host: string;
  port: number;
}
export interface ClientCert {
  format: CertFormat;
  certificatePath: string;
  keyPath: string | null;
}
export interface PgTls {
  caPath: string | null;
  clientCert: ClientCert | null;
}
export interface PgSettings {
  username: string;
  database: string;
  sslMode: SslMode;
  tls: PgTls;
  connectTimeoutS: number;
  /** null — все базы, к которым можно подключиться; список — только эти. */
  databases: string[] | null;
}
export interface ApiTls {
  caPath: string | null;
  verifyHostname: boolean;
  clientCert: ClientCert | null;
}
export interface HostOverride {
  member: string;
  host: string | null;
  port: number | null;
  apiUrl: string | null;
}
export interface PatroniSettings {
  urls: string[];
  username: string | null;
  tls: ApiTls | null;
  hostOverrides: HostOverride[];
}
export type SshAuth = "agent" | "key" | "password";
export interface SshSettings {
  /** Хост или алиас из ~/.ssh/config. */
  target: string;
  port: number | null;
  user: string | null;
  auth: SshAuth;
  keyPath: string | null;
}
export interface ConnectionProfile {
  id: string;
  name: string;
  color: string;
  environment: Environment;
  readOnly: boolean;
  kind: ConnectionKind;
  hosts: HostPort[];
  targetServerType: TargetServerType;
  patroni: PatroniSettings | null;
  pg: PgSettings;
  /** Туннель SSH до кластера; null — подключение напрямую. */
  ssh: SshSettings | null;
  folderId?: string | null;
  order?: number;
  /** Доступ LLM через MCP; нет поля — "off". Локальное: экспорт его не пишет, импорт не переносит. */
  llmAccess?: LlmAccess;
}
/** У каждого поля: null/нет — оставить сохранённый, "" — удалить, иначе — новый. */
export interface SecretsInput {
  pgPassword?: string | null;
  pgKeyPassword?: string | null;
  patroniPassword?: string | null;
  patroniKeyPassword?: string | null;
  sshPassword?: string | null;
  sshPassphrase?: string | null;
}
/** Какие секреты подключения сохранены в keychain (значения не передаются). */
export interface SecretsSet {
  pgPassword: boolean;
  pgKeyPassword: boolean;
  patroniPassword: boolean;
  patroniKeyPassword: boolean;
  sshPassword: boolean;
  sshPassphrase: boolean;
}
export interface ProfilePatch {
  kind: ConnectionKind;
  hosts: HostPort[];
  targetServerType: TargetServerType | null;
  username: string | null;
  database: string | null;
  sslMode: SslMode | null;
  caPath: string | null;
  clientCert: ClientCert | null;
  connectTimeoutS: number | null;
}
export interface ParsedConnection {
  patch: ProfilePatch;
  password: string | null;
  ignored: string[];
}

export interface ConnectionFolder {
  id: string;
  name: string;
  parentId: string | null;
  order: number;
}
/** Ссылка на элемент дерева подключений для перемещения. */
export interface TreeItemRef {
  kind: "folder" | "connection";
  id: string;
}

/** Итог экспорта подключений: сколько записано в файл. */
export interface ExportSummary {
  connections: number;
  folders: number;
}

/** Элемент предпросмотра импорта; `path` — имена папок от корня до места элемента. */
export interface PreviewItem {
  kind: "folder" | "connection";
  id: string;
  name: string;
  path: string[];
}

/** Подключение файла, которое не будет импортировано. */
export interface SkippedImport {
  name: string;
  reason: LocalizedText;
}

/** Предпросмотр (и итог) импорта подключений. */
export interface ImportPreview {
  added: PreviewItem[];
  updated: PreviewItem[];
  unchanged: PreviewItem[];
  skipped: SkippedImport[];
  warnings: LocalizedText[];
  needsPasswords: number;
}

/** Запланированный switchover кластера (`scheduled_switchover` из `/cluster`); `at` — время как есть (ISO 8601 со смещением). */
export interface ScheduledSwitchover {
  at: string;
  from: string | null;
  to: string | null;
}
export interface ScheduledRestart {
  at: string;
  postgresVersion: string | null;
}
export type Role = "primary" | "standbyLeader" | "syncStandby" | "quorumStandby" | "replica" | "unknown";
export interface Member {
  name: string;
  host: string;
  port: number;
  apiUrl: string | null;
  role: Role;
  rawRole: string | null;
  state: string | null;
  timeline: number | null;
  lagBytes: number | null;
  replayDelayMs: number | null;
  upstream: string | null;
  tags: Record<string, unknown>;
  pendingRestart: boolean;
  /** Запланированный restart участника (`scheduled_restart` из `/cluster`). */
  scheduledRestart: ScheduledRestart | null;
  serverVersion: string | null;
  patroniVersion: string | null;
  dcsLastSeenMs: number | null;
  reachable: boolean;
  error: AppError | null;
}
export type TopologyWarning =
  | { kind: "noPrimary" }
  | { kind: "multiplePrimaries"; members: string[] }
  | { kind: "systemIdMismatch"; members: string[] }
  | { kind: "patroniUnreachable" }
  | { kind: "memberUnreachable"; name: string };
export interface Topology {
  connectionId: string;
  kind: ConnectionKind;
  observedAtMs: number;
  scope: string | null;
  paused: boolean | null;
  scheduledSwitchover: ScheduledSwitchover | null;
  members: Member[];
  warnings: TopologyWarning[];
}
export type ConnState = "disconnected" | "connecting" | "connected" | "degraded" | "failed";
export interface TopologyView {
  state: ConnState;
  topology: Topology | null;
  /** Узел, с которого читается каталог. */
  catalogMember: string | null;
  /** Выбранный узел недоступен — каталог читается с узла по умолчанию. */
  catalogFallback: boolean;
  error: AppError | null;
}
export type TopologyEvent = { atMs: number } & (
  | { kind: "leaderChanged"; from: string | null; to: string | null }
  | { kind: "memberAdded"; name: string }
  | { kind: "memberRemoved"; name: string }
  | { kind: "memberRoleChanged"; name: string; from: Role; to: Role }
  | { kind: "memberStateChanged"; name: string; from: string | null; to: string | null }
  | { kind: "memberUnreachable"; name: string }
  | { kind: "memberReachable"; name: string }
  | { kind: "timelineChanged"; from: number; to: number }
  | { kind: "pauseChanged"; paused: boolean }
);
export interface MemberFacts {
  startedAtMs: number;
  connections: number;
  maxConnections: number;
  databases: number;
}

export type TestStep = "ssh" | "patroni" | "discovery" | "node" | "readiness";
export type StepStatus = "running" | "ok" | "failed" | "skipped";
export interface TestEvent {
  step: TestStep;
  member: string | null;
  status: StepStatus;
  message: LocalizedText | null;
  error: AppError | null;
}
export type ReadinessStatus = "ok" | "warn" | "missing";
export interface ReadinessItem {
  id: string;
  status: ReadinessStatus;
  params: Record<string, string>;
  fix: string | null;
}
export interface TestReport {
  ok: boolean;
  topology: Topology | null;
  readiness: ReadinessItem[];
}

export interface DatabaseInfo {
  name: string;
  /** null — нет права подключения и роли pg_read_all_stats. */
  sizeBytes: number | null;
}
export interface ObjectCounts {
  tables: number;
  views: number;
  matviews: number;
  sequences: number;
  functions: number;
}
export interface SchemaInfo {
  name: string;
  system: boolean;
  counts: ObjectCounts;
}
export type ObjectKind = "tables" | "views" | "matviews" | "functions" | "sequences";
export interface ObjectItem {
  oid: number;
  name: string;
  schema: string;
  relkind: string | null;
  /** Для партиционированной таблицы — число партиций. */
  partitions: number | null;
  estRows: number | null;
  totalBytes: number | null;
  /** Аргументы функции. */
  signature: string | null;
}
export interface ObjectPage {
  total: number;
  items: ObjectItem[];
  nextOffset: number | null;
}
/** Колонка таблицы или представления (зеркало `catalog::table::Column`). */
export interface Column {
  name: string;
  /** `format_type` с модификатором: `numeric(12,2)`. */
  dataType: string;
  notNull: boolean;
  /** Выражение `DEFAULT`, у генерируемой колонки — её выражение. */
  defaultExpr: string | null;
  /** `a` — `GENERATED ALWAYS AS IDENTITY`, `d` — `BY DEFAULT`. */
  identity: string | null;
  /** `s` — `GENERATED ALWAYS AS (…) STORED`. */
  generated: string | null;
  /** Сопоставление, только если отличается от сопоставления типа. */
  collation: string | null;
  /** Цель статистики; `null` — по умолчанию. */
  statTarget: number | null;
  /** `attstorage`: `p`, `e`, `m`, `x`. */
  storage: string;
  comment: string | null;
}
export interface Index {
  name: string;
  definition: string;
  primary: boolean;
  unique: boolean;
  valid: boolean;
  sizeBytes: number;
  idxScan: number | null;
  idxTupRead: number | null;
  /** Последнее сканирование (PostgreSQL 16+), миллисекунды эпохи. */
  lastIdxScanMs: number | null;
}
export interface Constraint {
  name: string;
  /** `contype`: `p`, `u`, `f`, `c`, `x`, `t`, `n`. */
  kind: string;
  definition: string;
  validated: boolean;
}
/** Внешний ключ другой таблицы, ссылающийся на эту. */
export interface IncomingFk {
  constraint: string;
  schema: string;
  table: string;
  definition: string;
}
/** У партиционированной таблицы — по всему дереву партиций: размеры и счётчики — суммы, времена — самые поздние. */
export interface TableStats {
  heapBytes: number;
  indexBytes: number;
  toastBytes: number;
  totalBytes: number;
  /** Оценка числа строк; -1 — таблицу ещё не анализировали. */
  reltuples: number;
  seqScan: number | null;
  idxScan: number | null;
  nLiveTup: number | null;
  nDeadTup: number | null;
  nModSinceAnalyze: number | null;
  nInsSinceVacuum: number | null;
  lastVacuumMs: number | null;
  lastAutovacuumMs: number | null;
  lastAnalyzeMs: number | null;
  lastAutoanalyzeMs: number | null;
  vacuumCount: number | null;
  autovacuumCount: number | null;
  analyzeCount: number | null;
  autoanalyzeCount: number | null;
}
/**
 * Ссылка на отношение: схема и имя как в каталоге, без кавычек (кавычки нужны только в DDL). Не путать с `RelationRef`
 * плана (там схема может отсутствовать).
 */
export interface CatalogRelationRef {
  schema: string;
  name: string;
}
export interface Partitioning {
  /** Ключ партиционирования (`RANGE (happened_at)`) — у партиционированной таблицы. */
  key: string | null;
  /** Границы (`FOR VALUES FROM (…) TO (…)`) — у партиции. */
  bound: string | null;
  /** Родитель партиции; у потомка по `INHERITS` — `null`. */
  parent: CatalogRelationRef | null;
  /** Прямые партиции. */
  partitions: CatalogRelationRef[];
}
/** Триггер таблицы (вкладка таблицы); триггер из плана запроса — `Trigger`. */
export interface TableTrigger {
  name: string;
  /** `tgenabled`: `O`, `D`, `R`, `A`. */
  enabled: string;
  definition: string;
}
export interface TableDetails {
  oid: number;
  schema: string;
  name: string;
  /** `r`, `p` или `f`. */
  relkind: string;
  /** Нежурналируемая таблица: не реплицируется на реплики и теряется при сбое. */
  unlogged: boolean;
  owner: string;
  tablespace: string | null;
  comment: string | null;
  reloptions: string[];
  columns: Column[];
  indexes: Index[];
  constraints: Constraint[];
  incomingFks: IncomingFk[];
  stats: TableStats;
  partitioning: Partitioning;
  triggers: TableTrigger[];
  /** DDL по каталогу (не замена pg_dump). */
  ddl: string;
}
export interface ViewDetails {
  oid: number;
  schema: string;
  name: string;
  matview: boolean;
  owner: string;
  comment: string | null;
  /** Запрос представления (`pg_get_viewdef`). */
  definition: string;
  columns: Column[];
  /** Заполнено ли материализованное представление; у обычного — `null`. */
  populated: boolean | null;
  /** Полный размер материализованного представления; у обычного — `null`. */
  sizeBytes: number | null;
  /** Индексы материализованного представления; у обычного — пусто. */
  indexes: Index[];
}
/** `prokind`: функция, процедура, агрегат, оконная функция. */
export type FunctionKind = "f" | "p" | "a" | "w";
export interface FunctionDetails {
  oid: number;
  schema: string;
  name: string;
  kind: FunctionKind;
  /** Аргументы (`pg_get_function_identity_arguments`). */
  signature: string;
  /** Тип результата; у процедуры и агрегата — `null`. */
  returns: string | null;
  language: string;
  /** `provolatile`: `i`, `s`, `v`. */
  volatility: string;
  securityDefiner: boolean;
  owner: string;
  comment: string | null;
  /** `CREATE OR REPLACE …`; у агрегата — `null`. */
  definition: string | null;
}
export interface SequenceDetails {
  oid: number;
  schema: string;
  name: string;
  /** `smallint`, `integer` или `bigint`. */
  dataType: string;
  /** Числа bigint приходят десятичными строками: JSON-число точно только до 2^53, а граница bigint выше. */
  start: string;
  increment: string;
  min: string;
  max: string;
  cycle: boolean;
  cache: string;
  /** Последнее выданное значение; `null` — последовательность не использовалась или у роли нет права. */
  lastValue: string | null;
  /** У роли есть право SELECT или USAGE: без него `lastValue` и `percentUsed` пусты при любом использовании. */
  readable: boolean;
  percentUsed: number | null;
  /** Колонка-владелец: `schema.table.column`. */
  ownedBy: string | null;
}
export interface ExtensionInfo {
  name: string;
  version: string;
  schema: string;
  defaultVersion: string | null;
  updateAvailable: boolean;
}
export interface RoleInfo {
  name: string;
  superuser: boolean;
  login: boolean;
  createDb: boolean;
  createRole: boolean;
  replication: boolean;
  bypassRls: boolean;
  connLimit: number;
  validUntil: string | null;
  memberOf: string[];
}
export interface UiSettings {
  showSystemObjects: boolean;
}

export interface PolicyStatus {
  managed: boolean;
  source: string | null;
  updatesEnabled: boolean;
  updateUrl: string | null;
  language: string | null;
  /** false — MCP-шлюз запрещён администратором. */
  mcpEnabled: boolean;
  /** false — фоновый ассистент запрещён администратором (спец. P4c §9.1); `mcpEnabled = false` запрещает и его. */
  mcpAgentEnabled: boolean;
  /** Потолок уровня доступа LLM к подключениям. */
  mcpMaxAccess: McpMaxAccess;
  errors: LocalizedText[];
}

/** Результат `check_update`. */
export type UpdateCheck =
  | { status: "disabled"; reason: LocalizedText }
  | { status: "upToDate" }
  | { status: "available"; version: string; notes: string | null; notesI18n: Record<string, string>; date: string | null };

export interface AppInfo {
  name: string;
  version: string;
  commit: string;
  os: string;
  arch: string;
  dataDir: string;
  logDir: string;
}

export type LanguageReason = "policy" | "user" | "system" | "default";
export interface LanguageSettings {
  system: string | null;
  systemMatch: string | null;
  chosen: string | null;
  policy: string | null;
  effective: string;
  reason: LanguageReason;
  languages: string[];
}

// ---------------------------------------------------------------------------------------------------------------
// Диагностика (P2a): зеркало JSON отчётов Rust (`diag::DiagReport`, `pg_advisor::*`), все поля camelCase.
// Длительности — секунды (`…S`) от `now()` сервера, размеры — байты (`…Bytes`), доли — проценты (`…Pct`), если не сказано иное.
// ---------------------------------------------------------------------------------------------------------------

/** Вкладка инструмента. `idleTx` — вкладка фронтенда поверх `sessions`, в Rust отдельного инструмента нет. */
export type DiagTool = "pulse" | "sessions" | "idleTx" | "waits" | "locks" | "horizon" | "replication" | "slots" | "failover" | "health" | "queries";
/** Инструмент Rust (`diag::Tool`). */
export type RustTool = Exclude<DiagTool, "idleTx">;
export type Severity = "info" | "warning" | "critical";
export type FindingTarget =
  | { kind: "session"; pid: number; backendStartUs: number }
  | { kind: "slot"; name: string }
  | { kind: "member"; name: string }
  | { kind: "relation"; name: string }
  | { kind: "database"; name: string }
  /** Запрос pgss: `queryId` — десятичная строка, `database` — пустая, если базы строки не знали. */
  | { kind: "query"; queryId: string; database: string };
/** Действие с сессиями, которое предлагает находка: `idleInTransaction` и `rootBlocker`. */
export type SessionActionLink =
  | { type: "terminateIdleInTransaction"; olderThanS: number }
  | { type: "terminateRootBlocker"; pid: number; backendStartUs: number };
/** Куда ведёт кнопка находки; `tool` — id вкладки (`idleTx` допустим), `action` — действие P2b (кнопки — задача 11). */
export type FindingLink =
  | { kind: "openTool"; tool: DiagTool; member: string | null }
  | { kind: "action"; action: SessionActionLink; member: string | null }
  /** Карточка запроса (вкладка `query`); без узла или с пустой базой кнопки нет. */
  | { kind: "openQuery"; queryId: string; database: string; member: string | null };
export interface Finding {
  /** Ключ текста: `findings.<id>.title` и `findings.<id>.fix`. */
  id: string;
  severity: Severity;
  member: string | null;
  target: FindingTarget | null;
  /** Имена по единицам: `…S` — секунды, `…Bytes` — байты, `…Pct` — проценты, `count` — число объектов, `command` — готовая команда. */
  params: Record<string, string | number | boolean | null>;
  links: FindingLink[];
}
/** Что не показано и почему (версия сервера, нет права); `member` — узел у инструментов кластера, иначе поля нет. */
export interface Unavailable {
  part: string;
  member?: string;
  reason: LocalizedText;
}
export interface DiagRequest {
  tool: RustTool;
  /** `null` — «за лидером»; у инструментов кластера игнорируется. */
  member: string | null;
  /** Только `waits`: 300, 900 или 3600. */
  windowS?: number;
  /** Только `queries`. */
  queries?: QueriesParams;
}

// ---------------------------------------------------------------------------------------------------------------
// Тяжёлые запросы (P3a): зеркало `pg_advisor::{queries, plan}`, `diag::{queries, query_detail}`, `explain`, `sampler::statements`.
// `queryId` — всегда строка (64-битный хеш не помещается в number).
// ---------------------------------------------------------------------------------------------------------------

export interface QueryKey {
  queryId: string;
  dbid: number;
  userid: number;
  toplevel: boolean;
}
export type QuerySort =
  | "totalTime"
  | "meanTime"
  | "maxTime"
  | "stddevTime"
  | "calls"
  | "rowsPerCall"
  | "sharedRead"
  | "hitRatio"
  | "tempWritten"
  | "walBytes"
  | "ioTime"
  | "planTime"
  | "jitTime"
  | "cpuTime";
/** Режим отчёта; `between` — дельты между двумя снимками (файлы снимков, порядок любой, Rust упорядочит). */
export type QueriesMode = { kind: "sinceReset" } | { kind: "window"; s: 300 | 900 | 3600 } | { kind: "between"; from: string; to: string };
export interface QueriesParams {
  mode: QueriesMode;
  sort: QuerySort;
  limit: 50 | 100 | 200;
  database: string | null;
  toplevelOnly: boolean;
  /** Подстрока текста запроса без учёта регистра (на сервере); пусто или нет — без фильтра. Итоги — по всем строкам. */
  search?: string | null;
}
/** Строка отчёта: накопительная («с момента сброса») или дельта за окно; `min`/`max`/`stddev` у окна — `null`. */
export interface QueryStat {
  key: QueryKey;
  datname: string | null;
  rolname: string | null;
  calls: number;
  totalExecMs: number;
  meanExecMs: number;
  minExecMs: number | null;
  maxExecMs: number | null;
  stddevExecMs: number | null;
  rows: number;
  rowsPerCall: number;
  plans: number;
  totalPlanMs: number;
  sharedHit: number;
  sharedRead: number;
  sharedDirtied: number;
  sharedWritten: number;
  localHit: number;
  localRead: number;
  localDirtied: number;
  localWritten: number;
  tempRead: number;
  tempWritten: number;
  ioReadMs: number;
  ioWriteMs: number;
  tempIoMs: number | null;
  walRecords: number;
  walFpi: number;
  walBytes: number;
  jitFunctions: number | null;
  jitMs: number | null;
  parallelPlanned: number | null;
  parallelLaunched: number | null;
  /** `pg_stat_kcache` (`null` без расширения или без строки kcache у ключа): CPU пользователя и ядра, мс; байты с диска и на диск. */
  cpuUserMs: number | null;
  cpuSysMs: number | null;
  diskReadBytes: number | null;
  diskWriteBytes: number | null;
  /** Доля в `totals.totalExecMs`, проценты; `hitRatio` — 0..1 или null без чтений. */
  sharePct: number;
  hitRatio: number | null;
  /** Текст из кеша; `null` — ещё не запрашивался (`query_texts`). */
  text: string | null;
  textTruncated: boolean;
  /** Запрос самой Глафиры (метка в начале текста): вне находок и итогов долей; Rust отдаёт поле всегда. */
  own: boolean;
}
export interface PgssMeta {
  version: string;
  /** Есть `pg_stat_statements_info` (1.9+); без него `statsResetMs` и `dealloc` — `null`. */
  hasInfo: boolean;
  max: number;
  trackPlanning: boolean;
  statsResetMs: number | null;
  dealloc: number | null;
  /** Колонки `pg_stat_kcache` (CPU, диск) заполнены: расширение установлено (между снимками — при снятии обоих). */
  kcache: boolean;
  /** «С момента сброса»: с какого момента накоплены колонки kcache (самая ранняя `stats_since`, kcache 2.3+); `null` — неизвестно. */
  kcacheSinceMs: number | null;
  /** «С момента сброса»: CPU и диск — за другой период, чем остальные колонки (pg_stat_kcache сбрасывается отдельно). */
  kcacheSeparate: boolean;
}
export interface QueriesData {
  mode: QueriesMode;
  /** Только у режима окна: сколько секунд часового буфера покрыто тиками. */
  coverageS: number | null;
  ticks: number | null;
  /** `null` — расширения нет (причина — в `unavailable`). */
  pgss: PgssMeta | null;
  totals: { calls: number; totalExecMs: number; rows: number };
  rows: QueryStat[];
}
/** Снимок pgss на диске (`snapshot_list`, новые первыми). `rows: null` и `unreadable` — файл не читается: его можно только удалить. */
export interface SnapshotInfo {
  /** Путь относительно каталога снимков подключения: `<каталог узла>/<yyyy-mm-ddThh-mm-ss>.json`; каталог — имя узла в `%XX`-форме (`db1%3A5432`), имя узла — в `member`. */
  file: string;
  member: string;
  takenAtMs: number;
  rows: number | null;
  bytes: number;
  unreadable?: "tooLarge" | "corrupt";
}
/** Запрос, среднее время которого изменилось: `before` — накопительный в `from`, `after` — дельта (без min/max/stddev). */
export interface Change {
  key: QueryKey;
  datname: string | null;
  rolname: string | null;
  before: QueryStat;
  after: QueryStat;
  ratio: number;
}
/** Сравнение двух снимков (`snapshot_compare`); строки несут `text` и `own` из снимков. */
export interface Comparison {
  /** Не больше 200 самых долгих; остаток — `newMore`. */
  new: QueryStat[];
  gone: QueryStat[];
  newMore: number;
  goneMore: number;
  degraded: Change[];
  improved: Change[];
  totalsFrom: { calls: number; totalExecMs: number; rows: number };
  totalsTo: { calls: number; totalExecMs: number; rows: number };
  reset: boolean;
}
export interface QueryText {
  key: QueryKey;
  text: string | null;
  truncated: boolean;
}
/** События сэмплера pg_stat_statements (канал `start_statements_sampler`). */
export type StatementsEvent =
  | { kind: "tick"; atMs: number; coverageS: number; ticks: number; changed: number }
  | { kind: "gap"; fromMs: number; toMs: number }
  | { kind: "error"; error: AppError }
  | { kind: "stopped" };

/** Выполняющийся экземпляр запроса: активный ведущий бэкенд с этим `query_id` (PG 14+). */
export interface RunningInstance {
  pid: number;
  backendStartUs: number;
  state: string | null;
  startedMs: number | null;
  durationS: number | null;
  waitEventType: string | null;
  waitEvent: string | null;
  usename: string | null;
  applicationName: string | null;
  clientAddr: string | null;
  query: string;
  /** Текст упёрся в `track_activity_query_size` или срезан: для плана не годится. */
  truncated: boolean;
}
export interface QueryWaits {
  source: "sampler" | "pgWaitSampling";
  sinceMs: number;
  /** Сколько секунд охватывают сэмплы (не больше часа); у профиля pg_wait_sampling — null. */
  coverageS: number | null;
  total: number;
  items: { eventType: string; event: string | null; count: number; sharePct: number }[];
}
export interface ByUser {
  rolname: string | null;
  toplevel: boolean;
  stat: QueryStat;
}
/** Статистика запроса за окно буфера сэмплера; `stat: null` — дельт запроса в окне нет. */
export interface WindowStat {
  windowS: 300 | 900 | 3600;
  stat: QueryStat | null;
}
export interface HistoryPoint {
  atMs: number;
  calls: number;
  totalExecMs: number;
  meanExecMs: number;
}
export interface QueryDetail {
  text: string | null;
  textTruncated: boolean;
  /** Сумма строк pgss запроса в базе; `null` — запроса в pgss уже нет. */
  sinceReset: QueryStat | null;
  windows: WindowStat[];
  totals: { calls: number; totalExecMs: number; rows: number };
  byUser: ByUser[];
  history: HistoryPoint[];
  gaps: { fromMs: number; toMs: number }[];
  running: RunningInstance[];
  /** `null` — ожиданий нет: ни профиля pg_wait_sampling с запросом, ни сэмплов запроса в буфере активности. */
  waits: QueryWaits | null;
  /** Буфер сэмплера активности узла; `null` — его нет («Профиль ожиданий» не открывался). `coverageS: null` — тиков ещё не было. */
  waitsSampler: { running: boolean; coverageS: number | null } | null;
  findings: Finding[];
  unavailable: Unavailable[];
  serverVersionNum: number;
  inRecovery: boolean;
  pgssVersion: string | null;
  /** `pg_stat_statements.track_planning`: выключен — планирование не считается (колонки нулевые). */
  trackPlanning: boolean;
  /** `pg_stat_kcache` установлен и читается: строки CPU и диска в метриках есть (без данных у запроса — «—»). */
  kcache: boolean;
}

// План запроса (`pg_advisor::plan`): разобранный вывод EXPLAIN (FORMAT JSON) с вычисленными долями времени.
export interface RelationRef {
  schema: string | null;
  name: string;
}
export interface Buffers {
  sharedHit: number;
  sharedRead: number;
  sharedDirtied: number;
  sharedWritten: number;
  localHit: number;
  localRead: number;
  localDirtied: number;
  localWritten: number;
  tempRead: number;
  tempWritten: number;
}
export interface Jit {
  functions: number;
  /** Времена JIT — только при ANALYZE с TIMING; иначе `null` (а не 0). */
  generationMs: number | null;
  inliningMs: number | null;
  optimizationMs: number | null;
  emissionMs: number | null;
  totalMs: number | null;
}
export interface Trigger {
  name: string;
  relation: string | null;
  timeMs: number;
  calls: number;
}
export interface Setting {
  name: string;
  value: string;
}
export interface PlanNode {
  /** Порядок обхода в глубину, с нуля. */
  id: number;
  nodeType: string;
  parentRelationship: string | null;
  subplanName: string | null;
  relation: { schema: string | null; name: string; alias: string | null } | null;
  indexName: string | null;
  strategy: string | null;
  joinType: string | null;
  parallelAware: boolean;
  startupCost: number;
  totalCost: number;
  planRows: number;
  planWidth: number;
  /** Времена — `null` при `TIMING OFF` (строки и циклы остаются). */
  actual: { startupMs: number | null; totalMs: number | null; rows: number; loops: number } | null;
  neverExecuted: boolean;
  workersPlanned: number | null;
  workersLaunched: number | null;
  rowsRemovedByFilter: number | null;
  rowsRemovedByIndexRecheck: number | null;
  rowsRemovedByJoinFilter: number | null;
  heapFetches: number | null;
  filter: string | null;
  indexCond: string | null;
  recheckCond: string | null;
  hashCond: string | null;
  joinFilter: string | null;
  mergeCond: string | null;
  sortKey: string[];
  groupKey: string[];
  sortMethod: string | null;
  sortSpaceKb: number | null;
  sortSpaceType: string | null;
  hashBatches: number | null;
  hashBuckets: number | null;
  peakMemoryKb: number | null;
  exactHeapBlocks: number | null;
  lossyHeapBlocks: number | null;
  subplansRemoved: number | null;
  buffers: Buffers | null;
  ioReadMs: number | null;
  ioWriteMs: number | null;
  output: string[];
  /** Нераспознанные поля узла — как есть. */
  extra: Record<string, unknown>;
  /** Вычисленные поля (без ANALYZE и при `TIMING OFF` времена и доли — `null`). Под `Gather` время усреднено по процессам. */
  inclusiveMs: number | null;
  exclusiveMs: number | null;
  sharePct: number | null;
  /** `null` без ANALYZE, у невыполнявшегося узла и при бесконечном отношении — ошибку оценки показывает флаг `misestimated`. */
  estimateRatio: number | null;
  hot: boolean;
  misestimated: boolean;
  children: PlanNode[];
}
export interface PlanTree {
  root: PlanNode;
  planningMs: number | null;
  executionMs: number | null;
  triggers: Trigger[];
  jit: Jit | null;
  settings: Setting[];
  /** Знаменатель долей узлов; без ANALYZE — `null`. */
  totalMs: number | null;
}

// EXPLAIN (консоль и карточка запроса).
export type ExplainMode = "plan" | "analyze";
export type ExplainParams = { kind: "none" } | { kind: "generic" } | { kind: "values"; values: (string | null)[] };
export type ExplainTier = "read" | "write";
export interface ExplainRequest {
  member: string;
  database: string;
  sql: string;
  mode: ExplainMode;
  params: ExplainParams;
  /** Только ANALYZE, 5…600 с. */
  timeoutS: number;
  searchPath: string | null;
  confirmation: Confirmation;
  /** Выполнить как запись, хотя пробный план — чтение (после `EXPLAIN_WRITES`). */
  forceWrite: boolean;
  /** Ярус записи подтверждён в `AnalyzeConfirm` («Выполнить и откатить»); без него Rust отказывает записи и `forceWrite`. */
  writeConfirmed: boolean;
}
export interface ExplainResult {
  mode: ExplainMode;
  tier: ExplainTier;
  plan: PlanTree;
  /** JSON сервера как есть. */
  raw: unknown;
  relations: RelationRef[];
  elapsedMs: number;
  /** Ключи предупреждений: `genericPlan`, `probeWithoutReadOnly`, `adminFunction`. */
  warnings: string[];
  /** Рекомендации по плану (P3b): от высокой важности к низкой. */
  recommendations: Recommendation[];
  /** Правила, которым нужен факт (ANALYZE): без него пропущены. */
  recommendationsSkipped: string[];
}

/** Вставленный план («Вставить план»): дерево и рекомендации по нему — правила чистые, сервер не нужен. */
export interface ParsedPlan {
  plan: PlanTree;
  recommendations: Recommendation[];
  /** Правила, которым нужен факт (ANALYZE): у плана без него пропущены. */
  recommendationsSkipped: string[];
}

/** Виртуальный индекс для hypopg: `CREATE INDEX` без `CONCURRENTLY`. */
export interface HypoIndex {
  indexDdl: string;
}
/**
 * Рекомендация по плану (спец. P3 §9.1). Команду приложение не выполняет — только показывает и копирует; `<…>` в ней —
 * шаблон для пользователя.
 */
export interface Recommendation {
  /** Ключ текстов `recommend.<id>.{title,evidence,effect,risks}`. */
  id: string;
  severity: Severity;
  nodeIds: number[];
  params: Record<string, string | number | boolean | null>;
  command: string | null;
  hypo: HypoIndex | null;
}
/** Проверка гипотезы hypopg: план запроса без индекса и с виртуальным индексом (`searchPath` — тот же, что у запроса). */
export interface HypoRequest {
  member: string;
  database: string;
  sql: string;
  params: ExplainParams;
  searchPath: string | null;
  indexDdl: string;
}
export interface HypoResult {
  indexName: string;
  estimatedBytes: number;
  costBefore: number;
  costAfter: number;
  used: boolean;
  planAfter: PlanTree;
}

// «Скопировать для LLM».
export type LlmSource = { kind: "query"; queryId: string } | { kind: "sql"; sql: string };
export interface LlmTable {
  schema: string;
  name: string;
  /** DDL каталога; индексы, не принадлежащие ограничениям, уже в нём. */
  ddl: string;
}
export interface LlmStats {
  sinceReset: QueryStat | null;
  window3600: QueryStat | null;
}
export interface LlmBundle {
  serverVersionNum: number;
  pgssVersion: string | null;
  inRecovery: boolean;
  member: string;
  database: string;
  text: string | null;
  textTruncated: boolean;
  stats: LlmStats | null;
  planText: string | null;
  tables: LlmTable[];
  /** Таблиц плана, не попавших в `tables`: сверх лимита или с непрочитанным DDL. */
  tablesOmitted: number;
  /** Ключи: `paramsNeeded`, `queryGone`, `planFailed`, `ddlFailed`. */
  warnings: string[];
}
// Запросы на оптимизацию и предложения LLM (спец. P4 §8).
/** Откуда запрос: консоль EXPLAIN (одна на подключение) или карточка запроса pgss. */
export type RequestSource = { kind: "console" } | { kind: "query"; queryId: string };
export type RequestStatus = "waiting" | "answered" | "cancelled";
/** Снимок запроса на оптимизацию (`llm-requests.json`): модель забирает его инструментом `get_optimization_request`. */
export interface OptimizationRequest {
  /** `r` + 6 hex. */
  id: string;
  connectionId: string;
  member: string;
  database: string;
  source: RequestSource;
  sql: string;
  /** Значения `$N` последнего плана консоли; пустое поле — null. */
  params: (string | null)[];
  /** Снимок `LlmBundle` (P3): уходит модели как есть, без маскирования. */
  bundle: LlmBundle;
  createdAtMs: number;
  status: RequestStatus;
  proposalIds: string[];
}
/** Аргументы `create_optimization_request`: `plan` — последний план вкладки (как у «Скопировать для LLM»), без него Rust строит пробный. */
export interface CreateRequestArgs {
  member: string;
  database: string;
  source: RequestSource;
  sql: string;
  params: (string | null)[];
  plan: PlanTree | null;
}
export type ProposalStatus = "new" | "accepted" | "rejected";
export interface ProposalIndexUse {
  ddl: string;
  used: boolean;
}
/** Итог «Сравнить планы»: EXPLAIN без выполнения обоих запросов на одном узле, предложенный — с индексами через hypopg. */
export interface ProposalComparison {
  costBefore: number;
  costAfter: number;
  indexesUsed: ProposalIndexUse[];
  member: string;
  atMs: number;
  planBefore: PlanTree;
  planAfter: PlanTree;
  recommendationsBefore: Recommendation[];
  recommendationsAfter: Recommendation[];
  /** В базе нет hypopg: предложенный запрос спланирован без своих индексов. */
  withoutHypopg: boolean;
}
/** Предложение LLM (`llm-proposals.json`). Необязательные поля Rust может не прислать: отсутствие и null равнозначны. */
export interface LlmProposal {
  /** `p` + 6 hex. */
  id: string;
  source: "mcp" | "app";
  /** Имя MCP-клиента, приславшего предложение. */
  client?: string | null;
  requestId?: string | null;
  connectionId: string;
  database: string;
  member?: string | null;
  originalSql: string;
  originalQueryId?: string | null;
  rewrittenSql: string;
  indexes: string[];
  rationale: string;
  expectedEffect?: string | null;
  status: ProposalStatus;
  /** Причина отклонения (до 500 символов), видна модели. */
  reason?: string | null;
  comparison?: ProposalComparison | null;
  createdAtMs: number;
  answeredAtMs?: number | null;
  /** Предложение фонового запуска (спец. P4c §7): панель пишет «От фонового ассистента (агент, модель)». */
  runId?: string | null;
}
/**
 * Фильтр `list_llm_proposals`. `connectionId` сужает всегда; `requestIds`, `database` и `queryId` объединяются (ИЛИ):
 * подходит предложение, совпавшее хотя бы с одним из заданных (`database` — только предложения без запроса). Если ни одного
 * из трёх нет, подходят все предложения подключения.
 */
export interface ProposalFilter {
  connectionId?: string | null;
  database?: string | null;
  requestIds?: string[] | null;
  queryId?: string | null;
}
/**
 * Событие `mcp://proposals`: новое предложение или смена статуса запроса подключения.
 */
export interface ProposalsEvent {
  kind: "request" | "proposal";
  connectionId: string;
  requestId?: string | null;
  proposalId?: string | null;
}
// Фоновый ассистент (спец. P4c): запуски агентского CLI и их статус.
/** Пресет агентского CLI (`agent::settings::Preset`): названия программ не переводятся. */
export type AgentPreset = "qwen" | "claude";
/** Запуск на запрос на оптимизацию или «Проверить» в «Настройках» (без подключения и запроса). */
export type RunKind = "optimize" | "check";
/** Жизненный цикл запуска (спец. P4c §3.2). */
export type RunStatus = "queued" | "starting" | "running" | "done" | "failed" | "cancelled" | "interrupted" | "skipped";
/**
 * Причина `failed` (спец. P4c §3.2). `unsafeTools` — модели доступно что-то кроме инструментов Glafira (§4.1 «Проверка
 * инструментов»): имена — в предупреждении `unsafe <имена через запятую>` или `unsafe no-init`.
 */
export type FailReason =
  | "cliNotFound"
  | "spawnFailed"
  | "timeout"
  | "limit"
  | "exit"
  | "notConnected"
  | "mcpOff"
  | "accessOff"
  | "policy"
  | "notConfigured"
  | "unsafeTools";
/**
 * Запуск фонового агента (`agent-runs.json`, событие `agent://runs`). Необязательные поля Rust может не прислать:
 * отсутствие и null равнозначны.
 */
export interface AgentRun {
  /** `a` + 6 hex. */
  id: string;
  kind: RunKind;
  requestId?: string | null;
  connectionId?: string | null;
  preset: AgentPreset;
  /** Модель из настроек пресета; нет — модель клиента по умолчанию. */
  model?: string | null;
  cliVersion?: string | null;
  /** Путь программы запуска (спец. P4c §10). */
  path?: string | null;
  status: RunStatus;
  reason?: FailReason | null;
  exitCode?: number | null;
  queuedAtMs: number;
  startedAtMs?: number | null;
  endedAtMs?: number | null;
  /** Вызовов инструментов за запуск (во всех его сессиях). */
  calls: number;
  lastTool?: string | null;
  lastCallAtMs?: number | null;
  /** Предложения этого запуска. */
  proposalIds: string[];
  /** Итоговый текст агента (до 4 КБ): показывается как простой текст. */
  summary?: string | null;
  /** Стоимость, если CLI её сообщил (Claude Code без Ollama). */
  costUsd?: number | null;
  /**
   * Оценка стоимости облачного запуска через Ollama по ценам Ollama Cloud (Qwen Code, Claude Code через Ollama; смоук P4c,
   * sf-T6). У записей до sf-T6 поля нет.
   */
  costEstimate?: CostEstimate | null;
  /**
   * Предупреждения запуска. `evict <другие модели через запятую> <модель запуска>` — Ollama может выгрузить другую модель;
   * `unsafe <имена через запятую>` / `unsafe no-init` — проверка инструментов остановила запуск. Прочие — как есть.
   */
  warnings: string[];
}
/** Оценка стоимости запуска по таблице цен Ollama Cloud (`agent::runs::CostEstimate`). */
export interface CostEstimate {
  /** Сумма, $. */
  usd: number;
  /** День цен таблицы, `ГГГГ-ММ-ДД`. */
  pricesDate: string;
  /** Цены «вне пика» (будни вне 12:00–18:00 UTC и выходные по началу запуска). */
  offPeak: boolean;
}
/** Действующая таблица цен Ollama Cloud (`agent::pricing::OllamaPrices`). */
export interface OllamaPrices {
  /** День цен, `ГГГГ-ММ-ДД`. */
  date: string;
  /** Адрес страницы цен. */
  source: string;
  /** Загружена «Обновить цены»; иначе — встроенная. */
  saved: boolean;
  /** Когда загружена (мс эпохи); у встроенной — нет. */
  fetchedAtMs?: number | null;
  /** Моделей в таблице. */
  models: number;
  /** «Обновить цены» разрешена политикой (`updatesEnabled`). */
  refreshAllowed: boolean;
}
/** Фильтр `list_agent_runs`: поля — пересечение; без полей — все запуски (последние 50). */
export interface RunFilter {
  connectionId?: string | null;
  requestIds?: string[] | null;
}
/** Фоновый ассистент в статусе MCP (`McpStatus.agent`, спец. P4c §8.4). */
export interface AgentStatus {
  /** Политика: `mcpEnabled` и `mcpAgentEnabled`. */
  policyEnabled: boolean;
  /** Выключатель «Запускать ассистента в фоне». */
  enabled: boolean;
  /** Агент настроен (`AgentSettings::configured`): путь программы выбранного пресета задан, у Claude Code через Ollama — и модель. */
  configured: boolean;
  preset: AgentPreset;
  /** Идущий запуск (`starting` или `running`). */
  running?: { runId: string; requestId?: string | null } | null;
  /** Запусков в очереди (все подключения). */
  queued: number;
}
/** Qwen Code в `settings.json` → `mcp.agent.qwen`: пустая модель — `model.name` из `~/.qwen/settings.json`. */
export interface QwenAgentSettings {
  path: string;
  model: string;
}
/** Claude Code: через Ollama модель обязательна; `budgetUsd` — предел расходов на запуск без Ollama (null — без предела). */
export interface ClaudeAgentSettings {
  path: string;
  model: string;
  viaOllama: boolean;
  budgetUsd: number | null;
}
/** «Фоновый ассистент» (`settings.json` → `mcp.agent`, спец. P4c §8.3, `agent::settings::AgentSettings`). Секретов в нём нет. */
export interface AgentSettings {
  /** «Запускать ассистента в фоне». */
  enabled: boolean;
  preset: AgentPreset;
  qwen: QwenAgentSettings;
  claude: ClaudeAgentSettings;
  /** Срок запуска, мин: 2–60. */
  timeoutMin: number;
  /** Держать модель Ollama в памяти после запуска, мин: 0–120 (0 — не управлять). */
  keepWarmMin: number;
  ollamaUrl: string;
}
/** Итог «Найти» (`agent::detect::Detected`): путь и версия программы пресета. */
export interface AgentDetected {
  /** Найденный абсолютный путь; нет — не нашлась. */
  path?: string | null;
  version?: string | null;
  /** Версия ниже минимальной пресета (`agent::presets::min_version`). */
  belowMinimum: boolean;
  /**
   * Почему версию не узнать (технический английский текст): бывает только у найденной или заданной программы, когда
   * `<путь> --version` не запустился, не ответил за 10 с или ответил ошибкой. Не нашлась — `path` пуст, а `error` нет.
   */
  error?: string | null;
}
/** Модели для поля «Модель» (`agent::models::AgentModels`): `source` — откуда список (`none` — только свободный ввод). */
export interface AgentModels {
  ids: string[];
  /** Модель по умолчанию (у Qwen Code — `model.name` его настроек). */
  default?: string | null;
  source: "qwen" | "ollama" | "none";
}
/** Запись журнала для EXPLAIN ANALYZE яруса записи (`ActionRecord.explain`). */
export interface ExplainRecord {
  database: string;
  tables: string[];
  timeoutS: number;
  sqlHead: string;
}

// Сессии и idle in transaction (`pg_advisor::rules::sessions`).
export interface SessionRow {
  pid: number;
  leaderPid: number | null;
  backendType: string | null;
  database: string | null;
  user: string | null;
  application: string | null;
  clientAddr: string | null;
  clientPort: number | null;
  /** `backend_start` в микросекундах эпохи: вместе с `pid` — ключ сессии. */
  backendStartUs: number | null;
  xactAgeS: number | null;
  queryAgeS: number | null;
  stateAgeS: number | null;
  state: string | null;
  waitEventType: string | null;
  waitEvent: string | null;
  backendXid: string | null;
  xminAge: number | null;
  /** PostgreSQL 14+. Десятичная строка: 64-битный хеш не помещается в точное число JS. */
  queryId: string | null;
  query: string | null;
  /** Текст обрезан по пределу списка (16 384 символа). */
  queryTruncated: boolean;
  /** Кто блокирует сессию — только у ждущих блокировку. */
  blockedBy: number[];
  locksHeld: number;
  /** Соединение самой Glafira. */
  glafira: boolean;
}
export interface SessionsData {
  rows: SessionRow[];
  settings: { idleInTransactionSessionTimeoutMs: number | null; trackActivityQuerySize: number | null };
}

// Пульс (`diag::pulse`, `pg_advisor::rules::pulse`).
export interface ConnectionsByState {
  active: number;
  idle: number;
  idleInTransaction: number;
  other: number;
}
export interface PulseFacts {
  used: number;
  /** Доступно обычным ролям: `max_connections` без резервов. */
  available: number;
  byState: ConnectionsByState;
  longestTransactionS: number | null;
  longestQueryS: number | null;
  longestIdleInTransactionS: number | null;
  lockWaiting: number;
  longestLockWaitS: number | null;
  maxXidAge: number;
  /** Доля от 2³¹, проценты. */
  xidAgePct: number;
  /** `null` — нет роли `pg_monitor`. */
  walDirBytes: number | null;
  maxWalSizeBytes: number | null;
  /** Из топологии, не из SQL узла. */
  lagBytes: number | null;
  replayDelayMs: number | null;
}
/** Скорости и приросты между двумя снимками счётчиков; без сравнимого снимка поля пусты. */
export interface PulseRates {
  tps: number | null;
  commitsPerS: number | null;
  rollbacksPerS: number | null;
  cacheHitPct: number | null;
  tempFilesPerS: number | null;
  tempBytesPerS: number | null;
  walBytesPerS: number | null;
  deadlocks: number | null;
  tempFiles: number | null;
  blksRead: number | null;
}
export interface PulseNode {
  member: string;
  role: Role;
  /** Достижимость по топологии; при `error` факты пусты. */
  reachable: boolean;
  error: AppError | null;
  facts: PulseFacts | null;
  rates: PulseRates | null;
}
export interface PulseData {
  nodes: PulseNode[];
}

// Профиль ожиданий (`pg_advisor::ash`).
export type WaitClass = "cpu" | "lock" | "lwlock" | "io" | "ipc" | "client" | "timeout" | "bufferPin" | "extension" | "activity" | "other";
export interface WaitBucket {
  startMs: number;
  /** Сколько секунд интервала работал сэмплер. */
  sampledS: number;
  /** Среднее число активных сессий по классам; отсутствующий класс — 0. */
  aas: Partial<Record<WaitClass, number>>;
}
/** Промежуток в истории: сэмплер не работал. */
export interface WaitGap {
  fromMs: number;
  toMs: number;
}
export interface TopEvent {
  class: WaitClass;
  /** У `cpu` пусто. */
  event: string | null;
  samples: number;
  /** Доля сэмплов, 0..1. */
  share: number;
}
export interface TopQuery {
  /** `id:<query_id>` или `text:<хэш>`. */
  key: string;
  /** Десятичная строка: 64-битный хеш не помещается в точное число JS. */
  queryId: string | null;
  text: string;
  samples: number;
  share: number;
  /** `Тип:событие`, `Тип` или `CPU*`. */
  topEvent: string | null;
}
export interface TopName {
  name: string;
  samples: number;
  share: number;
}
export interface WaitsData {
  windowS: number;
  bucketS: number;
  samplerRunning: boolean;
  /** Окно длиннее накопленной истории. */
  truncated: boolean;
  totalSamples: number;
  buckets: WaitBucket[];
  gaps: WaitGap[];
  topEvents: TopEvent[];
  topQueries: TopQuery[];
  byApplication: TopName[];
  byUser: TopName[];
  byDatabase: TopName[];
  byClient: TopName[];
}

// Блокировки (`pg_advisor::locktree`).
export interface LockSession {
  pid: number;
  backendStartUs: number | null;
  user: string | null;
  application: string | null;
  database: string | null;
  state: string | null;
  query: string | null;
  xactAgeS: number | null;
  queryAgeS: number | null;
}
/** Блокировка, которую сессия запросила и не получила. */
export interface WaitingLock {
  locktype: string | null;
  mode: string | null;
  /** Имя — только для отношения БД соединения. */
  relation: string | null;
  relationOid: number | null;
  /** Для отношения другой БД `relation` пуст: показывается «oid N в БД X». */
  relationDatabase: string | null;
  waitS: number | null;
  /** Ожидание оценено по `state_change` (PostgreSQL 13), а не измерено. */
  approx: boolean;
}
export interface LockNode {
  session: LockSession;
  /** У корня-блокирующего пусто; у ждущего корня (чей блокирующий не виден) задано. */
  lock: WaitingLock | null;
  /** Остальные блокирующие; 0 — подготовленная транзакция или уже завершившаяся сессия. */
  alsoBlockedBy: number[];
  /** Сколько сессий в поддереве (без самого узла); у ждущего корня может быть 0. */
  victims: number;
  children: LockNode[];
}
/** DDL в очереди: ждёт блокировку отношения и сам задерживает других. */
export interface DdlQueueItem {
  session: LockSession;
  /** Кого ждёт сам DDL; `null` — блокирующий не виден (подготовленная транзакция). */
  waitsFor: number | null;
  waitS: number | null;
  /** Сколько сессий стоит за DDL. */
  queued: number;
  relation: string | null;
  relationOid: number | null;
  relationDatabase: string | null;
  mode: string;
}
export interface LocksData {
  roots: LockNode[];
  /** Циклы ожидания: pid участников по возрастанию. */
  cycles: number[][];
  ddlQueue: DdlQueueItem[];
}

// Горизонт xmin (`pg_advisor::rules::horizon`).
export type HolderKind = "session" | "preparedXact" | "slot" | "standbyFeedback";
export interface Holder {
  kind: HolderKind;
  /** Сессия — `pid`, prepared transaction — `gid`, слот — имя, обратная связь — `application_name`. */
  name: string;
  /** Участник топологии: у `standbyFeedback` и у слота с активным отправителем WAL. */
  member: string | null;
  xminAge: number;
  durationS: number | null;
  pid: number | null;
  backendStartUs: number | null;
  user: string | null;
  application: string | null;
  database: string | null;
  state: string | null;
  gid: string | null;
  slotType: string | null;
  active: boolean | null;
  /** Слот держит горизонт каталога, а не данных. */
  catalog: boolean;
  clientAddr: string | null;
}
export interface HorizonData {
  /** По убыванию `xminAge`. */
  holders: Holder[];
  hotStandbyFeedback: boolean | null;
}

// Реплики (`pg_advisor::rules::replication`).
export interface Sender {
  pid: number;
  user: string | null;
  application: string | null;
  /** Участник топологии по `application_name` (без DNS); не сопоставился — `null`. */
  member: string | null;
  clientAddr: string | null;
  state: string | null;
  syncState: string | null;
  syncPriority: number | null;
  sentLagBytes: number | null;
  writeLagBytes: number | null;
  flushLagBytes: number | null;
  replayLagBytes: number | null;
  writeLagS: number | null;
  flushLagS: number | null;
  replayLagS: number | null;
  /** Сколько прошло с последнего ответа реплики. */
  replyAgeS: number | null;
  slotName: string | null;
  xminAge: number | null;
}
export interface Receiver {
  status: string | null;
  senderHost: string | null;
  senderPort: number | null;
  slotName: string | null;
  receivedTli: number | null;
  lastMsgAgeS: number | null;
  latestEndAgeS: number | null;
}
export interface Replay {
  receiveReplayGapBytes: number | null;
  replayDelayS: number | null;
  /** `not paused` / `pause requested` / `paused`. */
  pause: string | null;
}
export interface Conflicts {
  tablespace: number;
  lock: number;
  snapshot: number;
  bufferpin: number;
  deadlock: number;
  /** PostgreSQL 16+. */
  activeLogicalslot: number | null;
  /** Прирост с прошлого запроса того же узла; без сравнимого снимка — `null`. */
  growth: number | null;
}
export interface ReplicationData {
  inRecovery: boolean;
  senders: Sender[];
  synchronousStandbyNames: string | null;
  synchronousCommit: string | null;
  receiver: Receiver | null;
  replay: Replay | null;
  conflicts: Conflicts | null;
  hotStandbyFeedback: boolean | null;
  /** `-1` — ждать без предела. */
  maxStandbyStreamingDelayMs: number | null;
  /** Роль без `pg_monitor`: сведения чужих сессий скрыты, пустые поля не признак поломки. */
  detailsHidden: boolean;
}

// Слоты и подписки (`pg_advisor::rules::slots`).
export type PatroniSlot = "member" | "permanent";
export interface Slot {
  name: string;
  /** `physical` / `logical`. */
  slotType: string;
  plugin: string | null;
  database: string | null;
  temporary: boolean;
  active: boolean;
  activePid: number | null;
  xminAge: number | null;
  catalogXminAge: number | null;
  restartLsn: string | null;
  confirmedFlushLsn: string | null;
  /** `reserved` / `extended` / `unreserved` / `lost`. */
  walStatus: string | null;
  safeWalSize: number | null;
  retainedBytes: number | null;
  /** PostgreSQL 14+. */
  twoPhase: boolean | null;
  /** PostgreSQL 16+. */
  conflicting: boolean | null;
  /** PostgreSQL 17+: момент (мс эпохи), с которого слот неактивен. */
  inactiveSinceMs: number | null;
  /** PostgreSQL 17+. */
  invalidationReason: string | null;
  /** PostgreSQL 17+. */
  failover: boolean | null;
  /** PostgreSQL 17+. */
  synced: boolean | null;
  /** Только у подключения `patroni`. */
  patroni: PatroniSlot | null;
}
export interface Subscription {
  name: string;
  /** Нет процесса применения — подписка не работает. */
  pid: number | null;
  receivedLsn: string | null;
  lastMsgAgeS: number | null;
  latestEndAgeS: number | null;
  /** PostgreSQL 15+. */
  applyErrorCount: number | null;
  syncErrorCount: number | null;
}
export interface SlotsData {
  slots: Slot[];
  subscriptions: Subscription[];
  /** Мегабайты; `-1` — без ограничения. */
  maxSlotWalKeepSizeMb: number | null;
  subscriptionErrorsGrowth: number | null;
  /** Роль подключения может удалять слоты: суперпользователь или `REPLICATION`. */
  canDropSlots: boolean;
}

// Готовность к failover (`pg_advisor::failover`) и матрица health (`diag::patroni`).
export type SyncMode = "off" | "on" | "quorum";
export interface DynConfig {
  maximumLagOnFailover: number;
  synchronousMode: SyncMode;
  synchronousModeStrict: boolean;
  synchronousNodeCount: number | null;
  ttl: number | null;
  loopWait: number | null;
  retryTimeout: number | null;
  /** Имена постоянных слотов. */
  permanentSlots: string[];
}
export type FailoverReason = "unreachable" | "notStreaming" | "lagTooHigh" | "lagUnknown" | "noFailoverTag" | "zeroPriority" | "notSynchronous";
export interface Candidate {
  member: string;
  ready: boolean;
  reasons: FailoverReason[];
  lagBytes: number | null;
  pendingRestart: boolean;
}
export interface FailoverData {
  paused: boolean;
  config: DynConfig;
  candidates: Candidate[];
  readyCount: number;
}
/** Оценка клетки по роли участника (Rust `HealthGrade`): от неё цвет клетки. */
export type HealthGrade = "ok" | "notApplicable" | "degraded" | "bad";
export interface HealthCell {
  endpoint: string;
  /** HTTP-код; `null` — API не ответил (причина в `error`). */
  status: number | null;
  error: AppError | null;
  grade: HealthGrade;
}
export interface HealthMember {
  member: string;
  apiUrl: string | null;
  cells: HealthCell[];
}
export interface HealthData {
  endpoints: string[];
  members: HealthMember[];
}

/** Данные инструмента; `kind` — id инструмента Rust. */
export type DiagData =
  | ({ kind: "pulse" } & PulseData)
  | ({ kind: "sessions" } & SessionsData)
  | ({ kind: "waits" } & WaitsData)
  | ({ kind: "locks" } & LocksData)
  | ({ kind: "horizon" } & HorizonData)
  | ({ kind: "replication" } & ReplicationData)
  | ({ kind: "slots" } & SlotsData)
  | ({ kind: "failover" } & FailoverData)
  | ({ kind: "health" } & HealthData)
  | ({ kind: "queries" } & QueriesData);
/** Конверт отчёта. У инструментов кластера `member`, `serverVersionNum` и `inRecovery` пусты. */
export interface DiagReport<D extends DiagData = DiagData> {
  tool: RustTool;
  member: string | null;
  followsLeader: boolean;
  /** Время сервера (узловые) или приложения (кластера), мс эпохи. */
  observedAtMs: number;
  serverVersionNum: number | null;
  inRecovery: boolean | null;
  data: D;
  findings: Finding[];
  unavailable: Unavailable[];
}

/** События сэмплера узла (канал `start_sampler`). */
export type SamplerEvent =
  | { kind: "tick"; atMs: number; active: number; waiting: number }
  | { kind: "gap"; fromMs: number; toMs: number }
  | { kind: "error"; error: AppError }
  | { kind: "stopped" };

// Действия дежурного (P2b). Типы повторяют структуры `src-tauri/src/actions/*` (sessions, slots, patroni, watch,
// journal): поля — camelCase, `Option` — `| null`, перечисления с `#[serde(tag = …)]` — объединения по тегу.

/** Подтверждение действия (спец. P2 §8.1): имя подключения на prod, имя объекта или число сессий. */
export interface Confirmation {
  confirm: string | null;
  typedName: string | null;
}
/** Итог действия и записи журнала (`journal::Outcome`). */
export type ActionOutcome = "ok" | "partial" | "error";

export type SignalKind = "cancel" | "terminate";
/** Сессия адресуется парой `(pid, backendStartUs)`: PID переиспользуется, время запуска — нет. */
export interface SessionRef {
  pid: number;
  backendStartUs: number;
}
export type SessionTarget =
  | { kind: "selected"; sessions: SessionRef[] }
  | { kind: "idleInTransaction"; olderThanS: number }
  | { kind: "rootBlocker"; pid: number; backendStartUs: number };
export interface SessionActionRequest {
  member: string;
  kind: SignalKind;
  target: SessionTarget;
}
/** Почему сессию нельзя отменить или завершить. */
export type DenyReason = "superuserSession" | "otherRole" | "notClientBackend" | "ownConnection" | "notRunning";
export interface PreviewSession {
  pid: number;
  backendStartUs: number;
  user: string | null;
  database: string | null;
  application: string | null;
  client: string | null;
  state: string | null;
  backendType: string | null;
  xactAgeS: number | null;
  stateAgeS: number | null;
  /** Первые 500 символов текста запроса. */
  query: string | null;
}
export interface PreviewRow {
  session: PreviewSession;
  allowed: boolean;
  reason: DenyReason | null;
}
export interface SessionActionPreview {
  /** Узел, к которому относится предпросмотр: выполнение принимает именно его. */
  member: string;
  rows: PreviewRow[];
}
/** Запрос выполнения: сессии, показанные в предпросмотре и разрешённые пользователем, и критерий цели для проверки. */
export interface SessionActionExecute extends SessionActionRequest {
  sessions: SessionRef[];
}
/** Итог одной сессии: пара `(pid, backendStartUs)` и тег `result` (у `failed` — ещё `error`). */
export type SessionResult = SessionRef &
  ({ result: "terminated" | "signalSent" | "gone" | "noLongerMatches" | "notAllowed" } | { result: "failed"; error: AppError });
export interface SessionActionResult {
  member: string;
  results: SessionResult[];
  outcome: ActionOutcome;
  /** `false` — действие выполнено, но запись в журнал не удалась. */
  journalWritten: boolean;
}

export interface SlotDropRequest {
  member: string;
  slot: string;
}
export interface SlotDropPreview {
  member: string;
  /** Слот с отметкой Patroni (`patroni`): интерфейс предупреждает, что Patroni его пересоздаст. */
  slot: Slot;
}
/** Итог действия без подробностей: записан ли журнал. */
export interface ActionResult {
  journalWritten: boolean;
}

/**
 * Действие Patroni; `scheduledAt` — время RFC 3339 со смещением (см. `toRfc3339`), `null` — выполнить сейчас. `leader` у
 * switchover — лидер, которого показал предпросмотр: Rust откажет, если он сменился (`errors.actions.leaderChanged`), и
 * отправит его в теле запроса, чтобы оставшееся окно закрыл отказ самого Patroni.
 */
export type PatroniAction =
  | { kind: "switchover"; leader: string; candidate: string | null; scheduledAt: string | null }
  | { kind: "cancelSwitchover" }
  | { kind: "failover"; candidate: string }
  | { kind: "restart"; member: string; onlyPending: boolean; scheduledAt: string | null }
  | { kind: "cancelRestart"; member: string }
  | { kind: "pause"; paused: boolean };
export interface MemberPreview {
  name: string;
  role: Role;
  state: string | null;
  pendingRestart: boolean;
  scheduledRestart: ScheduledRestart | null;
}
export interface PatroniActionPreview {
  leader: string | null;
  /** Состояние лидера, как его прислал Patroni (`running`, `stopped`…). */
  leaderState: string | null;
  /** Лидер отвечает при опросе; `null` — лидера в топологии нет. */
  leaderReachable: boolean | null;
  paused: boolean;
  /** Switchover и failover: готовность реплик. */
  candidates: Candidate[];
  /** Switchover и failover: клиентские сессии лидера, они оборвутся; `null` — не удалось прочитать. */
  leaderClientSessions: number | null;
  /** Restart и его отмена. */
  member: MemberPreview | null;
  scheduledSwitchover: ScheduledSwitchover | null;
  /** В профиле есть логин API: без него выполнение откажет (`errors.patroni.authRequired`). */
  canSend: boolean;
}
export interface PatroniActionResult {
  /** Ответ Patroni; пусто при неизвестном исходе. */
  response: string;
  /** Сессия наблюдения (для `stopSession`); `null` — не наблюдается. */
  watch: string | null;
  journalWritten: boolean;
  /** Запрос ушёл, но ответа нет: выполнилось ли действие, покажет наблюдение. */
  outcomeUnknown: boolean;
  /** Причина неизвестного исхода (при `outcomeUnknown`). */
  error: AppError | null;
}
export type WatchStep =
  | "accepted"
  | "leaderStepped"
  | "newLeaderRunning"
  | "timelineIncreased"
  | "replicasStreaming"
  | "restarted"
  | "memberRunning"
  | "pendingCleared"
  | "pauseApplied"
  | "done";
export type WatchStatus = "running" | "done" | "failed" | "timedOut";
/** События наблюдения после действия Patroni (канал `execute_patroni_action`). */
export interface WatchEvent {
  step: WatchStep;
  status: WatchStatus;
  atMs: number;
  /** Причина `failed` (например, подключение закрыто). */
  detail: AppError | null;
}

// Журнал действий (`actions.jsonl`).
export interface SessionRecord {
  pid: number;
  backendStartUs: number;
  user: string | null;
  database: string | null;
  application: string | null;
  client: string | null;
  state: string | null;
  xactAgeS: number | null;
  /** Десятичной строкой: 64-битный хеш не помещается в число JS. */
  queryId: string | null;
  query: string | null;
  /** `terminated` | `signalSent` | `gone` | `noLongerMatches` | `notAllowed` | `failed`. */
  result: string;
}
export interface SlotRecord {
  name: string;
  slotType: string;
  plugin: string | null;
  database: string | null;
  retainedBytes: number | null;
  walStatus: string | null;
}
export interface PatroniRecord {
  method: string;
  /** Полный адрес без учётных данных. */
  path: string;
  /** Тело запроса (JSON) или `null`. */
  body: unknown;
  status: number | null;
  /** Первые 500 символов ответа. */
  response: string | null;
}
export interface ActionRecord {
  /** Мс эпохи. */
  ts: number;
  /** `cancel` | `terminate` | `dropSlot` | `switchover` | `cancelSwitchover` | `failover` | `restart` | `cancelRestart` | `pause` | `resume` | `explainAnalyze`. */
  action: string;
  connection: { id: string; name: string; environment: Environment };
  member: string | null;
  prodConfirmed: boolean;
  outcome: ActionOutcome;
  /** `AppError.code` при `outcome: "error"`. */
  code: string | null;
  ms: number;
  /** Пустые `sessions`, `slot`, `patroni` и `explain` в журнал не пишутся: поля отсутствуют. */
  sessions?: SessionRecord[];
  slot?: SlotRecord;
  patroni?: PatroniRecord;
  /** Только `explainAnalyze`: запись яруса записи EXPLAIN ANALYZE. */
  explain?: ExplainRecord;
}

/** Состояние MCP-сервера (спец. P4 §11): выключен, работает, обслуживает другой экземпляр Glafira, ошибка запуска. */
export type McpServerState = "stopped" | "running" | "otherInstance" | "error";

/** Сессия MCP-клиента (`glafira_mcp_core::server::SessionSnapshot`); `active` — сейчас идёт вызов. */
export interface McpSessionSnapshot {
  id: string;
  clientName: string | null;
  clientVersion: string | null;
  startedMs: number;
  calls: number;
  active: boolean;
  /** Сессия фонового запуска, опознанная по билету (спец. P4c §5.1). */
  runId?: string | null;
}

/** Сессия ждёт ответа пользователя в диалоге подтверждения; без имени — «Неизвестный MCP-клиент». */
export interface McpPendingApproval {
  sessionId: string;
  clientName: string | null;
  clientVersion: string | null;
}

/** Подтверждённый клиент (`settings.json` → `mcp.approvedClients`). */
export interface McpApprovedClient {
  name: string;
  version: string | null;
  firstSeenMs: number;
  lastSeenMs: number;
}

/**
 * Статус MCP целиком: ответ `get_mcp_status` и полезная нагрузка события `mcp://state` (задача 8, `mcp::McpStatus`).
 */
export interface McpStatus {
  server: McpServerState;
  /** Текст ошибки запуска сервера (английский, из Rust) при `server: "error"`. */
  error: string | null;
  /** Выключатель «Разрешить MCP-сервер» в `settings.json` (задача 8). */
  enabled: boolean;
  sessions: McpSessionSnapshot[];
  pending: McpPendingApproval[];
  approvedClients: McpApprovedClient[];
  /** Подключений с действующим уровнем ≠ off и из них — с уровнем data (оба счётчика с учётом политики). */
  openConnections: number;
  dataConnections: number;
  /** Политика `mcpEnabled` (нет политики — true). */
  policyEnabled: boolean;
  /** Потолок политики `mcpMaxAccess`. */
  maxAccess: PolicyStatus["mcpMaxAccess"];
  /** Фоновый ассистент (спец. P4c §8.4). */
  agent: AgentStatus;
}

/**
 * Строка журнала MCP `mcp-audit.jsonl` (спец. P4 §10.3): `event` — "call", "sessionStarted", "sessionEnded", "approved",
 * "rejected", "revoked", "serverStarted", "serverStopped" (P4b и новее добавят свои); поля — по виду события.
 */
export interface McpAuditEntry {
  ts: number;
  event: string;
  session?: string;
  client?: string | null;
  tool?: string;
  connection?: { id: string; name: string } | null;
  member?: string | null;
  /** Аргументы вызова; длиннее 1 КБ (кроме `sql`) журнал хранит усечённой строкой. */
  args?: unknown;
  outcome?: "ok" | "error";
  code?: string | null;
  bytes?: number;
  ms?: number;
  /** Счётчики приложения (`CallRecord.extra`): замаскировано литералов; строк `run_select` (P4b). */
  masked?: number;
  rows?: number;
  calls?: number;
  endpoint?: string;
  /** Записи фонового ассистента `agentRunStarted` / `agentRunFinished` (спец. P4c §9.2). */
  runId?: string;
  requestId?: string | null;
  /** Итог запуска (`agentRunFinished`): статус и причина — строки `RunStatus` / `FailReason`. */
  status?: string;
  reason?: string;
  exitCode?: number;
  proposals?: number;
}

/** Фрагменты конфигураций MCP-клиентов (`get_mcp_client_configs`): путь — к установленному приложению (AppImage — `$APPIMAGE`). */
export interface McpClientConfigs {
  command: string;
  args: string[];
  qwenCode: string;
  qwenSettings: string;
  claudeCode: string;
  claudeDesktop: string;
  cursor: string;
}
