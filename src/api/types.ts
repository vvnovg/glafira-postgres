export type ErrorCode =
  | "CONNECTION_FAILED"
  | "TLS_ERROR"
  | "AUTH_FAILED"
  | "NOT_AUTHORIZED"
  | "TIMEOUT"
  | "INVALID_INPUT"
  | "NOT_FOUND"
  | "PRO_REQUIRED"
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

export type Tier = "free" | "pro";
export type LicenseState = "none" | "valid" | "expiring" | "expired" | "invalid";
export interface LicenseStatus {
  tier: Tier;
  state: LicenseState;
  source: "policy" | "user" | null;
  licensee: string | null;
  kind: "personal" | "organization" | null;
  expires: string | null;
  daysLeft: number | null;
  /** Почему ключ недействителен — полная фраза. */
  error: LocalizedText | null;
}
export interface PolicyStatus {
  managed: boolean;
  source: string | null;
  updatesEnabled: boolean;
  updateUrl: string | null;
  hasLicenseKey: boolean;
  language: string | null;
  errors: LocalizedText[];
}

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
