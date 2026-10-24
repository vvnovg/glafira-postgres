import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { diagReport } from "../test/fixtures";

vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return { ...orig, api: { runDiagnostic: vi.fn() } };
});

import { api } from "../api/client";
import { defaultIntervalS, DIAG_INTERVALS_S, diagRefetchInterval, isClusterTool, rustTool, useDiagnostic } from "./useDiagnostic";

describe("опрос инструмента", () => {
  it("скрытая вкладка, пауза и отказ во входе останавливают опрос", () => {
    expect(diagRefetchInterval(true, 5, null)).toBe(5000);
    expect(diagRefetchInterval(false, 5, null)).toBe(false);
    expect(diagRefetchInterval(true, null, null)).toBe(false);
    expect(diagRefetchInterval(true, 5, { code: "AUTH_FAILED", message: "x", retryable: false })).toBe(false);
    expect(diagRefetchInterval(true, 5, { code: "TIMEOUT", message: "x", retryable: true })).toBe(5000);
  });

  it("idleTx — инструмент sessions; Patroni опрашивается реже", () => {
    expect(rustTool("idleTx")).toBe("sessions");
    expect(rustTool("locks")).toBe("locks");
    expect([defaultIntervalS("health"), defaultIntervalS("failover"), defaultIntervalS("sessions")]).toEqual([10, 10, 5]);
  });

  it("инструменты кластера — pulse, failover, health", () => {
    expect((["pulse", "sessions", "idleTx", "waits", "locks", "horizon", "replication", "slots", "failover", "health"] as const).filter(isClusterTool)).toEqual([
      "pulse",
      "failover",
      "health",
    ]);
  });

  it("интервалы выбора — 2, 5, 10 и 30 секунд; по умолчанию каждый из них доступен в списке", () => {
    expect([...DIAG_INTERVALS_S]).toEqual([2, 5, 10, 30]);
    expect(DIAG_INTERVALS_S).toContain(defaultIntervalS("pulse"));
    expect(DIAG_INTERVALS_S).toContain(defaultIntervalS("health"));
  });
});

describe("useDiagnostic: опрос по таймеру", () => {
  const report = diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] });

  function setup(opts: { active: boolean; intervalS: number | null; live?: boolean; windowS?: number }) {
    vi.mocked(api.runDiagnostic).mockResolvedValue(report);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) => createElement(QueryClientProvider, { client }, children);
    return { client, ...renderHook((p: typeof opts) => useDiagnostic("c", "idleTx", "pg-2", { live: true, ...p }), { wrapper, initialProps: opts }) };
  }

  /** Часы вперёд внутри act: обновления состояния запросов попадают в React без предупреждений. */
  const tick = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));

  beforeEach(() => {
    vi.resetAllMocks();
    vi.useFakeTimers();
  });
  afterEach(() => vi.useRealTimers());

  it("видимая вкладка опрашивает по интервалу; idleTx идёт как sessions, ключ кэша — по узлу и окну", async () => {
    const { client } = setup({ active: true, intervalS: 5, windowS: 900 });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "sessions", member: "pg-2", windowS: 900 });
    expect(client.getQueryData(["diag", "c", "sessions", "pg-2", 900])).toEqual(report);
    await tick(5000);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(2);
  });

  it("скрытая вкладка и пауза не опрашивают; подключение закрыто — запроса нет", async () => {
    const hidden = setup({ active: false, intervalS: 5 });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);
    await tick(20_000);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);
    hidden.unmount();

    vi.mocked(api.runDiagnostic).mockClear();
    setup({ active: true, intervalS: null });
    await tick(20_000);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);

    vi.mocked(api.runDiagnostic).mockClear();
    setup({ active: true, intervalS: 5, live: false });
    await tick(20_000);
    expect(api.runDiagnostic).not.toHaveBeenCalled();
  });

  it("вкладка, снова ставшая видимой, сразу перечитывает данные старше интервала", async () => {
    const h = setup({ active: false, intervalS: 5 });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);
    // Скрытая вкладка не опрашивает: данные стареют.
    await tick(20_000);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);
    h.rerender({ active: true, intervalS: 5 });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(2);
    // Дальше — обычный опрос по интервалу, без лишних запросов.
    await tick(5000);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(3);
  });

  it("свежие данные при показе не перечитываются; пауза и закрытое подключение — тоже", async () => {
    const fresh = setup({ active: false, intervalS: 5 });
    await tick(0);
    await tick(3_000);
    fresh.rerender({ active: true, intervalS: 5 });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);
    fresh.unmount();

    vi.mocked(api.runDiagnostic).mockClear();
    const paused = setup({ active: false, intervalS: null });
    await tick(0);
    await tick(60_000);
    paused.rerender({ active: true, intervalS: null });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);
    paused.unmount();

    vi.mocked(api.runDiagnostic).mockClear();
    const closed = setup({ active: false, intervalS: 5, live: false });
    await tick(60_000);
    closed.rerender({ active: true, intervalS: 5, live: false });
    await tick(0);
    expect(api.runDiagnostic).not.toHaveBeenCalled();
  });

  it("после отказа во входе опрос останавливается", async () => {
    vi.mocked(api.runDiagnostic).mockRejectedValue({ code: "AUTH_FAILED", message: "x", retryable: false });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) => createElement(QueryClientProvider, { client }, children);
    renderHook(() => useDiagnostic("c", "locks", null, { active: true, intervalS: 2, live: true }), { wrapper });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);
    await tick(20_000);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);
  });

  it("после отказа во входе показ вкладки не повторяет вход, даже если данные устарели", async () => {
    const h = setup({ active: true, intervalS: 5 });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);
    // Следующий опрос получает отказ во входе; отчёт первого опроса остаётся в кэше.
    vi.mocked(api.runDiagnostic).mockRejectedValue({ code: "AUTH_FAILED", message: "x", retryable: false });
    await tick(5000);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(2);
    expect(h.client.getQueryState(["diag", "c", "sessions", "pg-2", 0])?.error).toMatchObject({ code: "AUTH_FAILED" });
    expect(h.client.getQueryData(["diag", "c", "sessions", "pg-2", 0])).toEqual(report);
    h.rerender({ active: false, intervalS: 5 });
    await tick(20_000);
    h.rerender({ active: true, intervalS: 5 });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(2);
  });
});
