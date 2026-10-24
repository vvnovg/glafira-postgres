//! Пульс узла и кластера (спец. P2 §5.1): факты и скорости узла и находки по ним.

use serde::{Deserialize, Serialize};

use crate::findings::{Finding, Severity, grade, thresholds as th, worse};
use crate::rates::{CounterSnapshot, delta, rate};

/// Соединения клиентов по состояниям (`pg_stat_activity.state`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionsByState {
    pub active: i64,
    pub idle: i64,
    pub idle_in_transaction: i64,
    pub other: i64,
}

/// Факты узла на момент запроса. Длительности — от `now()` сервера, секунды.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PulseFacts {
    /// Клиентские соединения сейчас.
    pub used: i64,
    /// Сколько доступно обычным ролям: `max_connections` без резерва суперпользователя и `reserved_connections`.
    pub available: i64,
    pub by_state: ConnectionsByState,
    pub longest_transaction_s: Option<f64>,
    pub longest_query_s: Option<f64>,
    pub longest_idle_in_transaction_s: Option<f64>,
    /// Сколько сессий ждут блокировку.
    pub lock_waiting: i64,
    pub longest_lock_wait_s: Option<f64>,
    /// Наибольший `age(datfrozenxid)` по базам.
    pub max_xid_age: i64,
    /// Доля от 2³¹.
    pub xid_age_pct: f64,
    /// Размер `pg_wal`; `None` — нет роли `pg_monitor`.
    pub wal_dir_bytes: Option<i64>,
    pub max_wal_size_bytes: Option<i64>,
    /// Отставание реплики из топологии, не из SQL узла.
    pub lag_bytes: Option<i64>,
    pub replay_delay_ms: Option<i64>,
}

/// Скорости и приросты между двумя снимками счётчиков; без прошлого (сравнимого) снимка все поля пусты.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PulseRates {
    /// Транзакции (фиксации и откаты) в секунду.
    pub tps: Option<f64>,
    pub commits_per_s: Option<f64>,
    pub rollbacks_per_s: Option<f64>,
    /// Доля чтений из кэша буферов за интервал.
    pub cache_hit_pct: Option<f64>,
    pub temp_files_per_s: Option<f64>,
    pub temp_bytes_per_s: Option<f64>,
    pub wal_bytes_per_s: Option<f64>,
    pub deadlocks: Option<i64>,
    pub temp_files: Option<i64>,
    pub blks_read: Option<i64>,
}

/// Ключи счётчиков снимка пульса; `walLsn` — позиция WAL в байтах от `0/0`.
pub const COUNTERS: [&str; 8] = [
    "commits",
    "rollbacks",
    "blksHit",
    "blksRead",
    "tempFiles",
    "tempBytes",
    "deadlocks",
    "walLsn",
];

pub fn rates(prev: Option<&CounterSnapshot>, next: &CounterSnapshot) -> PulseRates {
    let r = |k| rate(prev, next, k);
    let d = |k| delta(prev, next, k);
    let (commits, rollbacks) = (r("commits"), r("rollbacks"));
    let (hit, read) = (d("blksHit"), d("blksRead"));
    PulseRates {
        tps: commits.zip(rollbacks).map(|(c, b)| c + b),
        commits_per_s: commits,
        rollbacks_per_s: rollbacks,
        cache_hit_pct: hit
            .zip(read)
            .filter(|(h, rd)| h + rd > 0)
            .map(|(h, rd)| h as f64 * 100.0 / (h + rd) as f64),
        temp_files_per_s: r("tempFiles"),
        temp_bytes_per_s: r("tempBytes"),
        wal_bytes_per_s: r("walLsn"),
        deadlocks: d("deadlocks"),
        temp_files: d("tempFiles"),
        blks_read: read,
    }
}

