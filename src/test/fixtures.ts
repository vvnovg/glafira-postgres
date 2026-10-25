import type { ConnectionProfile, DiagData, DiagReport, Holder, Member, PulseFacts, PulseRates, ReplicationData, SessionRow, Sender, Slot, Subscription, Topology, TopologyView } from "../api/types";

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

/** Факты узла пульса: нули и `null` (ничего не заметно); `patch` меняет нужные поля. */
export function pulseFacts(patch: Partial<PulseFacts> = {}): PulseFacts {
  return {
    used: 0,
    available: 0,
    byState: { active: 0, idle: 0, idleInTransaction: 0, other: 0 },
    longestTransactionS: null,
    longestQueryS: null,
    longestIdleInTransactionS: null,
    lockWaiting: 0,
    longestLockWaitS: null,
    maxXidAge: 0,
    xidAgePct: 0,
    walDirBytes: null,
    maxWalSizeBytes: null,
    lagBytes: null,
    replayDelayMs: null,
    ...patch,
  };
}

/** Скорости пульса: все `null` — первый вызов, сравнимого снимка счётчиков ещё нет. */
export function pulseRates(patch: Partial<PulseRates> = {}): PulseRates {
  return {
    tps: null,
    commitsPerS: null,
    rollbacksPerS: null,
    cacheHitPct: null,
    tempFilesPerS: null,
    tempBytesPerS: null,
    walBytesPerS: null,
    deadlocks: null,
    tempFiles: null,
    blksRead: null,
    ...patch,
  };
}

/** Строка списка сессий: обычная клиентская сессия без запроса и ожиданий; время старта выводится из `pid`. */
export function sessionRow(patch: Partial<SessionRow> = {}): SessionRow {
  const pid = patch.pid ?? 1;
  return {
    pid,
    leaderPid: null,
    backendType: "client backend",
    database: "orders",
    user: "app",
    application: null,
    clientAddr: "10.0.0.5",
    clientPort: 50_000,
    backendStartUs: 1_700_000_000_000_000 + pid,
    xactAgeS: null,
    queryAgeS: null,
    stateAgeS: null,
    state: "active",
    waitEventType: null,
    waitEvent: null,
    backendXid: null,
    xminAge: null,
    queryId: null,
    query: null,
    queryTruncated: false,
    blockedBy: [],
    locksHeld: 0,
    glafira: false,
    ...patch,
  };
}

/** Держатель горизонта xmin: сессия без подробностей; `patch` меняет нужные поля. */
export function holder(patch: Partial<Holder> = {}): Holder {
  return {
    kind: "session",
    name: "1",
    member: null,
    xminAge: 0,
    durationS: null,
    pid: null,
    backendStartUs: null,
    user: null,
    application: null,
    database: null,
    state: null,
    gid: null,
    slotType: null,
    active: null,
    catalog: false,
    clientAddr: null,
    ...patch,
  };
}

/** Отправитель WAL: потоковая асинхронная реплика без отставания; `patch` меняет нужные поля. */
export function sender(patch: Partial<Sender> = {}): Sender {
  return {
    pid: 100,
    user: "replicator",
    application: null,
    member: null,
    clientAddr: "10.0.0.2",
    state: "streaming",
    syncState: "async",
    syncPriority: 0,
    sentLagBytes: 0,
    writeLagBytes: 0,
    flushLagBytes: 0,
    replayLagBytes: 0,
    writeLagS: null,
    flushLagS: null,
    replayLagS: null,
    replyAgeS: 0,
    slotName: null,
    xminAge: null,
    ...patch,
  };
}

/** Данные инструмента «Реплики» (без `kind`): primary без отправителей и настроек; `patch` меняет нужные поля. */
export function replication(patch: Partial<ReplicationData> = {}): ReplicationData {
  return {
    inRecovery: false,
    senders: [],
    synchronousStandbyNames: null,
    synchronousCommit: null,
    receiver: null,
    replay: null,
    conflicts: null,
    hotStandbyFeedback: null,
    maxStandbyStreamingDelayMs: null,
    detailsHidden: false,
    ...patch,
  };
}

/** Слот репликации: активный физический слот без отметок версий PostgreSQL 14+; `patch` меняет нужные поля. */
export function slot(patch: Partial<Slot> = {}): Slot {
  return {
    name: "slot_1",
    slotType: "physical",
    plugin: null,
    database: null,
    temporary: false,
    active: true,
    activePid: null,
    xminAge: null,
    catalogXminAge: null,
    restartLsn: null,
    confirmedFlushLsn: null,
    walStatus: "reserved",
    safeWalSize: null,
    retainedBytes: null,
    twoPhase: null,
    conflicting: null,
    inactiveSinceMs: null,
    invalidationReason: null,
    failover: null,
    synced: null,
    patroni: null,
    ...patch,
  };
}

/** Подписка логической репликации с работающим процессом применения; `patch` меняет нужные поля. */
export function subscription(patch: Partial<Subscription> = {}): Subscription {
  return {
    name: "sub_1",
    pid: 1,
    receivedLsn: null,
    lastMsgAgeS: null,
    latestEndAgeS: null,
    applyErrorCount: null,
    syncErrorCount: null,
    ...patch,
  };
}
