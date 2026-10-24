import type { ConnectionProfile, DiagData, DiagReport, Member, Topology, TopologyView } from "../api/types";

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
    scheduledRestart: null,
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
    scheduledSwitchover: null,
    members: [member("pg-1", { role: "primary", state: "running", lagBytes: null }), member("pg-2", { role: "syncStandby" })],
    warnings: [],
    ...patch,
  };
}

export function topologyView(patch: Partial<TopologyView> = {}): TopologyView {
  return { state: "connected", topology: topology(), catalogMember: "pg-1", catalogFallback: false, error: null, ...patch };
}

/** Профиль подключения целиком (все поля `ConnectionProfile`), по умолчанию — кластер Patroni. */
export function connection(patch: Partial<ConnectionProfile> = {}): ConnectionProfile {
  return {
    id: "c",
    name: "billing",
    color: "#336699",
    environment: "dev",
    readOnly: false,
    kind: "patroni",
    hosts: [{ host: "pg-1", port: 5432 }],
    targetServerType: "primary",
    patroni: { urls: ["http://pg-1:8008"], username: null, tls: null, hostOverrides: [] },
    pg: { username: "glafira_diag", database: "postgres", sslMode: "prefer", tls: { caPath: null, clientCert: null }, connectTimeoutS: 10, databases: null },
    ssh: null,
    ...patch,
  };
}

/** Отчёт инструмента: `tool` берётся из `data.kind`, узел — первый участник, «за лидером», без находок и недоступного. */
export function diagReport(data: DiagData, patch: Partial<DiagReport> = {}): DiagReport {
  return {
    tool: data.kind,
    member: "pg-1",
    followsLeader: true,
    observedAtMs: 1_700_000_000_000,
    serverVersionNum: 170004,
    inRecovery: false,
    data,
    findings: [],
    unavailable: [],
    ...patch,
  };
}
