import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { QueriesParams } from "../api/types";
import { diagReport } from "../test/fixtures";

vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return { ...orig, api: { runDiagnostic: vi.fn() } };
});

import { api } from "../api/client";
import { useQueriesReport } from "./useQueriesReport";

const sinceReset: QueriesParams = { mode: { kind: "sinceReset" }, sort: "totalTime", limit: 50, database: null, toplevelOnly: true };
const window5: QueriesParams = { ...sinceReset, mode: { kind: "window", s: 300 } };

type Opts = { params: QueriesParams; active: boolean; intervalS: number | null; tickAtMs: number | null };

describe("useQueriesReport", () => {
  function setup(opts: Opts) {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "queries", mode: opts.params.mode, coverageS: null, ticks: null, pgss: null, totals: { calls: 0, totalExecMs: 0, rows: 0 }, rows: [] }));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) => createElement(QueryClientProvider, { client }, children);
    return renderHook((p: Opts) => useQueriesReport("c", "pg-1", p.params, { active: p.active, live: true, intervalS: p.intervalS, tickAtMs: p.tickAtMs }), { wrapper, initialProps: opts });
  }
  const tick = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));

  beforeEach(() => {
    vi.resetAllMocks();
    vi.useFakeTimers();
  });
  afterEach(() => vi.useRealTimers());

  it("окно обновляется по тику сэмплера; «Пауза» останавливает и тики, снятие паузы догоняет", async () => {
    const h = setup({ params: window5, active: true, intervalS: 10, tickAtMs: null });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);
    h.rerender({ params: window5, active: true, intervalS: 10, tickAtMs: 60_000 });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(2);
    // Пауза: новый тик отчёт не перечитывает, интервала в окне нет вовсе.
    h.rerender({ params: window5, active: true, intervalS: null, tickAtMs: 60_000 });
    h.rerender({ params: window5, active: true, intervalS: null, tickAtMs: 120_000 });
    await tick(30_000);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(2);
    h.rerender({ params: window5, active: true, intervalS: 10, tickAtMs: 120_000 });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(3);
  });

  it("«с момента сброса»: вкладка, снова ставшая видимой, перечитывает данные старше интервала; на паузе — нет", async () => {
    const h = setup({ params: sinceReset, active: false, intervalS: 10, tickAtMs: null });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);
    await tick(30_000);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(1);
    h.rerender({ params: sinceReset, active: true, intervalS: 10, tickAtMs: null });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(2);
    // Свежие данные при новом показе не перечитываются.
    h.rerender({ params: sinceReset, active: false, intervalS: 10, tickAtMs: null });
    await tick(3_000);
    h.rerender({ params: sinceReset, active: true, intervalS: 10, tickAtMs: null });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(2);
    // Пауза: показ после долгого скрытия ничего не читает.
    h.rerender({ params: sinceReset, active: false, intervalS: null, tickAtMs: null });
    await tick(60_000);
    h.rerender({ params: sinceReset, active: true, intervalS: null, tickAtMs: null });
    await tick(0);
    expect(api.runDiagnostic).toHaveBeenCalledTimes(2);
  });
});
