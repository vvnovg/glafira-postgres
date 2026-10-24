//! Сессии и idle in transaction (спец. P2 §5.2).

use pg_advisor::rules::sessions::{self as rules, SessionRow, SessionSettings, SessionsData};
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::diag::caps::{Caps, V14};
use crate::diag::{
    DiagData, DiagReport, Tool, head, make_report, on_member, sql::BACKEND_START_US,
};
use crate::error::AppResult;
use crate::state::AppState;
use crate::topology::runtime::DiagTarget;

/// Текст запроса в списке не длиннее (уточнение 6 плана P2a).
pub const QUERY_LIMIT: i32 = 16_384;

pub fn sessions_sql(caps: Caps) -> String {
    let query_id = caps.col(V14, "a.query_id", "bigint");
    format!(
        "SELECT a.pid, a.leader_pid, a.backend_type, a.datname::text, a.usename::text, a.application_name, \
         host(a.client_addr), a.client_port, {BACKEND_START_US}, \
         extract(epoch FROM now() - a.xact_start)::float8, extract(epoch FROM now() - a.query_start)::float8, \
         extract(epoch FROM now() - a.state_change)::float8, a.state, a.wait_event_type, a.wait_event, \
         a.backend_xid::text, age(a.backend_xmin)::bigint, {query_id}, left(a.query, {QUERY_LIMIT}), \
         length(a.query) > {QUERY_LIMIT}, \
         CASE WHEN a.wait_event_type = 'Lock' THEN pg_blocking_pids(a.pid) ELSE '{{}}'::int[] END, \
         COALESCE(l.n, 0)::bigint, a.application_name = 'glafira-pg' \
         FROM pg_stat_activity a \
         LEFT JOIN (SELECT pid, count(*) AS n FROM pg_locks WHERE granted GROUP BY pid) l ON l.pid = a.pid \
         WHERE a.pid <> pg_backend_pid() ORDER BY a.pid"
    )
}

/// `idle_in_transaction_session_timeout` — миллисекунды, `track_activity_query_size` — байты.
const SETTINGS_SQL: &str = "SELECT name::text, setting FROM pg_settings \
    WHERE name IN ('idle_in_transaction_session_timeout', 'track_activity_query_size')";

pub async fn collect(c: &Client, caps: Caps) -> AppResult<SessionsData> {
    let pg = |e: tokio_postgres::Error| from_pg(&e);
    let rows = c.query(&sessions_sql(caps), &[]).await.map_err(pg)?;
    let rows = rows
        .iter()
        .map(|r| SessionRow {
            pid: r.get(0),
            leader_pid: r.get(1),
            backend_type: r.get(2),
            database: r.get(3),
            user: r.get(4),
            application: r.get(5),
            client_addr: r.get(6),
            client_port: r.get(7),
            backend_start_us: r.get(8),
            xact_age_s: r.get(9),
            query_age_s: r.get(10),
            state_age_s: r.get(11),
            state: r.get(12),
            wait_event_type: r.get(13),
            wait_event: r.get(14),
            backend_xid: r.get(15),
            xmin_age: r.get(16),
            query_id: r.get(17),
            query: r.get(18),
            query_truncated: r.get::<_, Option<bool>>(19).unwrap_or(false),
            blocked_by: r.get(20),
            locks_held: r.get(21),
            glafira: r.get::<_, Option<bool>>(22).unwrap_or(false),
        })
        .collect();
    let mut settings = SessionSettings::default();
    for r in c.query(SETTINGS_SQL, &[]).await.map_err(pg)? {
        let (name, value): (String, Option<String>) = (r.get(0), r.get(1));
        let v = value.and_then(|v| v.parse::<i64>().ok());
        match name.as_str() {
            "idle_in_transaction_session_timeout" => {
                settings.idle_in_transaction_session_timeout_ms = v
            }
            _ => settings.track_activity_query_size = v,
        }
    }
    Ok(SessionsData { rows, settings })
}

pub async fn report(st: &AppState, id: &str, target: &DiagTarget) -> AppResult<DiagReport> {
    let (h, data) = on_member(st, id, target, |c| {
        Box::pin(async move {
            let h = head(c).await?;
            let data = collect(c, Caps::new(h.version_num)).await?;
            Ok((h, data))
        })
    })
    .await?;
    let member = target.member.as_ref().map(|m| m.name.as_str());
    let findings = rules::evaluate(member, &data);
    let unavailable = Caps::new(h.version_num)
        .missing(V14, "queryId")
        .into_iter()
        .collect();
    Ok(make_report(
        Tool::Sessions,
        target,
        Some(&h),
        DiagData::Sessions(data),
        findings,
        unavailable,
    ))
}
