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
  folderId?: string | null;
  order?: number;
}
/** У каждого поля: null/нет — оставить сохранённый, "" — удалить, иначе — новый. */
export interface SecretsInput {
  pgPassword?: string | null;
  pgKeyPassword?: string | null;
  patroniPassword?: string | null;
  patroniKeyPassword?: string | null;
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

export type TestStep = "patroni" | "discovery" | "node" | "readiness";
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
