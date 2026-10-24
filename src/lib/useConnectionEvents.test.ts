import { QueryClient, QueryClientProvider, type UseQueryResult } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TopologyEvent, TopologyView } from "../api/types";
import { api } from "../api/client";
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

vi.mock("../api/client", () => ({ api: { getTopology: vi.fn(), listTopologyEvents: vi.fn() } }));

import {
  applyState,
  applyTopology,
  disconnectedView,
  EVENTS_CAP,
  refreshAfterConnect,
  useConnectionEvents,
  useTopology,
  useTopologyEvents,
} from "./useConnectionEvents";

// Типов Node в проекте нет, а `unhandledRejection` приходит только из процесса тестов.
const node = (globalThis as unknown as { process: { on(e: string, f: () => void): void; off(e: string, f: () => void): void } }).process;

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

  it("событие, которое ответ журнала уже принёс, второй раз не добавляется", () => {
    const qc = new QueryClient();
    const e1: TopologyEvent = { atMs: 1_000, kind: "pauseChanged", paused: true };
    const e2: TopologyEvent = { atMs: 2_000, kind: "memberUnreachable", name: "pg-3" };
    const e3: TopologyEvent = { atMs: 3_000, kind: "memberReachable", name: "pg-3" };
    qc.setQueryData(["topologyEvents", "c"], [e2, e1]);
    applyTopology(qc, { connectionId: "c", view: topologyView(), newEvents: [e1, e2, e3] });
    expect(qc.getQueryData(["topologyEvents", "c"])).toEqual([e3, e2, e1]);
  });

  it("события с разным вложенным содержимым — не дубли, а порядок ключей верхнего уровня не важен", () => {
    const qc = new QueryClient();
    // Сегодня события плоские; проверка на случай будущих событий с вложенными полями.
    const nested = (tags: Record<string, unknown>, kind = "memberRoleChanged") => ({ atMs: 1_000, kind, name: "pg-1", tags }) as unknown as TopologyEvent;
    const swapped = { kind: "pauseChanged", atMs: 2_000, paused: true } as TopologyEvent;
    qc.setQueryData(["topologyEvents", "c"], [swapped, nested({ a: 1 })]);
    applyTopology(qc, { connectionId: "c", view: topologyView(), newEvents: [nested({ a: 2 }), { atMs: 2_000, kind: "pauseChanged", paused: true }] });
    expect(qc.getQueryData(["topologyEvents", "c"])).toEqual([nested({ a: 2 }), swapped, nested({ a: 1 })]);
  });

  it("журнал ограничен", () => {
    const qc = new QueryClient();
    qc.setQueryData(["topologyEvents", "c"], Array.from({ length: EVENTS_CAP }, (_, i) => ev(i)));
    applyTopology(qc, { connectionId: "c", view: topologyView(), newEvents: [ev(9999)] });
    const list = qc.getQueryData<TopologyEvent[]>(["topologyEvents", "c"])!;
    expect(list).toHaveLength(EVENTS_CAP);
    expect(list[0].atMs).toBe(9999);
    // Лишнее отбрасывается с хвоста: уходит самое старое (в кэше новые сверху), а не только что пришедшее.
    expect(list[list.length - 1].atMs).toBe(EVENTS_CAP - 2);
    expect(list.map((e) => e.atMs)).not.toContain(EVENTS_CAP - 1);
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

  it("вкладки объектов: смена узла каталога и отключение затрагивают только их подключение", () => {
    const qc = new QueryClient();
    const keys = [
      ["catalog", "c", "table", "orders", "shop", "events"],
      ["catalog", "c", "view", "orders", "shop", "v"],
      ["catalog", "c", "function", "orders", 16401],
      ["catalog", "c", "sequence", "orders", "shop", "s"],
    ];
    for (const k of keys) qc.setQueryData(k, {});
    qc.setQueryData(["catalog", "other", "table", "orders", "shop", "events"], {});
    qc.setQueryData(["topology", "c"], topologyView({ catalogMember: "a" }));
    applyTopology(qc, { connectionId: "c", view: topologyView({ catalogMember: "b" }), newEvents: [] });
    for (const k of keys) expect(qc.getQueryState(k)!.isInvalidated).toBe(true);
    expect(qc.getQueryState(["catalog", "other", "table", "orders", "shop", "events"])!.isInvalidated).toBe(false);
    applyState(qc, { connectionId: "c", state: "disconnected", error: null });
    for (const k of keys) expect(qc.getQueryData(k)).toBeUndefined();
    expect(qc.getQueryData(["catalog", "other", "table", "orders", "shop", "events"])).toEqual({});
  });

  it("после подключения перечитываются запросы PostgreSQL этого подключения, остальные не трогаются", () => {
    const qc = new QueryClient();
    const mine = [
      ["catalog", "c", "table", "orders", "shop", "events"],
      ["catalog", "c", "function", "orders", 16401],
      ["memberFacts", "c", "pg-1"],
    ];
    const rest = [["catalog", "other", "table", "orders", "shop", "events"], ["topology", "c"], ["connections"]];
    for (const k of [...mine, ...rest]) qc.setQueryData(k, {});
    refreshAfterConnect(qc, "c");
    for (const k of mine) expect(qc.getQueryState(k)!.isInvalidated).toBe(true);
    for (const k of rest) expect(qc.getQueryState(k)!.isInvalidated).toBe(false);
  });

  it("тот же узел каталога каталог не сбрасывает", () => {
    const qc = new QueryClient();
    qc.setQueryData(["topology", "c"], topologyView({ catalogMember: "a" }));
    qc.setQueryData(["catalog", "c", "databases"], []);
    applyTopology(qc, { connectionId: "c", view: topologyView({ catalogMember: "a" }), newEvents: [] });
    expect(qc.getQueryState(["catalog", "c", "databases"])!.isInvalidated).toBe(false);
  });

  it("смена состояния сохраняет ошибку из события", () => {
    const qc = new QueryClient();
    qc.setQueryData(["topology", "c"], topologyView());
    const error = { code: "CONNECTION_FAILED" as const, message: "boom", retryable: true };
    applyState(qc, { connectionId: "c", state: "failed", error });
    expect(qc.getQueryData<TopologyView>(["topology", "c"])).toEqual({ ...topologyView(), state: "failed", error });
  });

  it("смена состояния без кэша не синтезирует вид — его читают заново", () => {
    const qc = new QueryClient();
    applyState(qc, { connectionId: "c", state: "failed", error: null });
    expect(qc.getQueryData(["topology", "c"])).toBeUndefined();
  });

  it("отключение без кэша пишет вид «отключено» и пустой журнал: событие — полная картина", () => {
    const qc = new QueryClient();
    applyState(qc, { connectionId: "c", state: "disconnected", error: null });
    expect(qc.getQueryData(["topology", "c"])).toEqual(disconnectedView);
    expect(qc.getQueryData(["topologyEvents", "c"])).toEqual([]);
  });

  it("топология без кэша журнала вид пишет, журнал не синтезирует", () => {
    const qc = new QueryClient();
    const view = topologyView();
    applyTopology(qc, { connectionId: "c", view, newEvents: [ev(2)] });
    expect(qc.getQueryData(["topology", "c"])).toEqual(view);
    expect(qc.getQueryData(["topologyEvents", "c"])).toBeUndefined();
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
    vi.mocked(listen).mockClear();
  });

  const setup = () => {
    const qc = new QueryClient();
    const wrapper = ({ children }: { children: ReactNode }) => createElement(QueryClientProvider, { client: qc }, children);
    return { qc, ...renderHook(() => useConnectionEvents(), { wrapper }) };
  };

  it("события Tauri попадают в кэш запросов", async () => {
    const { qc } = setup();
    qc.setQueryData(["topologyEvents", "c"], [ev(1)]);
    await waitFor(() => expect(bus.handlers.size).toBe(2));
    bus.handlers.get("topology://changed")!({ payload: { connectionId: "c", view: topologyView(), newEvents: [ev(5)] } });
    expect(qc.getQueryData<TopologyView>(["topology", "c"])!.state).toBe("connected");
    expect(qc.getQueryData<TopologyEvent[]>(["topologyEvents", "c"])!.map((e) => e.atMs)).toEqual([5, 1]);
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
    const unhandled = vi.fn();
    node.on("unhandledRejection", unhandled);
    try {
      bus.fail = true;
      const { unmount } = setup();
      // Отклонённая цепочка промисов разбирается за несколько макрозадач; `unhandledRejection` приходит после них.
      for (let i = 0; i < 3; i++) await new Promise((resolve) => setTimeout(resolve, 0));
      expect(vi.mocked(listen)).toHaveBeenCalledTimes(2);
      expect(bus.handlers.size).toBe(0);
      expect(() => unmount()).not.toThrow();
      for (let i = 0; i < 3; i++) await new Promise((resolve) => setTimeout(resolve, 0));
      expect(unhandled).not.toHaveBeenCalled();
    } finally {
      node.off("unhandledRejection", unhandled);
    }
  });
});

