import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { emitEvent, listeners, resetEventBus } from "../test/events";
import { llmProposal, optimizationRequest } from "../test/llm";

vi.mock("@tauri-apps/api/event", async () => (await import("../test/events")).tauriEventMock());
vi.mock("../api/client", () => ({ api: { listOptimizationRequests: vi.fn(), listLlmProposals: vi.fn() } }));

import { api } from "../api/client";
import { byNewest, consoleProposals, LLM_PROPOSALS_EVENT, queryProposals, sourceMatches, useLlmProposals, useOptimizationRequests, waitingRequest } from "./useLlmProposals";

const emit = (payload: unknown) => emitEvent(LLM_PROPOSALS_EVENT, payload);

function wrapper() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return ({ children }: { children: ReactNode }) => createElement(QueryClientProvider, { client: qc }, children);
}

describe("отбор запросов и предложений", () => {
  const console1 = optimizationRequest({ id: "r1", createdAtMs: 1 });
  const console2 = optimizationRequest({ id: "r2", createdAtMs: 2 });
  const card = optimizationRequest({ id: "r3", source: { kind: "query", queryId: "7" }, createdAtMs: 3 });
  const cardOtherDb = optimizationRequest({ id: "r4", source: { kind: "query", queryId: "7" }, database: "postgres", createdAtMs: 4 });

  it("ожидающий запрос — последний своего источника; карточка — тот же queryid в той же базе", () => {
    const reqs = [console1, console2, card, cardOtherDb];
    expect(waitingRequest(reqs, { kind: "console" }, "perf")?.id).toBe("r2");
    expect(waitingRequest(reqs, { kind: "query", queryId: "7" }, "perf")?.id).toBe("r3");
    expect(waitingRequest(reqs, { kind: "query", queryId: "8" }, "perf")).toBeNull();
    expect(waitingRequest([{ ...console2, status: "answered" }, console1], { kind: "console" }, "perf")?.id).toBe("r1");
    expect(sourceMatches(cardOtherDb, { kind: "query", queryId: "7" }, "perf")).toBe(false);
    expect(sourceMatches(console1, { kind: "console" }, "postgres")).toBe(true);
  });

  it("консоль: предложения её запросов, без запроса — по базе, закреплённые — всегда; новые первыми", () => {
    const own = llmProposal({ id: "p1", requestId: "r1", status: "accepted", createdAtMs: 30 });
    const free = llmProposal({ id: "p2", requestId: null, database: "perf", createdAtMs: 10 });
    const freeOther = llmProposal({ id: "p3", requestId: null, database: "postgres" });
    const fromCard = llmProposal({ id: "p4", requestId: "r3" });
    const all = [own, free, freeOther, fromCard];
    expect(consoleProposals(all, [console1, card], "perf", []).map((p) => p.id)).toEqual(["p2", "p1"]);
    expect(consoleProposals(all, [console1, card], "perf", ["p4"]).map((p) => p.id)).toEqual(["p4", "p2", "p1"]);
    expect(consoleProposals(all, [console1, card], null, []).map((p) => p.id)).toEqual(["p1"]);
  });

  it("карточка: по originalQueryId и по запросам из карточки в той же базе", () => {
    const byQuery = llmProposal({ id: "p1", requestId: null, originalQueryId: "7" });
    const byRequest = llmProposal({ id: "p2", requestId: "r3", originalQueryId: null });
    const otherDb = llmProposal({ id: "p3", requestId: null, originalQueryId: "7", database: "postgres" });
    const fromConsole = llmProposal({ id: "p4", requestId: "r1" });
    const ids = queryProposals([byQuery, byRequest, otherDb, fromConsole], [console1, card], "perf", "7").map((p) => p.id);
    expect(ids.sort()).toEqual(["p1", "p2"]);
  });

  it("порядок: новые (без ответа) первыми, внутри — свежие первыми", () => {
    const a = llmProposal({ id: "a", status: "rejected", createdAtMs: 50 });
    const b = llmProposal({ id: "b", createdAtMs: 10 });
    const c = llmProposal({ id: "c", createdAtMs: 20 });
    expect([a, b, c].sort(byNewest).map((p) => p.id)).toEqual(["c", "b", "a"]);
  });
});

