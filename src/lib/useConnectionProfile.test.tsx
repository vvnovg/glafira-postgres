import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { connection } from "../test/fixtures";

vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return { ...orig, api: { listConnections: vi.fn() } };
});

import { api } from "../api/client";
import { canAct, useConnectionProfile } from "./useConnectionProfile";

const wrapper = ({ children }: { children: ReactNode }) =>
  createElement(QueryClientProvider, { client: new QueryClient({ defaultOptions: { queries: { retry: false } } }) }, children);

describe("useConnectionProfile", () => {
  beforeEach(() => {
    vi.mocked(api.listConnections).mockReset();
  });

  it("возвращает профиль по id из списка подключений", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "a", name: "A" }), connection({ id: "b", name: "B", environment: "prod" })]);
    const { result } = renderHook(() => useConnectionProfile("b"), { wrapper });
    expect(result.current).toBeUndefined();
    await waitFor(() => expect(result.current?.name).toBe("B"));
    expect(result.current?.environment).toBe("prod");
  });

  it("неизвестный id — undefined", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "a" })]);
    const { result } = renderHook(() => useConnectionProfile("zzz"), { wrapper });
    await waitFor(() => expect(api.listConnections).toHaveBeenCalled());
    expect(result.current).toBeUndefined();
  });
});

describe("canAct", () => {
  it("нужен профиль без read-only", () => {
    expect(canAct(undefined)).toBe(false);
    expect(canAct(connection({ readOnly: true }))).toBe(false);
    expect(canAct(connection({ readOnly: false }))).toBe(true);
  });
});