/// Находки узла. `standby` — узел реплика по топологии: лаг оценивается только у реплик.
pub fn evaluate(member: &str, standby: bool, f: &PulseFacts, r: &PulseRates) -> Vec<Finding> {
    let m = Some(member);
    let mut out = Vec::new();
    let mut push = |id: &str, sev: Option<Severity>, f: Finding| {
        if let Some(sev) = sev {
            let mut f = f;
            f.id = id.to_string();
            f.severity = sev;
            out.push(f.member(m));
        }
    };
    let base = |tool: &str| Finding::new("", Severity::Info).link_tool(tool, m);
    if f.available > 0 {
        let pct = f.used as f64 * 100.0 / f.available as f64;
        push(
            "connectionsSaturated",
            grade(pct, th::CONNECTIONS_PCT.0, th::CONNECTIONS_PCT.1),
            base("sessions")
                .param("usedPct", pct)
                .param("used", f.used)
                .param("available", f.available),
        );
    }
    if let Some(v) = f.longest_idle_in_transaction_s {
        push(
            "idleInTransaction",
            grade(v, th::IDLE_IN_TRANSACTION_S.0, th::IDLE_IN_TRANSACTION_S.1),
            base("idleTx").param("maxS", v),
        );
    }
    if let Some(v) = f.longest_query_s {
        push(
            "longQuery",
            grade(v, th::LONG_QUERY_S.0, th::LONG_QUERY_S.1),
            base("sessions").param("maxS", v),
        );
    }
    if let Some(v) = f.longest_transaction_s {
        push(
            "longTransaction",
            grade(v, th::LONG_TRANSACTION_S.0, th::LONG_TRANSACTION_S.1),
            base("sessions").param("maxS", v),
        );
    }
    push(
        "xidAge",
        grade(f.xid_age_pct, th::XID_AGE_PCT.0, th::XID_AGE_PCT.1),
        base("horizon")
            .param("agePct", f.xid_age_pct)
            .param("maxXidAge", f.max_xid_age),
    );
    if let Some(v) = f.longest_lock_wait_s {
        push(
            "lockWait",
            grade(v, th::LOCK_WAIT_S.0, th::LOCK_WAIT_S.1),
            base("locks")
                .param("maxS", v)
                .param("count", f.lock_waiting),
        );
    }
    if let (Some(wal), Some(max)) = (f.wal_dir_bytes, f.max_wal_size_bytes.filter(|m| *m > 0)) {
        let ratio = wal as f64 / max as f64;
        push(
            "walDirSize",
            grade(ratio, th::WAL_DIR_RATIO.0, th::WAL_DIR_RATIO.1),
            base("slots")
                .param("walDirBytes", wal)
                .param("maxWalSizeBytes", max),
        );
    }
    if let Some(n) = r.deadlocks.filter(|n| *n > 0) {
        push(
            "deadlocks",
            Some(Severity::Warning),
            base("locks").param("count", n),
        );
    }
    if standby {
        let by_bytes = f
            .lag_bytes
            .and_then(|b| grade(b as f64, th::REPLICA_LAG_BYTES.0, th::REPLICA_LAG_BYTES.1));
        let by_time = f
            .replay_delay_ms
            .and_then(|ms| grade(ms as f64 / 1000.0, th::REPLICA_LAG_S.0, th::REPLICA_LAG_S.1));
        let mut lag = base("replication");
        if let Some(b) = f.lag_bytes {
            lag = lag.param("lagBytes", b);
        }
        if let Some(ms) = f.replay_delay_ms {
            lag = lag.param("delayS", ms as f64 / 1000.0);
        }
        push("replicaLag", worse(by_bytes, by_time), lag);
    }
    if let (Some(pct), Some(reads)) = (r.cache_hit_pct, r.blks_read)
        && pct < th::CACHE_HIT_LOW_PCT
        && reads >= th::CACHE_HIT_MIN_READS
    {
        push(
            "cacheHitLow",
            Some(Severity::Info),
            base("sessions").param("hitPct", pct),
        );
    }
    if let Some(n) = r.temp_files.filter(|n| *n > 0) {
        push(
            "tempFiles",
            Some(Severity::Info),
            base("sessions").param("count", n),
        );
    }
    crate::findings::sort(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::Severity;
    use crate::rates::CounterSnapshot;

    fn facts() -> PulseFacts {
        PulseFacts {
            used: 10,
            available: 100,
            max_xid_age: 1000,
            xid_age_pct: 0.0,
            ..Default::default()
        }
    }

    fn snap(at_ms: i64, v: [i64; 8]) -> CounterSnapshot {
        CounterSnapshot {
            at_ms,
            started_ms: 1,
            reset_ms: None,
            values: COUNTERS.iter().map(|k| k.to_string()).zip(v).collect(),
        }
    }

    fn ids(f: &[Finding]) -> Vec<(&str, Severity)> {
        f.iter().map(|x| (x.id.as_str(), x.severity)).collect()
    }

    #[test]
    fn rates_from_two_snapshots() {
        // commits, rollbacks, blksHit, blksRead, tempFiles, tempBytes, deadlocks, walLsn
        let a = snap(0, [100, 10, 9000, 1000, 0, 0, 0, 5_000]);
        let b = snap(
            2_000,
            [300, 30, 9900, 1100, 2, 4096, 1, 5_000 + 2 * 1024 * 1024],
        );
        let r = rates(Some(&a), &b);
        assert_eq!(r.tps, Some(110.0));
        assert_eq!(
            (r.commits_per_s, r.rollbacks_per_s),
            (Some(100.0), Some(10.0))
        );
        assert_eq!(r.cache_hit_pct, Some(90.0));
        assert_eq!(
            (r.temp_files, r.deadlocks, r.blks_read),
            (Some(2), Some(1), Some(100))
        );
        assert_eq!(r.wal_bytes_per_s, Some(1024.0 * 1024.0));
        let first = rates(None, &b);
        assert_eq!(
            (first.tps, first.cache_hit_pct, first.deadlocks),
            (None, None, None)
        );
    }

    #[test]
    fn findings_follow_the_thresholds() {
        let mut f = facts();
        f.used = 96;
        f.longest_idle_in_transaction_s = Some(400.0);
        f.longest_query_s = Some(4000.0);
        f.xid_age_pct = 60.0;
        f.lock_waiting = 3;
        f.longest_lock_wait_s = Some(31.0);
        f.wal_dir_bytes = Some(5 * 1024);
        f.max_wal_size_bytes = Some(1024);
        let r = PulseRates {
            deadlocks: Some(2),
            cache_hit_pct: Some(50.0),
            blks_read: Some(5000),
            temp_files: Some(1),
            ..Default::default()
        };
        let got = evaluate("pg-1", false, &f, &r);
        assert_eq!(
            ids(&got),
            [
                ("connectionsSaturated", Severity::Critical),
                ("longQuery", Severity::Critical),
                ("walDirSize", Severity::Critical),
                ("deadlocks", Severity::Warning),
                ("idleInTransaction", Severity::Warning),
                ("lockWait", Severity::Warning),
                ("xidAge", Severity::Warning),
                ("cacheHitLow", Severity::Info),
                ("tempFiles", Severity::Info),
            ]
        );
        assert!(got.iter().all(|x| x.member.as_deref() == Some("pg-1")));
        assert_eq!(got[0].params["usedPct"], 96.0);
    }

    #[test]
    fn replica_lag_only_on_standbys_and_cache_hit_needs_enough_reads() {
        let mut f = facts();
        f.lag_bytes = Some(2 * 1024 * 1024 * 1024);
        f.replay_delay_ms = Some(40_000);
        assert_eq!(
            ids(&evaluate("r", true, &f, &PulseRates::default())),
            [("replicaLag", Severity::Critical)]
        );
        assert!(
            evaluate("p", false, &f, &PulseRates::default()).is_empty(),
            "у primary лаг из топологии не оценивается"
        );
        let r = PulseRates {
            cache_hit_pct: Some(10.0),
            blks_read: Some(10),
            ..Default::default()
        };
        assert!(
            evaluate("p", false, &facts(), &r).is_empty(),
            "мало чтений — нет вывода о cache hit"
        );
    }
}
