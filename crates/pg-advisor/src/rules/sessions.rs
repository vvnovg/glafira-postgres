//! Сессии и idle in transaction (спец. P2 §5.2): строки `pg_stat_activity`, настройки и находки.

use serde::{Deserialize, Serialize};

use crate::findings::{
    Finding, FindingTarget, SessionActionLink, Severity, grade, thresholds as th,
};

/// Сессия `pg_stat_activity`. Длительности — от `now()` сервера, в секундах.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub pid: i32,
    pub leader_pid: Option<i32>,
    pub backend_type: Option<String>,
    pub database: Option<String>,
    pub user: Option<String>,
    pub application: Option<String>,
    pub client_addr: Option<String>,
    pub client_port: Option<i32>,
    /// `backend_start` в микросекундах эпохи: вместе с `pid` — ключ сессии, защищающий от переиспользования PID.
    pub backend_start_us: Option<i64>,
    pub xact_age_s: Option<f64>,
    pub query_age_s: Option<f64>,
    pub state_age_s: Option<f64>,
    pub state: Option<String>,
    pub wait_event_type: Option<String>,
    pub wait_event: Option<String>,
    pub backend_xid: Option<String>,
    pub xmin_age: Option<i64>,
    /// `query_id` — PostgreSQL 14+. В JSON — десятичная строка: 64-битный хеш не помещается в точное число JS.
    #[serde(with = "crate::serde_util::opt_i64_string", default)]
    pub query_id: Option<i64>,
    pub query: Option<String>,
    /// Текст запроса обрезан по пределу списка.
    pub query_truncated: bool,
    /// Кто блокирует сессию — только у ждущих блокировку (`wait_event_type = 'Lock'`).
    pub blocked_by: Vec<i32>,
    /// Сколько выданных блокировок держит сессия.
    pub locks_held: i64,
    /// Соединение самой Glafira (`application_name = 'glafira-pg'`).
    pub glafira: bool,
}

/// Настройки сервера, нужные вкладке: таймаут простоя в транзакции (мс) и предел текста запроса (байты).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSettings {
    pub idle_in_transaction_session_timeout_ms: Option<i64>,
    pub track_activity_query_size: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionsData {
    pub rows: Vec<SessionRow>,
    pub settings: SessionSettings,
}

/// `idle in transaction` и `idle in transaction (aborted)`.
pub fn is_idle_in_transaction(r: &SessionRow) -> bool {
    r.state
        .as_deref()
        .is_some_and(|s| s.starts_with("idle in transaction"))
}

/// Клиентская сессия не Glafira: фоновые процессы и собственные соединения находок не дают.
fn is_client(r: &SessionRow) -> bool {
    !r.glafira && r.backend_type.as_deref() == Some("client backend")
}

/// Имя в SQL всегда в кавычках (как `readiness::quote_ident`): правило «простое — как есть» ломается на ключевых словах.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Худшая строка по `value` среди подходящих и число строк не ниже порога предупреждения.
fn worst<'a>(
    rows: impl Iterator<Item = &'a SessionRow>,
    value: impl Fn(&SessionRow) -> Option<f64>,
    warn: f64,
) -> Option<(&'a SessionRow, f64, usize)> {
    let mut best: Option<(&SessionRow, f64)> = None;
    let mut count = 0;
    for r in rows {
        let Some(v) = value(r) else { continue };
        if v >= warn {
            count += 1;
        }
        if best.is_none_or(|(_, b)| v > b) {
            best = Some((r, v));
        }
    }
    best.map(|(r, v)| (r, v, count))
}

/// Одна находка на правило и узел (уточнение 2 плана P2a): `target` — худшая сессия, `count` — сколько сессий
/// достигли порога предупреждения, `maxS` — худшее значение.
fn session_finding(
    id: &str,
    member: Option<&str>,
    r: &SessionRow,
    v: f64,
    count: usize,
    (warn, crit): (f64, f64),
    tool: &str,
) -> Option<Finding> {
    let sev = grade(v, warn, crit)?;
    let mut f = Finding::new(id, sev)
        .member(member)
        .param("maxS", v)
        .param("count", count)
        .link_tool(tool, member);
    if let Some(us) = r.backend_start_us {
        f = f.target(FindingTarget::Session {
            pid: r.pid,
            backend_start_us: us,
        });
    }
    Some(f)
}

