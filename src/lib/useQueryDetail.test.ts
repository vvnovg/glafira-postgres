import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppError } from "../api/types";

vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return { ...orig, api: { queryDetail: vi.fn() } };
});

import { api } from "../api/client";
import { useQueryDetail } from "./useQueryDetail";

type Opts = { active: boolean; tickAtMs: number | null };

describe("useQueryDetail", () => {
  function setup(error: AppError, opts: Opts) {
    vi.mocked(api.queryDetail).mockRejectedValue(error);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) => createElement(QueryClientProvider, { client }, children);
    return renderHook((p: Opts) => useQueryDetail("c", "pg-1", "perf", "7", p), { wrapper, initialProps: opts });
  }
  const tick = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));

  beforeEach(() => {
    vi.resetAllMocks();
    vi.useFakeTimers();
  });
  afterEach(() => vi.useRealTimers());

  it("отказ во входе: ни интервал, ни тик, ни возвращение на вкладку карточку не перечитывают", async () => {
    const h = setup({ code: "AUTH_FAILED", message: "password authentication failed", retryable: false }, { active: true, tickAtMs: 60_000 });
    await tick(0);
    const calls = vi.mocked(api.queryDetail).mock.calls.length;
    expect(calls).toBeGreaterThan(0);
    h.rerender({ active: false, tickAtMs: 60_000 });
    h.rerender({ active: true, tickAtMs: 60_000 });
    await tick(0);
    h.rerender({ active: true, tickAtMs: 120_000 });
    await tick(30_000);
    expect(api.queryDetail).toHaveBeenCalledTimes(calls);
  });

  it("временная ошибка: возвращение на вкладку перечитывает карточку по тику", async () => {
    const h = setup({ code: "TIMEOUT", message: "timeout", retryable: true }, { active: false, tickAtMs: 60_000 });
    await tick(0);
    const calls = vi.mocked(api.queryDetail).mock.calls.length;
    h.rerender({ active: true, tickAtMs: 60_000 });
    await tick(0);
    expect(vi.mocked(api.queryDetail).mock.calls.length).toBeGreaterThan(calls);
  });
});
