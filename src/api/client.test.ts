import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { api, toAppError } from "./client";

beforeEach(() => {
  invoke.mockReset();
});

describe("api client", () => {
  it("передаёт аргументы в camelCase", async () => {
    invoke.mockResolvedValue({});
    await api.setLanguage("ru");
    expect(invoke).toHaveBeenCalledWith("set_language", { lang: "ru" });
    await api.setLanguage(null);
    expect(invoke).toHaveBeenLastCalledWith("set_language", { lang: null });
  });

  it("setLicenseKey передаёт ключ", async () => {
    invoke.mockResolvedValue({ tier: "pro" });
    await api.setLicenseKey("a.b");
    expect(invoke).toHaveBeenCalledWith("set_license_key", { key: "a.b" });
  });

  it("команды без аргументов вызываются по имени", async () => {
    invoke.mockResolvedValue({});
    await api.getAppInfo();
    expect(invoke).toHaveBeenLastCalledWith("get_app_info", undefined);
    await api.getPolicyStatus();
    expect(invoke).toHaveBeenLastCalledWith("get_policy_status", undefined);
    await api.getLicenseStatus();
    expect(invoke).toHaveBeenLastCalledWith("get_license_status", undefined);
    await api.removeLicenseKey();
    expect(invoke).toHaveBeenLastCalledWith("remove_license_key", undefined);
    await api.getLanguageSettings();
    expect(invoke).toHaveBeenLastCalledWith("get_language_settings", undefined);
  });

  it("пробрасывает AppError как есть", async () => {
    const err = { code: "NOT_FOUND", message: "нет", retryable: false };
    invoke.mockRejectedValue(err);
    await expect(api.getAppInfo()).rejects.toEqual(err);
  });

  it("строковые ошибки превращает в INTERNAL", async () => {
    invoke.mockRejectedValue("boom");
    await expect(api.getAppInfo()).rejects.toEqual({ code: "INTERNAL", message: "boom", retryable: false });
  });

  it("toAppError сохраняет key и params ошибки из Rust", () => {
    const e = { code: "NOT_FOUND", key: "errors.notFound", params: { name: "p" }, message: "“p” not found", retryable: false };
    expect(toAppError(e)).toEqual(e);
  });

  it("toAppError понимает Error", () => {
    expect(toAppError(new Error("x"))).toEqual({ code: "INTERNAL", message: "x", retryable: false });
  });
});
