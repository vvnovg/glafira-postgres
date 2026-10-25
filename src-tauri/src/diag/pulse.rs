//! Пульс узла и кластера (спец. P2 §5.1): все доступные участники параллельно, по одному запросу на узел.

use std::collections::BTreeMap;

use futures_util::future::join_all;
use pg_advisor::findings::Finding;
use pg_advisor::rates::CounterSnapshot;
use pg_advisor::rules::pulse::{self as rules, PulseFacts, PulseRates};
use serde::Serialize;
use tokio_postgres::Client;

use crate::commands::catalog::{catalog_limit, on_catalog_node_within};
use crate::conn::errors::from_pg;
use crate::diag::caps::{Caps, V14, V16};
use crate::diag::{DiagData, DiagReport, Tool, Unavailable, head, make_report};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::AppState;
use crate::topology::model::Role;
use crate::topology::runtime::DiagTarget;

/// Строка пульса: участник кластера, его факты и скорости. Недоступный узел и ошибка запроса — в `error`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PulseNode {
    pub member: String,
    pub role: Role,
    /// Достижимость по топологии; запрос пульса мог всё равно не удаться — тогда `error` заполнена, а `facts` пусты.
    pub reachable: bool,
    pub error: Option<AppError>,
    pub facts: Option<PulseFacts>,
    pub rates: Option<PulseRates>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PulseData {
    pub nodes: Vec<PulseNode>,
}

/// Один запрос на узел. Собственное соединение пульса считается в соединениях, но не в длительностях: его запрос
/// всегда активен, а `now()` — начало транзакции, то есть «длительность» его запроса нулевая или отрицательная. Колонки: 0–3 соединения (всего, active, idle, idle in transaction), 4 доступно,
/// 5–7 длительности (транзакция, запрос, простой в транзакции), 8 ждут блокировку, 9 дольше всех ждёт,
/// 10 возраст XID, 11–17 счётчики (`commits` … `deadlocks`, порядок `rules::COUNTERS`), 18 `stats_reset`,
/// 19 позиция WAL, 20 запуск postmaster, 21 `now()`, 22 `max_wal_size`.
pub fn pulse_sql(caps: Caps) -> String {
    // `reserved_connections` появился в 16; `pg_locks.waitstart` — в 14, раньше ожидание считается по `state_change`.
    // И число ждущих, и самое долгое ожидание — только по клиентским сессиям (`a`): иначе autovacuum или фоновый
    // процесс давали бы «0 ждут» с долгим ожиданием в одной клетке. Ожидания служебных процессов видны в «Блокировках».
    let reserved = caps.col(V16, "current_setting('reserved_connections')::int", "int");
    let lock_wait = if caps.at_least(V14) {
        "(SELECT max(extract(epoch FROM now() - l.waitstart)) FROM pg_locks l JOIN a ON a.pid = l.pid WHERE NOT l.granted)"
    } else {
        "(SELECT max(extract(epoch FROM now() - state_change)) FROM a WHERE wait_event_type = 'Lock')"
    };
    format!(
        "WITH a AS (SELECT pid, state, xact_start, query_start, state_change, wait_event_type FROM pg_stat_activity \
                    WHERE backend_type = 'client backend'), \
              d AS (SELECT sum(xact_commit)::bigint AS c, sum(xact_rollback)::bigint AS r, sum(blks_hit)::bigint AS h, \
                           sum(blks_read)::bigint AS rd, sum(temp_files)::bigint AS tf, sum(temp_bytes)::bigint AS tb, \
                           sum(deadlocks)::bigint AS dl, max((extract(epoch FROM stats_reset) * 1000)::bigint) AS reset \
                    FROM pg_stat_database) \
         SELECT (SELECT count(*) FROM a)::bigint, \
                (SELECT count(*) FROM a WHERE state = 'active')::bigint, \
                (SELECT count(*) FROM a WHERE state = 'idle')::bigint, \
                (SELECT count(*) FROM a WHERE state LIKE 'idle in transaction%')::bigint, \
                (current_setting('max_connections')::int - current_setting('superuser_reserved_connections')::int \
                 - COALESCE({reserved}, 0))::bigint, \
                (SELECT max(extract(epoch FROM now() - xact_start)) FROM a WHERE pid <> pg_backend_pid())::float8, \
                (SELECT max(extract(epoch FROM now() - query_start)) FROM a WHERE state = 'active' AND pid <> pg_backend_pid())::float8, \
                (SELECT max(extract(epoch FROM now() - state_change)) FROM a WHERE state LIKE 'idle in transaction%')::float8, \
                (SELECT count(*) FROM a WHERE wait_event_type = 'Lock')::bigint, \
                {lock_wait}::float8, \
                (SELECT max(age(datfrozenxid)) FROM pg_database)::bigint, \
                d.c, d.r, d.h, d.rd, d.tf, d.tb, d.dl, d.reset, \
                (CASE WHEN pg_is_in_recovery() THEN pg_wal_lsn_diff(pg_last_wal_receive_lsn(), '0/0') \
                      ELSE pg_wal_lsn_diff(pg_current_wal_lsn(), '0/0') END)::bigint, \
                (extract(epoch FROM pg_postmaster_start_time()) * 1000)::bigint, \
                (extract(epoch FROM now()) * 1000)::bigint, \
                (SELECT setting::bigint * 1024 * 1024 FROM pg_settings WHERE name = 'max_wal_size') \
         FROM d"
    )
}

