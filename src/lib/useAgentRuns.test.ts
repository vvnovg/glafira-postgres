import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentRun } from "../api/types";
import { emitEvent, listeners, resetEventBus } from "../test/events";
import { agentRun } from "../test/llm";

vi.mock("@tauri-apps/api/event", async () => (await import("../test/events")).tauriEventMock());
vi.mock("../api/client", () => ({ api: { listAgentRuns: vi.fn() } }));

import { api } from "../api/client";
import { AGENT_RUNS_EVENT, elapsedText, latestRunFor, queueAhead, upsertRun, useAgentRuns } from "./useAgentRuns";

function wrapper() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return ({ children }: { children: ReactNode }) => createElement(QueryClientProvider, { client: qc }, children);
}

describe("запуски: чистые функции", () => {
  it("последний запуск запроса — по времени постановки в очередь", () => {
    const old = agentRun({ id: "a1", status: "failed", queuedAtMs: 1 });
    const fresh = agentRun({ id: "a2", queuedAtMs: 2 });
    const other = agentRun({ id: "a3", requestId: "r9", queuedAtMs: 3 });
    expect(latestRunFor([old, fresh, other], "r3a9f1c")?.id).toBe("a2");
    expect(latestRunFor([other], "r3a9f1c")).toBeNull();
  });

  it("перед запуском в очереди — идущий и поставленные раньше, в любых подключениях", () => {
    const running = agentRun({ id: "a1", connectionId: "other", status: "running", queuedAtMs: 1 });
    const earlier = agentRun({ id: "a2", connectionId: "other", queuedAtMs: 2 });
    const mine = agentRun({ id: "a3", queuedAtMs: 3 });
    const later = agentRun({ id: "a4", queuedAtMs: 4 });
    const done = agentRun({ id: "a0", status: "done", queuedAtMs: 0 });
    expect(queueAhead([later, mine, earlier, running, done], mine)).toBe(2);
    expect(queueAhead([mine], mine)).toBe(0);
  });

  it("событие заменяет запуск по id или добавляет новый; новые первыми", () => {
    const a = agentRun({ id: "a1", queuedAtMs: 1 });
    const b = agentRun({ id: "a2", queuedAtMs: 2 });
    expect(upsertRun([b, a], { ...a, status: "running" }).map((r) => [r.id, r.status])).toEqual([
      ["a2", "queued"],
      ["a1", "running"],
    ]);
    expect(upsertRun([a], b).map((r) => r.id)).toEqual(["a2", "a1"]);
  });

  it("время работы — «м:сс»", () => {
    expect(elapsedText(83_400)).toBe("1:23");
    expect(elapsedText(5_000)).toBe("0:05");
    expect(elapsedText(-10)).toBe("0:00");
    expect(elapsedText(61 * 60_000)).toBe("61:00");
  });
});

describe("useAgentRuns", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    resetEventBus();
  });

  it("читает все запуски один раз, отдаёт запуски подключения; событие обновляет запуск на месте", async () => {
    vi.mocked(api.listAgentRuns).mockResolvedValue([agentRun({ id: "a1" }), agentRun({ id: "a2", connectionId: "other", requestId: "r9", queuedAtMs: 0 })]);
    const { result } = renderHook(() => useAgentRuns("c"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.state).toBe("ready"));
    expect(api.listAgentRuns).toHaveBeenCalledWith({});
    expect(result.current.runs.map((r) => r.id)).toEqual(["a1"]);
    expect(result.current.all).toHaveLength(2);
    await waitFor(() => expect(listeners(AGENT_RUNS_EVENT)).toHaveLength(1));
    emitEvent(AGENT_RUNS_EVENT, agentRun({ id: "a1", status: "running", calls: 3 }));
    await waitFor(() => expect(result.current.runs[0]).toMatchObject({ id: "a1", status: "running", calls: 3 }));
    // Новый запуск другого подключения — в общем списке, но не в списке подключения.
    emitEvent(AGENT_RUNS_EVENT, agentRun({ id: "a3", connectionId: "other", queuedAtMs: 5 }));
    await waitFor(() => expect(result.current.all.map((r) => r.id)).toEqual(["a1", "a3", "a2"]));
    expect(result.current.runs.map((r) => r.id)).toEqual(["a1"]);
    expect(api.listAgentRuns).toHaveBeenCalledTimes(1);
  });

  it("null — все запуски, в том числе «Проверку» без подключения", async () => {
    vi.mocked(api.listAgentRuns).mockResolvedValue([agentRun({ id: "a1" }), agentRun({ id: "a2", kind: "check", connectionId: null, requestId: null, queuedAtMs: 5 })]);
    const { result } = renderHook(() => useAgentRuns(null), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.state).toBe("ready"));
    expect(result.current.runs.map((r) => r.id)).toEqual(["a1", "a2"]);
  });

  it("событие до первого ответа перечитывает список (запоздавший ответ не затирает событие); размонтирование отписывает", async () => {
    let resolveFirst: (v: AgentRun[]) => void = () => {};
    vi.mocked(api.listAgentRuns)
      .mockReturnValueOnce(new Promise<AgentRun[]>((r) => (resolveFirst = r)))
      .mockResolvedValue([agentRun({ status: "running" })]);
    const { result, unmount } = renderHook(() => useAgentRuns("c"), { wrapper: wrapper() });
    await waitFor(() => expect(listeners(AGENT_RUNS_EVENT)).toHaveLength(1));
    emitEvent(AGENT_RUNS_EVENT, agentRun({ status: "running" }));
    resolveFirst([agentRun()]);
    await waitFor(() => expect(result.current.runs[0]?.status).toBe("running"));
    expect(api.listAgentRuns).toHaveBeenCalledTimes(2);
    unmount();
    expect(listeners(AGENT_RUNS_EVENT)).toHaveLength(0);
  });

  it("не прочиталось — «failed»; refresh перечитывает", async () => {
    vi.mocked(api.listAgentRuns).mockRejectedValueOnce({ code: "INTERNAL", message: "disk", retryable: true }).mockResolvedValue([agentRun()]);
    const { result } = renderHook(() => useAgentRuns("c"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.state).toBe("failed"));
    result.current.refresh();
    await waitFor(() => expect(result.current.state).toBe("ready"));
    expect(result.current.runs).toHaveLength(1);
  });

  it("enabled: false — ни чтения, ни подписки", () => {
    const { result } = renderHook(() => useAgentRuns("c", { enabled: false }), { wrapper: wrapper() });
    expect(result.current.state).toBe("loading");
    expect(result.current.runs).toEqual([]);
    expect(api.listAgentRuns).not.toHaveBeenCalled();
    expect(listeners(AGENT_RUNS_EVENT)).toHaveLength(0);
  });
});
