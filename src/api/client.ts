import { invoke } from "@tauri-apps/api/core";
import type { AppError, AppInfo, LanguageSettings, LicenseStatus, PolicyStatus } from "./types";

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
};
