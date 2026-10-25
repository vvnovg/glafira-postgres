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

// ---------------------------------------------------------------------------------------------------------------
// Диагностика (P2a): зеркало JSON отчётов Rust (`diag::DiagReport`, `pg_advisor::*`), все поля camelCase.
// Длительности — секунды (`…S`) от `now()` сервера, размеры — байты (`…Bytes`), доли — проценты (`…Pct`), если не сказано иное.
// ---------------------------------------------------------------------------------------------------------------

/** Вкладка инструмента. `idleTx` — вкладка фронтенда поверх `sessions`, в Rust отдельного инструмента нет. */
export type DiagTool = "pulse" | "sessions" | "idleTx" | "waits" | "locks" | "horizon" | "replication" | "slots" | "failover" | "health";
/** Инструмент Rust (`diag::Tool`). */
export type RustTool = Exclude<DiagTool, "idleTx">;
export type Severity = "info" | "warning" | "critical";
export type FindingTarget =
  | { kind: "session"; pid: number; backendStartUs: number }
  | { kind: "slot"; name: string }
  | { kind: "member"; name: string }
  | { kind: "relation"; name: string }
  | { kind: "database"; name: string };
/** Действие с сессиями, которое предлагает находка: `idleInTransaction` и `rootBlocker`. */
export type SessionActionLink =
  | { type: "terminateIdleInTransaction"; olderThanS: number }
  | { type: "terminateRootBlocker"; pid: number; backendStartUs: number };
/** Куда ведёт кнопка находки; `tool` — id вкладки (`idleTx` допустим), `action` — действие P2b (кнопки — задача 11). */
export type FindingLink =
  | { kind: "openTool"; tool: DiagTool; member: string | null }
  | { kind: "action"; action: SessionActionLink; member: string | null };
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
  | ({ kind: "health" } & HealthData);
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
  /** `cancel` | `terminate` | `dropSlot` | `switchover` | `cancelSwitchover` | `failover` | `restart` | `cancelRestart` | `pause` | `resume`. */
  action: string;
  connection: { id: string; name: string; environment: Environment };
  member: string | null;
  prodConfirmed: boolean;
  outcome: ActionOutcome;
  /** `AppError.code` при `outcome: "error"`. */
  code: string | null;
  ms: number;
  /** Пустые `sessions`, `slot` и `patroni` в журнал не пишутся: поля отсутствуют. */
  sessions?: SessionRecord[];
  slot?: SlotRecord;
  patroni?: PatroniRecord;
}
