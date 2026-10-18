import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useDebouncedValue } from "./useDebouncedValue";

describe("useDebouncedValue", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("updates only after the delay", () => {
    const { result, rerender } = renderHook(({ v }) => useDebouncedValue(v, 250), { initialProps: { v: "a" } });
    rerender({ v: "b" });
    expect(result.current).toBe("a");
    act(() => {
      vi.advanceTimersByTime(249);
    });
    expect(result.current).toBe("a");
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(result.current).toBe("b");
  });
});
