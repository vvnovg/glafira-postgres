import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppError, SamplerEvent } from "../api/types";

vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return { ...orig, api: { startSampler: vi.fn(), stopSession: vi.fn() } };
});

import { api } from "../api/client";
import { useSampler } from "./useSampler";

const authFailed: AppError = { code: "AUTH_FAILED", message: "password authentication failed", retryable: false };

describe("useSampler", () => {
  beforeEach(() => vi.resetAllMocks());

  it("подписывается на узел, показывает тики и отписывается при смене узла и размонтировании", async () => {
    let emit: (e: SamplerEvent) => void = () => {};
    vi.mocked(api.startSampler).mockImplementation(async (_id, _m, onEvent) => {
      emit = onEvent;
      return `s-${vi.mocked(api.startSampler).mock.calls.length}`;
    });
    vi.mocked(api.stopSession).mockResolvedValue(true);
    const { result, rerender, unmount } = renderHook(({ member }) => useSampler("c", member, true), { initialProps: { member: "pg-1" as string | null } });
    await waitFor(() => expect(api.startSampler).toHaveBeenCalledWith("c", "pg-1", expect.any(Function)));
    act(() => emit({ kind: "tick", atMs: 5, active: 3, waiting: 1 }));
    expect(result.current.live).toEqual({ atMs: 5, active: 3, waiting: 1 });
    rerender({ member: "pg-2" });
    await waitFor(() => expect(api.stopSession).toHaveBeenCalledWith("s-1"));
    await waitFor(() => expect(api.startSampler).toHaveBeenLastCalledWith("c", "pg-2", expect.any(Function)));
    unmount();
    await waitFor(() => expect(api.stopSession).toHaveBeenLastCalledWith("s-2"));
  });

  it("без узла не подписывается; ответ, пришедший после размонтирования, сразу отписывается", async () => {
    let resolve: (id: string) => void = () => {};
    vi.mocked(api.startSampler).mockImplementation(() => new Promise((r) => (resolve = r)));
    const { rerender, unmount } = renderHook(({ member }) => useSampler("c", member, true), { initialProps: { member: null as string | null } });
    expect(api.startSampler).not.toHaveBeenCalled();
    rerender({ member: "pg-1" });
    unmount();
    resolve("late");
    await waitFor(() => expect(api.stopSession).toHaveBeenCalledWith("late"));
  });

  it("выключенная подписка не стартует; выключение снимает действующую", async () => {
    vi.mocked(api.startSampler).mockResolvedValue("s-1");
    vi.mocked(api.stopSession).mockResolvedValue(true);
    const { rerender } = renderHook(({ enabled }) => useSampler("c", "pg-1", enabled), { initialProps: { enabled: false } });
    expect(api.startSampler).not.toHaveBeenCalled();
    rerender({ enabled: true });
    await waitFor(() => expect(api.startSampler).toHaveBeenCalledTimes(1));
    rerender({ enabled: false });
    await waitFor(() => expect(api.stopSession).toHaveBeenCalledWith("s-1"));
  });

  it("смена узла сбрасывает показания прежнего; поздний тик отписанного узла не возвращает их", async () => {
    const emits: ((e: SamplerEvent) => void)[] = [];
    vi.mocked(api.startSampler).mockImplementation(async (_id, _m, onEvent) => {
      emits.push(onEvent);
      return `s-${emits.length}`;
    });
    vi.mocked(api.stopSession).mockResolvedValue(true);
    const { result, rerender } = renderHook(({ member }) => useSampler("c", member, true), { initialProps: { member: "pg-1" } });
    await waitFor(() => expect(emits).toHaveLength(1));
    act(() => emits[0]({ kind: "tick", atMs: 1, active: 2, waiting: 0 }));
    expect(result.current.live).not.toBeNull();
    rerender({ member: "pg-2" });
    await waitFor(() => expect(emits).toHaveLength(2));
    expect(result.current.live).toBeNull();
    // Канал старой подписки ещё может доставить событие уже после отписки.
    act(() => emits[0]({ kind: "tick", atMs: 2, active: 9, waiting: 9 }));
    expect(result.current.live).toBeNull();
    act(() => emits[1]({ kind: "tick", atMs: 3, active: 1, waiting: 0 }));
    expect(result.current.live).toEqual({ atMs: 3, active: 1, waiting: 0 });
  });

  it("после stopped поздние тики этой подписки игнорируются", async () => {
    let emit: (e: SamplerEvent) => void = () => {};
    vi.mocked(api.startSampler).mockImplementation(async (_id, _m, onEvent) => {
      emit = onEvent;
      return "s-1";
    });
    const { result } = renderHook(() => useSampler("c", "pg-1", true));
    await waitFor(() => expect(api.startSampler).toHaveBeenCalled());
    act(() => emit({ kind: "tick", atMs: 1, active: 2, waiting: 0 }));
    expect(result.current.live).not.toBeNull();
    act(() => emit({ kind: "stopped" }));
    expect(result.current.live).toBeNull();
    act(() => emit({ kind: "tick", atMs: 2, active: 5, waiting: 1 }));
    act(() => emit({ kind: "gap", fromMs: 1, toMs: 2 }));
    expect(result.current.live).toBeNull();
  });

  it("ошибка сэмплера гасит показания, следующий тик снимает ошибку", async () => {
    let emit: (e: SamplerEvent) => void = () => {};
    vi.mocked(api.startSampler).mockImplementation(async (_id, _m, onEvent) => {
      emit = onEvent;
      return "s-1";
    });
    const { result } = renderHook(() => useSampler("c", "pg-1", true));
    await waitFor(() => expect(api.startSampler).toHaveBeenCalled());
    act(() => emit({ kind: "tick", atMs: 1, active: 2, waiting: 0 }));
    act(() => emit({ kind: "error", error: authFailed }));
    expect(result.current.error).toEqual(authFailed);
    expect(result.current.live).toBeNull();
    act(() => emit({ kind: "tick", atMs: 2, active: 1, waiting: 0 }));
    expect(result.current.error).toBeNull();
    expect(result.current.live).toEqual({ atMs: 2, active: 1, waiting: 0 });
  });

  it("отказ самой подписки — ошибка хука; отписка, упавшая с ошибкой, не роняет вкладку", async () => {
    vi.mocked(api.startSampler).mockRejectedValueOnce(authFailed);
    const { result, unmount } = renderHook(() => useSampler("c", "pg-1", true));
    await waitFor(() => expect(result.current.error).toEqual(authFailed));
    vi.mocked(api.startSampler).mockResolvedValue("s-2");
    vi.mocked(api.stopSession).mockRejectedValue(new Error("gone"));
    const second = renderHook(() => useSampler("c", "pg-2", true));
    await waitFor(() => expect(api.startSampler).toHaveBeenCalledTimes(2));
    second.unmount();
    unmount();
    await waitFor(() => expect(api.stopSession).toHaveBeenCalledWith("s-2"));
  });
});
