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
  it("команды MCP: имена и аргументы (задача 8)", async () => {
    invoke.mockResolvedValue(undefined);
    await api.getMcpStatus();
    expect(invoke).toHaveBeenLastCalledWith("get_mcp_status", undefined);
    await api.setMcpEnabled(true);
    expect(invoke).toHaveBeenLastCalledWith("set_mcp_enabled", { on: true });
    await api.answerMcpApproval("s1", false);
    expect(invoke).toHaveBeenLastCalledWith("answer_mcp_approval", { sessionId: "s1", approve: false });
    await api.revokeMcpClient("qwen-code");
    expect(invoke).toHaveBeenLastCalledWith("revoke_mcp_client", { name: "qwen-code" });
    await api.disconnectMcpSessions("s2");
    expect(invoke).toHaveBeenLastCalledWith("disconnect_mcp_sessions", { sessionId: "s2" });
    // Без id — все сессии: Option<String> в Rust получает null.
    await api.disconnectMcpSessions();
    expect(invoke).toHaveBeenLastCalledWith("disconnect_mcp_sessions", { sessionId: null });
    await api.getMcpAudit(200);
    expect(invoke).toHaveBeenLastCalledWith("get_mcp_audit", { limit: 200 });
    await api.getMcpClientConfigs();
    expect(invoke).toHaveBeenLastCalledWith("get_mcp_client_configs", undefined);
    await api.revealMcpAudit();
    expect(invoke).toHaveBeenLastCalledWith("reveal_mcp_audit", undefined);
  });

  it("передаёт аргументы в camelCase", async () => {
    invoke.mockResolvedValue({});
    await api.setLanguage("ru");
    expect(invoke).toHaveBeenCalledWith("set_language", { lang: "ru" });
    await api.setLanguage(null);
    expect(invoke).toHaveBeenLastCalledWith("set_language", { lang: null });
  });

  it("команды без аргументов вызываются по имени", async () => {
    invoke.mockResolvedValue({});
    await api.getAppInfo();
    expect(invoke).toHaveBeenLastCalledWith("get_app_info", undefined);
    await api.getPolicyStatus();
    expect(invoke).toHaveBeenLastCalledWith("get_policy_status", undefined);
    await api.getLanguageSettings();
    expect(invoke).toHaveBeenLastCalledWith("get_language_settings", undefined);
  });

  it("команды updater вызываются по имени", async () => {
    invoke.mockResolvedValue({ status: "upToDate" });
    await api.checkUpdate();
    expect(invoke).toHaveBeenLastCalledWith("check_update", undefined);
    await api.installUpdate();
    expect(invoke).toHaveBeenLastCalledWith("install_update", undefined);
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

  it("команды запросов на оптимизацию и предложений LLM", async () => {
    invoke.mockResolvedValue({});
    const req = { member: "pg-1", database: "perf", source: { kind: "console" } as const, sql: "SELECT $1", params: [null], plan: null };
    await api.createOptimizationRequest("c", req);
    expect(invoke).toHaveBeenLastCalledWith("create_optimization_request", { id: "c", req });
    await api.cancelOptimizationRequest("r1");
    expect(invoke).toHaveBeenLastCalledWith("cancel_optimization_request", { requestId: "r1" });
    invoke.mockResolvedValue([]);
    await api.listOptimizationRequests("c");
    expect(invoke).toHaveBeenLastCalledWith("list_optimization_requests", { connectionId: "c" });
    await api.listLlmProposals({ connectionId: "c" });
    expect(invoke).toHaveBeenLastCalledWith("list_llm_proposals", { filter: { connectionId: "c" } });
    invoke.mockResolvedValue({});
    await api.answerLlmProposal("p1", "rejected", "same rows");
    expect(invoke).toHaveBeenLastCalledWith("answer_llm_proposal", { id: "p1", status: "rejected", reason: "same rows" });
    await api.compareLlmProposal("p1");
    expect(invoke).toHaveBeenLastCalledWith("compare_llm_proposal", { id: "p1" });
  });
});
