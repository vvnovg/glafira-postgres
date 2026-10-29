import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppError, StatementsEvent } from "../api/types";

vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return { ...orig, api: { startStatementsSampler: vi.fn(), stopSession: vi.fn() } };
});

import { api } from "../api/client";
import { useConnectionEpochs } from "../state/connectionEpoch";
import { RESUBSCRIBE_DELAY_MS } from "./useSampler";
import { useStatementsSampler } from "./useStatementsSampler";

const authFailed: AppError = { code: "AUTH_FAILED", message: "password authentication failed", retryable: false };
const noExtension: AppError = { code: "FEATURE_UNAVAILABLE", message: "pg_stat_statements is not installed", retryable: false };

describe("useStatementsSampler", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    useConnectionEpochs.setState({ epochs: {} });
  });

  it("без узла подписывается «за лидером», показывает тик и отписывается при размонтировании; выключенная подписка не стартует", async () => {
    let emit: (e: StatementsEvent) => void = () => {};
    vi.mocked(api.startStatementsSampler).mockImplementation(async (_id, _m, onEvent) => {
      emit = onEvent;
      return "s-1";
    });
    vi.mocked(api.stopSession).mockResolvedValue(true);
    const off = renderHook(() => useStatementsSampler("c", null, false));
    expect(api.startStatementsSampler).not.toHaveBeenCalled();
    off.unmount();
    const { result, unmount } = renderHook(() => useStatementsSampler("c", null, true));
    await waitFor(() => expect(api.startStatementsSampler).toHaveBeenCalledWith("c", null, expect.any(Function)));
    expect(result.current.live).toBeNull();
    act(() => emit({ kind: "tick", atMs: 5, coverageS: 120, ticks: 2, changed: 4 }));
    expect(result.current.live).toEqual({ atMs: 5, coverageS: 120, ticks: 2 });
    unmount();
    await waitFor(() => expect(api.stopSession).toHaveBeenCalledWith("s-1"));
  });

  it("ошибка сэмплера показывается, следующий тик её снимает; отказ самой подписки — ошибка хука", async () => {
    let emit: (e: StatementsEvent) => void = () => {};
    vi.mocked(api.startStatementsSampler).mockImplementationOnce(async (_id, _m, onEvent) => {
      emit = onEvent;
      return "s-1";
    });
    vi.mocked(api.stopSession).mockResolvedValue(true);
    const { result } = renderHook(() => useStatementsSampler("c", "pg-1", true));
    await waitFor(() => expect(api.startStatementsSampler).toHaveBeenCalled());
    act(() => emit({ kind: "error", error: authFailed }));
    expect(result.current.error).toEqual(authFailed);
    act(() => emit({ kind: "tick", atMs: 1, coverageS: 60, ticks: 1, changed: 0 }));
    expect(result.current.error).toBeNull();

    vi.mocked(api.startStatementsSampler).mockRejectedValueOnce(noExtension);
    const second = renderHook(() => useStatementsSampler("c", "pg-2", true));
    await waitFor(() => expect(second.result.current.error).toEqual(noExtension));
  });
});

describe("useStatementsSampler: повторная подписка после stopped", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.useFakeTimers();
    useConnectionEpochs.setState({ epochs: {} });
  });
  afterEach(() => vi.useRealTimers());

  const advance = (ms: number) =>
    act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });

  function channels() {
    const emits: ((e: StatementsEvent) => void)[] = [];
    vi.mocked(api.startStatementsSampler).mockImplementation(async (_id, _m, onEvent) => {
      emits.push(onEvent);
      return `s-${emits.length}`;
    });
    vi.mocked(api.stopSession).mockResolvedValue(true);
    return emits;
  }

  it("stopped не из-за входа: через паузу подписка заново, старая отписана, поздние события старого канала игнорируются", async () => {
    const emits = channels();
    const { result } = renderHook(() => useStatementsSampler("c", "pg-1", true));
    await advance(0);
    act(() => emits[0]({ kind: "stopped" }));
    await advance(RESUBSCRIBE_DELAY_MS - 1);
    expect(api.startStatementsSampler).toHaveBeenCalledTimes(1);
    await advance(1);
    expect(api.startStatementsSampler).toHaveBeenCalledTimes(2);
    expect(api.stopSession).toHaveBeenCalledWith("s-1");
    act(() => emits[0]({ kind: "tick", atMs: 9, coverageS: 60, ticks: 1, changed: 1 }));
    expect(result.current.live).toBeNull();
    act(() => emits[1]({ kind: "tick", atMs: 10, coverageS: 120, ticks: 2, changed: 1 }));
    expect(result.current.live).toEqual({ atMs: 10, coverageS: 120, ticks: 2 });
  });

  it.each([
    ["отказ во входе", authFailed],
    ["расширение исчезло или слишком старое", noExtension],
  ])("stopped после ошибки «%s»: подписки заново нет, ошибка остаётся; новый номер подключения подписывает снова", async (_name, error) => {
    const emits = channels();
    const { result } = renderHook(() => useStatementsSampler("c", "pg-1", true));
    await advance(0);
    act(() => emits[0]({ kind: "error", error }));
    act(() => emits[0]({ kind: "stopped" }));
    await advance(10 * RESUBSCRIBE_DELAY_MS);
    expect(api.startStatementsSampler).toHaveBeenCalledTimes(1);
    expect(result.current.error).toEqual(error);
    act(() => useConnectionEpochs.getState().bump("c"));
    await advance(0);
    expect(api.startStatementsSampler).toHaveBeenCalledTimes(2);
  });
});