describe("useLlmProposals", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    resetEventBus();
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    vi.mocked(api.listLlmProposals).mockResolvedValue([llmProposal()]);
  });

  it("читает запросы подключения и предложения по фильтру", async () => {
    const { result } = renderHook(() => useLlmProposals("c", { connectionId: "c" }), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.proposals).toHaveLength(1));
    expect(result.current.requests.map((r) => r.id)).toEqual(["r3a9f1c"]);
    expect(api.listOptimizationRequests).toHaveBeenCalledWith("c");
    expect(api.listLlmProposals).toHaveBeenCalledWith({ connectionId: "c" });
  });

  it("событие своего подключения перечитывает оба списка, чужого — нет; размонтирование отписывает", async () => {
    const { result, unmount } = renderHook(() => useLlmProposals("c", { connectionId: "c" }), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.proposals).toHaveLength(1));
    await waitFor(() => expect(listeners(LLM_PROPOSALS_EVENT)).toHaveLength(1));
    emit({ kind: "proposal", connectionId: "other", proposalId: "p9" });
    expect(api.listLlmProposals).toHaveBeenCalledTimes(1);
    vi.mocked(api.listLlmProposals).mockResolvedValue([llmProposal(), llmProposal({ id: "p2" })]);
    emit({ kind: "proposal", connectionId: "c", proposalId: "p2" });
    await waitFor(() => expect(result.current.proposals).toHaveLength(2));
    expect(api.listOptimizationRequests).toHaveBeenCalledTimes(2);
    unmount();
    expect(listeners(LLM_PROPOSALS_EVENT)).toHaveLength(0);
  });

  it("refresh перечитывает оба списка", async () => {
    const { result } = renderHook(() => useLlmProposals("c", { connectionId: "c" }), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.requests).toHaveLength(1));
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest({ status: "cancelled" })]);
    act(() => result.current.refresh());
    await waitFor(() => expect(result.current.requests[0].status).toBe("cancelled"));
    expect(api.listLlmProposals).toHaveBeenCalledTimes(2);
  });
});
describe("useOptimizationRequests (лёгкое чтение для кнопки)", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    resetEventBus();
  });

  it("читает только запросы, предложения не трогает; состояние ready после чтения", async () => {
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    const { result } = renderHook(() => useOptimizationRequests("c"), { wrapper: wrapper() });
    expect(result.current.state).toBe("loading");
    await waitFor(() => expect(result.current.state).toBe("ready"));
    expect(result.current.requests).toHaveLength(1);
    expect(api.listLlmProposals).not.toHaveBeenCalled();
  });

  it("неудачное первое чтение — failed (не «ничего не ждёт»); retry перечитывает", async () => {
    vi.mocked(api.listOptimizationRequests).mockRejectedValueOnce({ code: "INTERNAL", key: "errors.internal", message: "x", retryable: true });
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([]);
    const { result } = renderHook(() => useOptimizationRequests("c"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.state).toBe("failed"));
    act(() => result.current.retry());
    await waitFor(() => expect(result.current.state).toBe("ready"));
  });

  it("событие своего подключения перечитывает запросы", async () => {
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([]);
    const { result } = renderHook(() => useOptimizationRequests("c"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.state).toBe("ready"));
    await waitFor(() => expect(listeners(LLM_PROPOSALS_EVENT)).toHaveLength(1));
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    emit({ kind: "request", connectionId: "c", requestId: "r3a9f1c", proposalId: null });
    await waitFor(() => expect(result.current.requests).toHaveLength(1));
  });
});