/// Размер `pg_wal` отдельным запросом: без `pg_monitor` он падает с отказом в праве, а остальной пульс — нет.
const WAL_DIR_SQL: &str = "SELECT COALESCE(sum(size), 0)::bigint FROM pg_ls_waldir()";

/// Факты, снимок счётчиков и пометка о недоступном размере `pg_wal` (нет `pg_monitor`).
pub async fn collect(
    c: &Client,
    caps: Caps,
) -> AppResult<(PulseFacts, CounterSnapshot, Option<Unavailable>)> {
    let r = c
        .query_one(&pulse_sql(caps), &[])
        .await
        .map_err(|e| from_pg(&e))?;
    let (used, active, idle, idle_tx): (i64, i64, i64, i64) =
        (r.get(0), r.get(1), r.get(2), r.get(3));
    let max_xid_age: i64 = r.get::<_, Option<i64>>(10).unwrap_or(0);
    let mut values = BTreeMap::new();
    for (i, key) in rules::COUNTERS.iter().enumerate().take(7) {
        if let Some(v) = r.get::<_, Option<i64>>(11 + i) {
            values.insert(key.to_string(), v);
        }
    }
    if let Some(lsn) = r.get::<_, Option<i64>>(19) {
        values.insert("walLsn".into(), lsn);
    }
    let snapshot = CounterSnapshot {
        at_ms: r.get(21),
        started_ms: r.get(20),
        reset_ms: r.get(18),
        values,
    };
    let (wal_dir_bytes, unavailable) = match c.query_one(WAL_DIR_SQL, &[]).await {
        Ok(w) => (Some(w.get(0)), None),
        Err(e) if from_pg(&e).code == ErrorCode::NotAuthorized => (
            None,
            Some(Unavailable {
                part: "walDir".into(),
                member: None,
                reason: ltext!(
                    "diag.unavailable.privilege",
                    "{{part}}: needs the pg_monitor role",
                    part = "walDir"
                ),
            }),
        ),
        Err(e) => return Err(from_pg(&e)),
    };
    let facts = PulseFacts {
        used,
        available: r.get(4),
        by_state: rules::ConnectionsByState {
            active,
            idle,
            idle_in_transaction: idle_tx,
            other: (used - active - idle - idle_tx).max(0),
        },
        longest_transaction_s: r.get(5),
        longest_query_s: r.get(6),
        longest_idle_in_transaction_s: r.get(7),
        lock_waiting: r.get(8),
        longest_lock_wait_s: r.get(9),
        max_xid_age,
        xid_age_pct: max_xid_age as f64 * 100.0 / pg_advisor::findings::thresholds::XID_LIMIT,
        wal_dir_bytes,
        max_wal_size_bytes: r.get(22),
        lag_bytes: None,
        replay_delay_ms: None,
    };
    Ok((facts, snapshot, unavailable))
}

