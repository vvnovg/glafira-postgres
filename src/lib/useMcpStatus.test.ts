import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { McpStatus } from "../api/types";
import { mcpStatus } from "../test/mcp";

// Мок событий Tauri: запоминает подписчиков, чтобы доставить событие и проверить отписку.
const bus = vi.hoisted(() => ({ handlers: new Map<string, (e: { payload: unknown }) => void>(), off: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (e: { payload: unknown }) => void) => {
    bus.handlers.set(name, handler);
    return bus.off;
  }),
}));
vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return { ...orig, api: { getMcpStatus: vi.fn() } };
});

import { api } from "../api/client";
import { MCP_STATUS_EVENT, MCP_STATUS_KEY, useMcpEvents, useMcpStatus } from "./useMcpStatus";

const client = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });
const wrapper = (qc: QueryClient) => ({ children }: { children: ReactNode }) => createElement(QueryClientProvider, { client: qc }, children);
const both = () => {
  useMcpEvents();
  return useMcpStatus();
};

beforeEach(() => {
  vi.clearAllMocks();
  bus.handlers.clear();
});

describe("useMcpStatus", () => {
  it("первое значение — командой, дальше его заменяет событие mcp://state", async () => {
    expect(MCP_STATUS_EVENT).toBe("mcp://state");
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ openConnections: 1 }));
    const { result } = renderHook(both, { wrapper: wrapper(client()) });
    expect(result.current.loading).toBe(true);
    expect(result.current.status).toBeUndefined();
    await waitFor(() => expect(result.current.status?.openConnections).toBe(1));
    await waitFor(() => expect(bus.handlers.has("mcp://state")).toBe(true));
    act(() => bus.handlers.get("mcp://state")!({ payload: mcpStatus({ openConnections: 3 }) }));
    // Уведомление подписчиков TanStack Query уходит таймером, а не синхронно с записью.
    await waitFor(() => expect(result.current.status?.openConnections).toBe(3));
    expect(api.getMcpStatus).toHaveBeenCalledTimes(1);
  });

  it("событие, пришедшее раньше ответа команды, этим ответом не затирается", async () => {
    let resolve!: (s: McpStatus) => void;
    vi.mocked(api.getMcpStatus).mockReturnValue(new Promise((r) => (resolve = r)));
    const qc = client();
    const { result } = renderHook(both, { wrapper: wrapper(qc) });
    await waitFor(() => expect(bus.handlers.has("mcp://state")).toBe(true));
    act(() => bus.handlers.get("mcp://state")!({ payload: mcpStatus({ server: "stopped" }) }));
    await act(async () => resolve(mcpStatus({ server: "running" })));
    expect(qc.getQueryData<McpStatus>(MCP_STATUS_KEY)?.server).toBe("stopped");
    await waitFor(() => expect(result.current.status?.server).toBe("stopped"));
  });

  it("команда не ответила: статуса нет, загрузка закончена, ошибка есть", async () => {
    vi.mocked(api.getMcpStatus).mockRejectedValue({ code: "INTERNAL", message: "boom", retryable: false });
    const { result } = renderHook(() => useMcpStatus(), { wrapper: wrapper(client()) });
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.status).toBeUndefined();
    expect(result.current.error).toMatchObject({ code: "INTERNAL", message: "boom" });
  });

  it("при размонтировании подписка снимается", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus());
    const { unmount } = renderHook(() => useMcpEvents(), { wrapper: wrapper(client()) });
    await waitFor(() => expect(bus.handlers.has("mcp://state")).toBe(true));
    unmount();
    expect(bus.off).toHaveBeenCalledTimes(1);
  });
});