// Ответ, запрошенный до события, старее события: он не должен его затирать, а при `staleTime: Infinity`
// исправить это было бы некому — после отключения событий больше нет.
describe("запоздавшие ответы и события", () => {
  const deferred = <T>() => {
    let resolve!: (v: T) => void;
    const promise = new Promise<T>((r) => (resolve = r));
    return { promise, resolve };
  };
  const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
  // useQuery перерисовывает компонент только при смене прочитанных полей: читаем их сразу.
  const track = <T,>(q: UseQueryResult<T>) => ({ data: q.data, isError: q.isError, fetchStatus: q.fetchStatus, refetch: q.refetch });

  const setup = () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) => createElement(QueryClientProvider, { client: qc }, children);
    return { qc, wrapper };
  };

  beforeEach(() => {
    vi.mocked(api.getTopology).mockReset();
    vi.mocked(api.listTopologyEvents).mockReset();
  });

  it("отключение во время загрузки топологии: запоздавший «connected» его не затирает", async () => {
    const { qc, wrapper } = setup();
    const stale = deferred<TopologyView>();
    vi.mocked(api.getTopology).mockReturnValueOnce(stale.promise);
    const { result } = renderHook(() => track(useTopology("c")), { wrapper });
    await waitFor(() => expect(api.getTopology).toHaveBeenCalledTimes(1));

    await act(async () => {
      applyState(qc, { connectionId: "c", state: "disconnected", error: null });
      stale.resolve(topologyView({ state: "connected" }));
      await flush();
    });

    expect(qc.getQueryData(["topology", "c"])).toEqual(disconnectedView);
    expect(result.current.data).toEqual(disconnectedView);
    expect(result.current.isError).toBe(false);
    expect(result.current.fetchStatus).toBe("idle");
    expect(api.getTopology).toHaveBeenCalledTimes(1);
  });

  it.each([
    ["первая загрузка", false],
    ["обновление при данных в кэше", true],
  ])("медленный getTopology (%s) не затирает более новый вид из topology://changed", async (_name, cached) => {
    const { qc, wrapper } = setup();
    const stale = deferred<TopologyView>();
    if (cached) qc.setQueryData(["topology", "c"], topologyView({ catalogMember: "pg-0" }));
    vi.mocked(api.getTopology).mockReturnValueOnce(stale.promise);
    const { result } = renderHook(() => track(useTopology("c")), { wrapper });
    if (cached) await act(async () => void result.current.refetch());
    await waitFor(() => expect(api.getTopology).toHaveBeenCalledTimes(1));

    const fresh = topologyView({ catalogMember: "pg-2" });
    await act(async () => {
      applyTopology(qc, { connectionId: "c", view: fresh, newEvents: [] });
      stale.resolve(topologyView({ catalogMember: "pg-1" }));
      await flush();
    });

    expect(qc.getQueryData(["topology", "c"])).toEqual(fresh);
    expect(result.current.data).toEqual(fresh);
    expect(result.current.isError).toBe(false);
  });

  it("смена состояния во время загрузки топологии: запоздавший ответ не затирает вид из кэша", async () => {
    const { qc, wrapper } = setup();
    const stale = deferred<TopologyView>();
    qc.setQueryData(["topology", "c"], topologyView());
    vi.mocked(api.getTopology).mockReturnValueOnce(stale.promise);
    const { result } = renderHook(() => track(useTopology("c")), { wrapper });
    await act(async () => void result.current.refetch());

    await act(async () => {
      applyState(qc, { connectionId: "c", state: "degraded", error: null });
      stale.resolve(topologyView({ state: "connected" }));
      await flush();
    });

    expect(result.current.data!.state).toBe("degraded");
    expect(result.current.data!.topology).not.toBeNull();
  });

  it("отключение во время загрузки журнала: запоздавший список его не затирает", async () => {
    const { qc, wrapper } = setup();
    const stale = deferred<TopologyEvent[]>();
    vi.mocked(api.listTopologyEvents).mockReturnValueOnce(stale.promise);
    const { result } = renderHook(() => track(useTopologyEvents("c")), { wrapper });
    await waitFor(() => expect(api.listTopologyEvents).toHaveBeenCalledTimes(1));

    await act(async () => {
      applyState(qc, { connectionId: "c", state: "disconnected", error: null });
      stale.resolve([ev(2), ev(1)]);
      await flush();
    });

    expect(qc.getQueryData(["topologyEvents", "c"])).toEqual([]);
    expect(result.current.data).toEqual([]);
    expect(api.listTopologyEvents).toHaveBeenCalledTimes(1);
  });

  it("обновление журнала запоздало: добавленные события не теряются", async () => {
    const { qc, wrapper } = setup();
    const stale = deferred<TopologyEvent[]>();
    qc.setQueryData(["topologyEvents", "c"], [ev(1)]);
    vi.mocked(api.listTopologyEvents).mockReturnValueOnce(stale.promise);
    const { result } = renderHook(() => track(useTopologyEvents("c")), { wrapper });
    await act(async () => void result.current.refetch());

    await act(async () => {
      applyTopology(qc, { connectionId: "c", view: topologyView(), newEvents: [ev(2), ev(3)] });
      stale.resolve([ev(1)]);
      await flush();
    });

    expect(result.current.data!.map((e) => e.atMs)).toEqual([3, 2, 1]);
  });

  it("первая загрузка журнала идёт, пришло событие: без истории журнал читается заново и целиком", async () => {
    const { qc, wrapper } = setup();
    const stale = deferred<TopologyEvent[]>();
    vi.mocked(api.listTopologyEvents).mockReturnValueOnce(stale.promise).mockResolvedValueOnce([ev(2), ev(1)]);
    const { result } = renderHook(() => track(useTopologyEvents("c")), { wrapper });
    await waitFor(() => expect(api.listTopologyEvents).toHaveBeenCalledTimes(1));

    await act(async () => {
      applyTopology(qc, { connectionId: "c", view: topologyView(), newEvents: [ev(2)] });
      await flush();
    });
    // Только что пришедшее событие не выдаётся за всю историю: журнал перечитан.
    expect(api.listTopologyEvents).toHaveBeenCalledTimes(2);
    await waitFor(() => expect(result.current.data!.map((e) => e.atMs)).toEqual([2, 1]));

    await act(async () => {
      stale.resolve([ev(1)]);
      await flush();
    });
    expect(result.current.data!.map((e) => e.atMs)).toEqual([2, 1]);
  });

  it("смена состояния при неизвестной топологии (вебвью перезагружен): вид читается заново", async () => {
    const { qc, wrapper } = setup();
    const stale = deferred<TopologyView>();
    const truth = topologyView({ state: "degraded" });
    vi.mocked(api.getTopology).mockReturnValueOnce(stale.promise).mockResolvedValueOnce(truth);
    const { result } = renderHook(() => track(useTopology("c")), { wrapper });
    await waitFor(() => expect(api.getTopology).toHaveBeenCalledTimes(1));

    await act(async () => {
      applyState(qc, { connectionId: "c", state: "degraded", error: null });
      await flush();
    });
    expect(api.getTopology).toHaveBeenCalledTimes(2);
    await waitFor(() => expect(result.current.data).toEqual(truth));

    await act(async () => {
      stale.resolve(topologyView({ state: "connected" }));
      await flush();
    });
    expect(result.current.data).toEqual(truth);
  });
});