type NodeResult = AppResult<(PulseFacts, CounterSnapshot, Option<Unavailable>)>;

/// Пульс кластера: все достижимые участники параллельно, скорости — от прошлого снимка того же узла.
pub async fn report(st: &AppState, id: &str, target: &DiagTarget) -> AppResult<DiagReport> {
    let limit = catalog_limit(&target.profile);
    let members = &target.topology.members;
    let queries = members.iter().map(|m| async move {
        if !m.reachable {
            return None;
        }
        let one: NodeResult =
            on_catalog_node_within(limit, st.client(id, Some(&m.name), None), |c| {
                Box::pin(async move {
                    let h = head(c).await?;
                    collect(c, Caps::new(h.version_num)).await
                })
            })
            .await;
        Some(one)
    });
    let results = join_all(queries).await;

    let (mut nodes, mut findings, mut unavailable) =
        (Vec::new(), Vec::<Finding>::new(), Vec::new());
    for (m, result) in members.iter().zip(results) {
        let node = |reachable, error, facts, rates| PulseNode {
            member: m.name.clone(),
            role: m.role,
            reachable,
            error,
            facts,
            rates,
        };
        match result {
            // Топология не достучалась: её ошибка и есть причина.
            None => nodes.push(node(false, m.error.clone(), None, None)),
            Some(Err(e)) => nodes.push(node(m.reachable, Some(e), None, None)),
            Some(Ok((mut facts, snapshot, missing))) => {
                // Лаг реплики пульс берёт из топологии, своего запроса на него нет.
                facts.lag_bytes = m.lag_bytes;
                facts.replay_delay_ms = m.replay_delay_ms;
                let prev = st.counters.swap(
                    id,
                    target.generation,
                    &m.name,
                    Tool::Pulse,
                    snapshot.clone(),
                );
                let rates = rules::rates(prev.as_ref(), &snapshot);
                findings.extend(rules::evaluate(
                    &m.name,
                    m.role.is_standby(),
                    &facts,
                    &rates,
                ));
                unavailable.extend(missing.map(|u| Unavailable {
                    member: Some(m.name.clone()),
                    ..u
                }));
                nodes.push(node(true, None, Some(facts), Some(rates)));
            }
        }
    }
    Ok(make_report(
        Tool::Pulse,
        target,
        None,
        DiagData::Pulse(PulseData { nodes }),
        findings,
        unavailable,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_sql_uses_waitstart_and_reserved_connections_only_where_they_exist() {
        let old = pulse_sql(Caps::new(130000));
        let mid = pulse_sql(Caps::new(150000));
        let new = pulse_sql(Caps::new(170000));
        assert!(
            !old.contains("waitstart") && mid.contains("waitstart") && new.contains("waitstart")
        );
        assert!(
            !mid.contains("current_setting('reserved_connections')")
                && new.contains("current_setting('reserved_connections')")
        );
        assert!(old.contains("NULL::int"));
    }

    #[test]
    fn lock_wait_count_and_longest_wait_are_both_over_client_sessions() {
        // На 14+ самое долгое ожидание — по `pg_locks`, но только строки клиентских сессий (`a`), как и число ждущих.
        let new = pulse_sql(Caps::new(170000));
        assert!(new.contains("FROM pg_locks l JOIN a ON a.pid = l.pid WHERE NOT l.granted"));
        assert!(new.contains("(SELECT count(*) FROM a WHERE wait_event_type = 'Lock')"));
        // На 13 оба числа и так берутся из `a`.
        let old = pulse_sql(Caps::new(130000));
        assert!(old.contains("FROM a WHERE wait_event_type = 'Lock')::bigint"));
        assert!(!old.contains("pg_locks"));
    }

    #[test]
    fn durations_skip_the_collecting_session() {
        let sql = pulse_sql(Caps::new(170000));
        assert_eq!(
            sql.matches("AND pid <> pg_backend_pid()").count(),
            1,
            "самый долгий активный запрос"
        );
        assert!(sql.contains("FROM a WHERE pid <> pg_backend_pid())::float8"));
    }
}
