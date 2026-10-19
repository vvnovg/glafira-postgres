import type { Member, Topology, TopologyView } from "../api/types";

export function member(name: string, patch: Partial<Member> = {}): Member {
  return {
    name,
    host: name,
    port: 5432,
    apiUrl: null,
    role: "replica",
    rawRole: null,
    state: "streaming",
    timeline: 1,
    lagBytes: 0,
    replayDelayMs: null,
    upstream: null,
    tags: {},
    pendingRestart: false,
    serverVersion: "17.2",
    patroniVersion: "4.0.4",
    dcsLastSeenMs: null,
    reachable: true,
    error: null,
    ...patch,
  };
}

export function topology(patch: Partial<Topology> = {}): Topology {
  return {
    connectionId: "c",
    kind: "patroni",
    observedAtMs: 1_000,
    scope: "demo",
    paused: false,
    members: [member("pg-1", { role: "primary", state: "running", lagBytes: null }), member("pg-2", { role: "syncStandby" })],
    warnings: [],
    ...patch,
  };
}

export function topologyView(patch: Partial<TopologyView> = {}): TopologyView {
  return { state: "connected", topology: topology(), catalogMember: "pg-1", catalogFallback: false, error: null, ...patch };
}
