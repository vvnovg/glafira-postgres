import { type QueryClient, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { api } from "../api/client";
import type { AppError, ConnState, TopologyEvent, TopologyView } from "../api/types";

export const EVENTS_CAP = 500;

export const disconnectedView: TopologyView = {
  state: "disconnected",
  topology: null,
  catalogMember: null,
  catalogFallback: false,
  error: null,
};

type StatePayload = { connectionId: string; state: ConnState; error: AppError | null };
type TopologyPayload = { connectionId: string; view: TopologyView; newEvents: TopologyEvent[] };

const ofConnection = (prefixes: string[], id: string) => (q: { queryKey: readonly unknown[] }) =>
  prefixes.includes(q.queryKey[0] as string) && q.queryKey[1] === id;

export function applyState(qc: QueryClient, p: StatePayload) {
  if (p.state === "disconnected") {
    qc.setQueryData(["topology", p.connectionId], disconnectedView);
    qc.setQueryData(["topologyEvents", p.connectionId], []);
    qc.removeQueries({ predicate: ofConnection(["catalog", "memberFacts"], p.connectionId) });
    return;
  }
  qc.setQueryData<TopologyView>(["topology", p.connectionId], (old) => ({ ...(old ?? disconnectedView), state: p.state, error: p.error }));
}

export function applyTopology(qc: QueryClient, p: TopologyPayload) {
  const old = qc.getQueryData<TopologyView>(["topology", p.connectionId]);
  qc.setQueryData(["topology", p.connectionId], p.view);
  if (old?.catalogMember && old.catalogMember !== p.view.catalogMember) {
    // Узел каталога сменился (например, после switchover) — каталог читается заново.
    void qc.invalidateQueries({ predicate: ofConnection(["catalog"], p.connectionId) });
  }
  if (p.newEvents.length > 0) {
    // В событии — от старых к новым, в кэше — новые сверху.
    qc.setQueryData<TopologyEvent[]>(["topologyEvents", p.connectionId], (prev = []) =>
      [...[...p.newEvents].reverse(), ...prev].slice(0, EVENTS_CAP),
    );
  }
}

/** Подписка на события наблюдения — один раз, в Layout. */
export function useConnectionEvents() {
  const qc = useQueryClient();
  useEffect(() => {
    // Подписка асинхронна: если хук размонтирован раньше, чем она готова, снимаем её сразу.
    // Вне Tauri (тесты, браузер) подписка отклоняется — это не ошибка.
    let unmounted = false;
    const offs: (() => void)[] = [];
    const subscribe = (p: Promise<() => void>) =>
      p.then((off) => (unmounted ? off() : offs.push(off))).catch(() => {});
    subscribe(listen<StatePayload>("connection://state", (e) => applyState(qc, e.payload)));
    subscribe(listen<TopologyPayload>("topology://changed", (e) => applyTopology(qc, e.payload)));
    return () => {
      unmounted = true;
      offs.forEach((off) => off());
    };
  }, [qc]);
}

export function useTopology(connectionId: string) {
  return useQuery({ queryKey: ["topology", connectionId], queryFn: () => api.getTopology(connectionId), staleTime: Infinity });
}

export function useTopologyEvents(connectionId: string) {
  return useQuery({
    queryKey: ["topologyEvents", connectionId],
    queryFn: () => api.listTopologyEvents(connectionId),
    staleTime: Infinity,
  });
}