/// Находки инструмента «Сессии» на узле `member`.
pub fn evaluate(member: Option<&str>, d: &SessionsData) -> Vec<Finding> {
    let mut out = Vec::new();
    let idle = worst(
        d.rows
            .iter()
            .filter(|r| is_client(r) && is_idle_in_transaction(r)),
        |r| r.state_age_s,
        th::IDLE_IN_TRANSACTION_S.0,
    );
    if let Some((r, v, n)) = idle {
        out.extend(
            session_finding(
                "idleInTransaction",
                member,
                r,
                v,
                n,
                th::IDLE_IN_TRANSACTION_S,
                "idleTx",
            )
            .map(|f| {
                f.link_action(
                    SessionActionLink::TerminateIdleInTransaction {
                        older_than_s: th::IDLE_IN_TRANSACTION_S.0,
                    },
                    member,
                )
            }),
        );
        if v >= th::IDLE_IN_TRANSACTION_S.0
            && d.settings.idle_in_transaction_session_timeout_ms == Some(0)
        {
            let role = r
                .user
                .as_deref()
                .map(quote_ident)
                .unwrap_or_else(|| "\"<role>\"".into());
            out.push(
                Finding::new("noIdleTimeout", Severity::Info)
                    .member(member)
                    .param(
                        "command",
                        format!(
                            "ALTER ROLE {role} SET idle_in_transaction_session_timeout = '10min';"
                        ),
                    ),
            );
        }
    }
    let active = |r: &&SessionRow| is_client(r) && r.state.as_deref() == Some("active");
    if let Some((r, v, n)) = worst(
        d.rows.iter().filter(active),
        |r| r.query_age_s,
        th::LONG_QUERY_S.0,
    ) {
        out.extend(session_finding(
            "longQuery",
            member,
            r,
            v,
            n,
            th::LONG_QUERY_S,
            "sessions",
        ));
    }
    if let Some((r, v, n)) = worst(
        d.rows.iter().filter(|r| is_client(r)),
        |r| r.xact_age_s,
        th::LONG_TRANSACTION_S.0,
    ) {
        out.extend(session_finding(
            "longTransaction",
            member,
            r,
            v,
            n,
            th::LONG_TRANSACTION_S,
            "sessions",
        ));
    }
    let hidden = d
        .rows
        .iter()
        .filter(|r| r.query.as_deref() == Some("<insufficient privilege>"))
        .count();
    if hidden > 0 {
        out.push(
            Finding::new("hiddenQueries", Severity::Info)
                .member(member)
                .param("count", hidden),
        );
    }
    crate::findings::sort(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::{FindingLink, FindingTarget, SessionActionLink, Severity};

    pub(crate) fn row(pid: i32, state: &str) -> SessionRow {
        SessionRow {
            pid,
            leader_pid: None,
            backend_type: Some("client backend".into()),
            database: Some("orders".into()),
            user: Some("app".into()),
            application: Some("api".into()),
            client_addr: None,
            client_port: None,
            backend_start_us: Some(1_000 + pid as i64),
            xact_age_s: None,
            query_age_s: None,
            state_age_s: None,
            state: Some(state.into()),
            wait_event_type: None,
            wait_event: None,
            backend_xid: None,
            xmin_age: None,
            query_id: None,
            query: Some("select 1".into()),
            query_truncated: false,
            blocked_by: vec![],
            locks_held: 0,
            glafira: false,
        }
    }

    fn data(rows: Vec<SessionRow>, timeout_ms: i64) -> SessionsData {
        SessionsData {
            rows,
            settings: SessionSettings {
                idle_in_transaction_session_timeout_ms: Some(timeout_ms),
                track_activity_query_size: Some(4096),
            },
        }
    }

    fn ids(f: &[Finding]) -> Vec<(&str, Severity)> {
        f.iter().map(|x| (x.id.as_str(), x.severity)).collect()
    }

    #[test]
    fn one_idle_in_transaction_finding_per_node_with_the_worst_session() {
        let mut a = row(1, "idle in transaction");
        a.state_age_s = Some(400.0);
        let mut b = row(2, "idle in transaction (aborted)");
        b.state_age_s = Some(2000.0);
        let mut c = row(3, "idle in transaction");
        c.state_age_s = Some(10.0);
        let f = evaluate(Some("pg-1"), &data(vec![a, b, c], 60_000));
        assert_eq!(ids(&f), [("idleInTransaction", Severity::Critical)]);
        assert_eq!(
            f[0].target,
            Some(FindingTarget::Session {
                pid: 2,
                backend_start_us: 1_002
            })
        );
        assert_eq!(f[0].params["count"], 2);
        assert_eq!(f[0].params["maxS"], 2000.0);
        assert_eq!(f[0].member.as_deref(), Some("pg-1"));
        // Ссылка на инструмент остаётся первой, действие идёт вслед за ней.
        let idle = f.iter().find(|x| x.id == "idleInTransaction").unwrap();
        assert_eq!(
            idle.links,
            [
                FindingLink::OpenTool {
                    tool: "idleTx".into(),
                    member: Some("pg-1".into())
                },
                FindingLink::Action {
                    action: SessionActionLink::TerminateIdleInTransaction {
                        older_than_s: th::IDLE_IN_TRANSACTION_S.0
                    },
                    member: Some("pg-1".into()),
                },
            ]
        );
    }

    #[test]
    fn only_the_idle_in_transaction_finding_offers_an_action() {
        let mut q = row(1, "active");
        q.query_age_s = Some(1000.0);
        q.xact_age_s = Some(4000.0);
        let mut idle = row(2, "idle in transaction");
        idle.state_age_s = Some(400.0);
        let f = evaluate(Some("pg-1"), &data(vec![q, idle], 0));
        assert!(f.len() >= 4, "{f:?}");
        for x in &f {
            let has_action = x
                .links
                .iter()
                .any(|l| matches!(l, FindingLink::Action { .. }));
            assert_eq!(has_action, x.id == "idleInTransaction", "{}", x.id);
        }
    }

    #[test]
    fn long_query_and_transaction_ignore_glafira_and_background_processes() {
        let mut q = row(1, "active");
        q.query_age_s = Some(1000.0);
        q.xact_age_s = Some(4000.0);
        let mut ours = row(2, "active");
        ours.glafira = true;
        ours.query_age_s = Some(99_999.0);
        let mut vacuum = row(3, "active");
        vacuum.backend_type = Some("autovacuum worker".into());
        vacuum.query_age_s = Some(99_999.0);
        let f = evaluate(None, &data(vec![q, ours, vacuum], 60_000));
        // Одна строгость и пустой узел — порядок по id.
        assert_eq!(
            ids(&f),
            [
                ("longQuery", Severity::Warning),
                ("longTransaction", Severity::Warning)
            ]
        );
    }

    #[test]
    fn no_idle_timeout_and_hidden_queries_are_info() {
        let mut a = row(1, "idle in transaction");
        a.state_age_s = Some(301.0);
        a.user = Some("user".into());
        let mut hidden = row(2, "active");
        hidden.query = Some("<insufficient privilege>".into());
        let f = evaluate(None, &data(vec![a, hidden], 0));
        let no_timeout = f.iter().find(|x| x.id == "noIdleTimeout").unwrap();
        assert_eq!(no_timeout.severity, Severity::Info);
        assert_eq!(
            no_timeout.params["command"],
            "ALTER ROLE \"user\" SET idle_in_transaction_session_timeout = '10min';"
        );
        let hidden = f.iter().find(|x| x.id == "hiddenQueries").unwrap();
        assert_eq!(
            (hidden.severity, hidden.params["count"].as_i64()),
            (Severity::Info, Some(1))
        );
        // Таймаут задан — совета нет.
        let mut b = row(1, "idle in transaction");
        b.state_age_s = Some(301.0);
        assert!(
            evaluate(None, &data(vec![b], 600_000))
                .iter()
                .all(|x| x.id != "noIdleTimeout")
        );
    }

    #[test]
    fn query_id_goes_to_json_as_a_string() {
        // Хеш выше 2^53: числом JS округлил бы его.
        let mut a = row(1, "active");
        a.query_id = Some(i64::MAX);
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["queryId"], "9223372036854775807");
        assert_eq!(serde_json::from_value::<SessionRow>(v).unwrap(), a);
        let none = serde_json::to_value(row(2, "idle")).unwrap();
        assert!(none["queryId"].is_null());
    }
}
