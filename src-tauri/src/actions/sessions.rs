//! Отмена запроса и завершение сессий (спец. P2 §8.3): предпросмотр (задача 3) и выполнение (задача 4).

use serde::{Deserialize, Serialize};
use tokio_postgres::Client;

use crate::diag::sql::BACKEND_START_US;
use crate::error::AppResult;
use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SignalKind {
    Cancel,
    Terminate,
}

/// Сессия адресуется парой `(pid, backend_start)`: PID переиспользуется, время запуска — нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRef {
    pub pid: i32,
    pub backend_start_us: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionTarget {
    Selected {
        sessions: Vec<SessionRef>,
    },
    #[serde(rename_all = "camelCase")]
    IdleInTransaction {
        older_than_s: f64,
    },
    #[serde(rename_all = "camelCase")]
    RootBlocker {
        pid: i32,
        backend_start_us: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActionRequest {
    pub member: String,
    pub kind: SignalKind,
    pub target: SessionTarget,
}

/// Почему сессию нельзя отменить или завершить (видно в строке предпросмотра и подсказке кнопки).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DenyReason {
    /// Сессия суперпользователя: нужна роль суперпользователя.
    SuperuserSession,
    /// Чужая роль и нет `pg_signal_backend`.
    OtherRole,
    NotClientBackend,
    /// Соединение самого приложения.
    OwnConnection,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSession {
    pub pid: i32,
    pub backend_start_us: i64,
    pub user: Option<String>,
    pub database: Option<String>,
    pub application: Option<String>,
    pub client: Option<String>,
    pub state: Option<String>,
    pub backend_type: Option<String>,
    pub xact_age_s: Option<f64>,
    pub state_age_s: Option<f64>,
    /// Первые 500 символов текста запроса.
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRow {
    pub session: PreviewSession,
    pub allowed: bool,
    pub reason: Option<DenyReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActionPreview {
    /// Узел, к которому относится предпросмотр: выполнение принимает именно его.
    pub member: String,
    pub rows: Vec<PreviewRow>,
}

/// Факты прав для строки предпросмотра (из того же запроса).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivFacts {
    pub client: bool,
    pub own: bool,
    pub session_super: bool,
    pub same_role: bool,
    pub me_super: bool,
    pub signal: bool,
}

/// Правило PostgreSQL для `pg_cancel_backend` / `pg_terminate_backend`: суперпользователь — любую клиентскую сессию;
/// член роли сессии — её; член `pg_signal_backend` — любую сессию не суперпользователя.
pub fn deny_reason(f: &PrivFacts) -> Option<DenyReason> {
    if !f.client {
        return Some(DenyReason::NotClientBackend);
    }
    if f.own {
        return Some(DenyReason::OwnConnection);
    }
    if f.me_super || f.same_role {
        return None;
    }
    if f.session_super {
        return Some(DenyReason::SuperuserSession);
    }
    if f.signal {
        None
    } else {
        Some(DenyReason::OtherRole)
    }
}

/// Условие выбора строк по цели: `$1` — массив pid (`selected`), порог в секундах (`idleInTransaction`) или pid.
fn target_filter(target: &SessionTarget) -> &'static str {
    match target {
        SessionTarget::Selected { .. } => "a.pid = ANY($1)",
        SessionTarget::IdleInTransaction { .. } => {
            "a.backend_type = 'client backend' AND a.state IN ('idle in transaction', 'idle in transaction (aborted)') \
             AND a.state_change <= now() - make_interval(secs => $1)"
        }
        SessionTarget::RootBlocker { .. } => "a.pid = $1",
    }
}

pub fn preview_sql(target: &SessionTarget) -> String {
    format!(
        "SELECT a.pid, {BACKEND_START_US}, a.usename::text, a.datname::text, a.application_name, host(a.client_addr), \
                a.state, a.backend_type, extract(epoch FROM now() - a.xact_start)::float8, \
                extract(epoch FROM now() - a.state_change)::float8, left(a.query, 500), \
                a.backend_type = 'client backend', a.pid = pg_backend_pid(), COALESCE(r.rolsuper, false), \
                COALESCE(pg_has_role(current_user, a.usesysid, 'MEMBER'), false), \
                (SELECT rolsuper FROM pg_roles WHERE rolname = current_user), \
                pg_has_role(current_user, 'pg_signal_backend', 'MEMBER') \
         FROM pg_stat_activity a LEFT JOIN pg_roles r ON r.oid = a.usesysid \
         WHERE {} ORDER BY a.pid",
        target_filter(target)
    )
}

async fn query_preview(c: &Client, target: &SessionTarget) -> AppResult<Vec<PreviewRow>> {
    let sql = preview_sql(target);
    let rows = match target {
        SessionTarget::Selected { sessions } => {
            let pids: Vec<i32> = sessions.iter().map(|s| s.pid).collect();
            c.query(&sql, &[&pids]).await
        }
        SessionTarget::IdleInTransaction { older_than_s } => c.query(&sql, &[older_than_s]).await,
        SessionTarget::RootBlocker { pid, .. } => c.query(&sql, &[pid]).await,
    }
    .map_err(|e| crate::conn::errors::from_pg(&e))?;
    let wanted = |pid: i32, start: i64| match target {
        SessionTarget::Selected { sessions } => sessions
            .iter()
            .any(|s| s.pid == pid && s.backend_start_us == start),
        SessionTarget::IdleInTransaction { .. } => true,
        SessionTarget::RootBlocker {
            pid: p,
            backend_start_us,
        } => *p == pid && *backend_start_us == start,
    };
    Ok(rows
        .iter()
        .filter_map(|r| {
            // Чужую сессию без права её видеть (нет `pg_read_all_stats`) сервер отдаёт с пустым `backend_start`:
            // адресовать её парой (pid, backend_start) нельзя, в предпросмотр она не попадает.
            let start = r.get::<_, Option<i64>>(1)?;
            if !wanted(r.get(0), start) {
                return None;
            }
            let f = PrivFacts {
                client: r.get(11),
                own: r.get(12),
                session_super: r.get(13),
                same_role: r.get(14),
                me_super: r.get::<_, Option<bool>>(15).unwrap_or(false),
                signal: r.get(16),
            };
            let reason = deny_reason(&f);
            Some(PreviewRow {
                session: PreviewSession {
                    pid: r.get(0),
                    backend_start_us: start,
                    user: r.get(2),
                    database: r.get(3),
                    application: r.get(4),
                    client: r.get(5),
                    state: r.get(6),
                    backend_type: r.get(7),
                    xact_age_s: r.get(8),
                    state_age_s: r.get(9),
                    query: r.get(10),
                },
                allowed: reason.is_none(),
                reason,
            })
        })
        .collect())
}

/// Заново читает сессии цели на узле `member` (соединение каталога: предпросмотр ничего не меняет).
pub async fn preview(
    st: &AppState,
    id: &str,
    req: &SessionActionRequest,
) -> AppResult<SessionActionPreview> {
    let target = st
        .runtimes
        .diag_target(id, Some(req.member.as_str()), false)?;
    let t = req.target.clone();
    let rows = crate::diag::on_member(st, id, &target, move |c| {
        Box::pin(async move { query_preview(c, &t).await })
    })
    .await?;
    Ok(SessionActionPreview {
        member: req.member.clone(),
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(
        client: bool,
        own: bool,
        session_super: bool,
        same_role: bool,
        me_super: bool,
        signal: bool,
    ) -> PrivFacts {
        PrivFacts {
            client,
            own,
            session_super,
            same_role,
            me_super,
            signal,
        }
    }

    #[test]
    fn privilege_rules_follow_postgres() {
        // Суперпользователь может всё, кроме чужого фонового процесса и своего соединения.
        assert_eq!(
            deny_reason(&facts(true, false, true, false, true, false)),
            None
        );
        // Своя роль — можно и без pg_signal_backend.
        assert_eq!(
            deny_reason(&facts(true, false, false, true, false, false)),
            None
        );
        // pg_signal_backend — только сессии не суперпользователя.
        assert_eq!(
            deny_reason(&facts(true, false, false, false, false, true)),
            None
        );
        assert_eq!(
            deny_reason(&facts(true, false, true, false, false, true)),
            Some(DenyReason::SuperuserSession)
        );
        assert_eq!(
            deny_reason(&facts(true, false, false, false, false, false)),
            Some(DenyReason::OtherRole)
        );
        assert_eq!(
            deny_reason(&facts(false, false, false, false, true, true)),
            Some(DenyReason::NotClientBackend)
        );
        assert_eq!(
            deny_reason(&facts(true, true, false, true, true, true)),
            Some(DenyReason::OwnConnection)
        );
    }

    #[test]
    fn preview_sql_filters_by_target() {
        let sel = preview_sql(&SessionTarget::Selected { sessions: vec![] });
        assert!(sel.contains("a.pid = ANY($1)"));
        let idle = preview_sql(&SessionTarget::IdleInTransaction {
            older_than_s: 300.0,
        });
        assert!(
            idle.contains("a.state IN ('idle in transaction', 'idle in transaction (aborted)')")
        );
        assert!(idle.contains("make_interval(secs => $1)"));
        assert!(idle.contains("a.backend_type = 'client backend'"));
        let root = preview_sql(&SessionTarget::RootBlocker {
            pid: 1,
            backend_start_us: 2,
        });
        assert!(root.contains("a.pid = $1"));
        for s in [&sel, &idle, &root] {
            assert!(
                s.contains(crate::diag::sql::BACKEND_START_US),
                "одно выражение backend_start во всех запросах"
            );
            assert!(s.contains("pg_has_role(current_user, 'pg_signal_backend', 'MEMBER')"));
        }
    }

    #[test]
    fn request_json_is_camel_case() {
        let r: SessionActionRequest = serde_json::from_value(serde_json::json!({
            "member": "pg-1", "kind": "terminate",
            "target": {"kind": "rootBlocker", "pid": 42, "backendStartUs": 1_761_299_000_000_000_i64}
        }))
        .unwrap();
        assert_eq!(r.kind, SignalKind::Terminate);
        assert_eq!(
            r.target,
            SessionTarget::RootBlocker {
                pid: 42,
                backend_start_us: 1_761_299_000_000_000
            }
        );
        let idle: SessionTarget = serde_json::from_value(
            serde_json::json!({"kind": "idleInTransaction", "olderThanS": 300}),
        )
        .unwrap();
        assert_eq!(
            idle,
            SessionTarget::IdleInTransaction {
                older_than_s: 300.0
            }
        );
    }
}
