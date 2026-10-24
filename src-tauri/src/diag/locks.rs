//! Блокировки и DDL в очереди (спец. P2 §5.4). `pg_blocking_pids` — только для ждущих блокировку.

use pg_advisor::locktree::{self, LockRow, LockSession, WaitingLock};
use pg_advisor::rules::locks as rules;
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::diag::caps::{Caps, V14};
use crate::diag::{
    DiagData, DiagReport, Tool, head, make_report, on_member, sql::BACKEND_START_US,
};
use crate::error::AppResult;
use crate::state::AppState;
use crate::topology::runtime::DiagTarget;

/// Участники цепочек блокировок (ждущие и блокирующие), по одной строке на сессию. Колонки: 0 pid, 1 `backend_start`,
/// 2 пользователь, 3 приложение, 4 БД, 5 состояние, 6 запрос, 7–8 возраст транзакции и запроса, 9 `pg_blocking_pids`,
/// 10–11 вид и режим запрошенной блокировки, 12 отношение (`regclass`, только БД соединения), 13 oid отношения,
/// 14 БД отношения, 15 ожидание в секундах. До PostgreSQL 14 нет `pg_locks.waitstart`: ожидание оценивается по
/// `state_change` и помечается `approx`.
pub fn locks_sql(caps: Caps) -> String {
    let wait = if caps.at_least(V14) {
        "extract(epoch FROM now() - l.waitstart)::float8"
    } else {
        "CASE WHEN l.pid IS NOT NULL THEN extract(epoch FROM now() - a.state_change)::float8 END"
    };
    format!(
        "WITH w AS (SELECT pid, pg_blocking_pids(pid) AS blocked_by FROM pg_stat_activity WHERE wait_event_type = 'Lock'), \
              inv AS (SELECT pid FROM w UNION SELECT unnest(blocked_by) FROM w) \
         SELECT a.pid, {BACKEND_START_US}, a.usename::text, a.application_name, a.datname::text, a.state, left(a.query, 16384), \
                extract(epoch FROM now() - a.xact_start)::float8, extract(epoch FROM now() - a.query_start)::float8, \
                COALESCE(w.blocked_by, '{{}}'::int[]), l.locktype, l.mode, \
                CASE WHEN l.database = (SELECT oid FROM pg_database WHERE datname = current_database()) THEN l.relation::regclass::text END, \
                l.relation::bigint, ld.datname::text, {wait} \
         FROM inv JOIN pg_stat_activity a ON a.pid = inv.pid \
         LEFT JOIN w ON w.pid = a.pid \
         LEFT JOIN LATERAL (SELECT * FROM pg_locks x WHERE x.pid = a.pid AND NOT x.granted LIMIT 1) l ON true \
         LEFT JOIN pg_database ld ON ld.oid = l.database \
         ORDER BY a.pid"
    )
}

pub async fn collect(c: &Client, caps: Caps) -> AppResult<Vec<LockRow>> {
    let rows = c
        .query(&locks_sql(caps), &[])
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(rows
        .iter()
        .map(|r| {
            let locktype: Option<String> = r.get(10);
            LockRow {
                session: LockSession {
                    pid: r.get(0),
                    backend_start_us: r.get(1),
                    user: r.get(2),
                    application: r.get(3),
                    database: r.get(4),
                    state: r.get(5),
                    query: r.get(6),
                    xact_age_s: r.get(7),
                    query_age_s: r.get(8),
                },
                blocked_by: r.get(9),
                waiting: locktype.map(|locktype| WaitingLock {
                    locktype: Some(locktype),
                    mode: r.get(11),
                    relation: r.get(12),
                    relation_oid: r.get(13),
                    relation_database: r.get(14),
                    wait_s: r.get(15),
                    approx: !caps.at_least(V14),
                }),
            }
        })
        .collect())
}

pub async fn report(st: &AppState, id: &str, target: &DiagTarget) -> AppResult<DiagReport> {
    let (h, rows) = on_member(st, id, target, |c| {
        Box::pin(async move {
            let h = head(c).await?;
            let rows = collect(c, Caps::new(h.version_num)).await?;
            Ok((h, rows))
        })
    })
    .await?;
    let data = locktree::build(&rows);
    let member = target.member.as_ref().map(|m| m.name.as_str());
    let findings = rules::evaluate(member, &data);
    Ok(make_report(
        Tool::Locks,
        target,
        Some(&h),
        DiagData::Locks(data),
        findings,
        Vec::new(),
    ))
}
