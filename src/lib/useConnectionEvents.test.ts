import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TopologyEvent, TopologyView } from "../api/types";
import { topologyView } from "../test/fixtures";

// Мок событий Tauri: запоминает подписчиков, чтобы доставить им событие и проверить отписку.
const bus = vi.hoisted(() => ({
  handlers: new Map<string, (e: { payload: unknown }) => void>(),
  off: vi.fn(),
  fail: false,
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (e: { payload: unknown }) => void) => {
    if (bus.fail) throw new Error("no tauri");
    bus.handlers.set(name, handler);
    return bus.off;
  }),
}));

import { applyState, applyTopology, disconnectedView, EVENTS_CAP, useConnectionEvents } from "./useConnectionEvents";

const ev = (atMs: number): TopologyEvent => ({ atMs, kind: "pauseChanged", paused: true });

describe("события наблюдения", () => {
  it("топология заменяет вид и добавляет события сверху", () => {
    const qc = new QueryClient();
    qc.setQueryData(["topologyEvents", "c"], [ev(1)]);
    const view = topologyView();
    applyTopology(qc, { connectionId: "c", view, newEvents: [ev(2), ev(3)] });
    expect(qc.getQueryData<TopologyView>(["topology", "c"])).toEqual(view);
    expect(qc.getQueryData<TopologyEvent[]>(["topologyEvents", "c"])!.map((e) => e.atMs)).toEqual([3, 2, 1]);
  });

  it("журнал ограничен", () => {
    const qc = new QueryClient();
    qc.setQueryData(["topologyEvents", "c"], Array.from({ length: EVENTS_CAP }, (_, i) => ev(i)));
    applyTopology(qc, { connectionId: "c", view: topologyView(), newEvents: [ev(9999)] });
    const list = qc.getQueryData<TopologyEvent[]>(["topologyEvents", "c"])!;
    expect(list).toHaveLength(EVENTS_CAP);
    expect(list[0].atMs).toBe(9999);
  });

  it("смена узла каталога сбрасывает кэш каталога", async () => {
    const qc = new QueryClient();
    qc.setQueryData(["topology", "c"], topologyView({ catalogMember: "a" }));
    qc.setQueryData(["catalog", "c", "databases"], [{ name: "x", sizeBytes: 1 }]);
    applyTopology(qc, { connectionId: "c", view: topologyView({ catalogMember: "b" }), newEvents: [] });
    expect(qc.getQueryState(["catalog", "c", "databases"])!.isInvalidated).toBe(true);
  });

  it("отключение очищает каталог и журнал", () => {
    const qc = new QueryClient();
    qc.setQueryData(["topology", "c"], topologyView());
    qc.setQueryData(["catalog", "c", "databases"], []);
    qc.setQueryData(["catalog", "other", "databases"], []);
    applyState(qc, { connectionId: "c", state: "disconnected", error: null });
    expect(qc.getQueryData(["topology", "c"])).toEqual(disconnectedView);
    expect(qc.getQueryData(["catalog", "c", "databases"])).toBeUndefined();
    expect(qc.getQueryData(["catalog", "other", "databases"])).toEqual([]);
  });

  it("отключение очищает журнал и сведения об узлах, чужие не трогает", () => {
    const qc = new QueryClient();
    qc.setQueryData(["topologyEvents", "c"], [ev(1)]);
    qc.setQueryData(["memberFacts", "c", "pg-1"], { startedAtMs: 1, connections: 1, maxConnections: 100, databases: 2 });
    qc.setQueryData(["memberFacts", "other", "pg-1"], { startedAtMs: 1, connections: 1, maxConnections: 100, databases: 2 });
    applyState(qc, { connectionId: "c", state: "disconnected", error: null });
    expect(qc.getQueryData(["topologyEvents", "c"])).toEqual([]);
    expect(qc.getQueryData(["memberFacts", "c", "pg-1"])).toBeUndefined();
    expect(qc.getQueryData(["memberFacts", "other", "pg-1"])).toBeDefined();
  });

  it("тот же узел каталога каталог не сбрасывает", () => {
    const qc = new QueryClient();
    qc.setQueryData(["topology", "c"], topologyView({ catalogMember: "a" }));
    qc.setQueryData(["catalog", "c", "databases"], []);
    applyTopology(qc, { connectionId: "c", view: topologyView({ catalogMember: "a" }), newEvents: [] });
    expect(qc.getQueryState(["catalog", "c", "databases"])!.isInvalidated).toBe(false);
  });

  it("смена состояния без известной топологии даёт вид с ошибкой", () => {
    const qc = new QueryClient();
    const error = { code: "CONNECTION_FAILED" as const, message: "boom", retryable: true };
    applyState(qc, { connectionId: "c", state: "failed", error });
    expect(qc.getQueryData<TopologyView>(["topology", "c"])).toEqual({ ...disconnectedView, state: "failed", error });
  });

  it("смена состояния сохраняет топологию", () => {
    const qc = new QueryClient();
    qc.setQueryData(["topology", "c"], topologyView());
    applyState(qc, { connectionId: "c", state: "degraded", error: null });
    const v = qc.getQueryData<TopologyView>(["topology", "c"])!;
    expect(v.state).toBe("degraded");
    expect(v.topology).not.toBeNull();
  });
});

describe("useConnectionEvents", () => {
  beforeEach(() => {
    bus.handlers.clear();
    bus.off.mockClear();
    bus.fail = false;
  });

  const setup = () => {
    const qc = new QueryClient();
    const wrapper = ({ children }: { children: ReactNode }) => createElement(QueryClientProvider, { client: qc }, children);
    return { qc, ...renderHook(() => useConnectionEvents(), { wrapper }) };
  };

  it("события Tauri попадают в кэш запросов", async () => {
    const { qc } = setup();
    await waitFor(() => expect(bus.handlers.size).toBe(2));
    bus.handlers.get("topology://changed")!({ payload: { connectionId: "c", view: topologyView(), newEvents: [ev(5)] } });
    expect(qc.getQueryData<TopologyView>(["topology", "c"])!.state).toBe("connected");
    expect(qc.getQueryData<TopologyEvent[]>(["topologyEvents", "c"])).toHaveLength(1);
    bus.handlers.get("connection://state")!({ payload: { connectionId: "c", state: "failed", error: null } });
    expect(qc.getQueryData<TopologyView>(["topology", "c"])!.state).toBe("failed");
  });

  it("при размонтировании подписки снимаются, в том числе поздние", async () => {
    const { unmount } = setup();
    await waitFor(() => expect(bus.handlers.size).toBe(2));
    unmount();
    expect(bus.off).toHaveBeenCalledTimes(2);

    // Размонтирование раньше готовности подписки: отписка выполняется, как только она готова.
    bus.off.mockClear();
    const early = setup();
    early.unmount();
    await waitFor(() => expect(bus.off).toHaveBeenCalledTimes(2));
  });

  it("вне Tauri подписка не удаётся — без необработанной ошибки", async () => {
    bus.fail = true;
    const { unmount } = setup();
    await Promise.resolve();
    expect(() => unmount()).not.toThrow();
  });
});
