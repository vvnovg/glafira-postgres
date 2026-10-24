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
export interface Partitioning {
  /** Ключ партиционирования (`RANGE (happened_at)`) — у партиционированной таблицы. */
  key: string | null;
  /** Границы (`FOR VALUES FROM (…) TO (…)`) — у партиции. */
  bound: string | null;
  /** Родитель партиции (`schema.table`); у потомка по `INHERITS` — `null`. */
  parent: string | null;
  /** Прямые партиции (`schema.table`). */
  partitions: string[];
}
export interface Trigger {
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
  triggers: Trigger[];
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
  start: number;
  increment: number;
  min: number;
  max: number;
  cycle: boolean;
  cache: number;
  /** Последнее выданное значение; `null` — последовательность не использовалась или у роли нет права. */
  lastValue: number | null;
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
  errors: LocalizedText[];
}

/** Результат `check_update`. */
export type UpdateCheck =
  | { status: "disabled"; reason: LocalizedText }
  | { status: "upToDate" }
  | { status: "available"; version: string; notes: string | null; date: string | null };

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
