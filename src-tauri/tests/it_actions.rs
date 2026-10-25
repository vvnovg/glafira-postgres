//! Действия P2b на стенде: соединение, сессии, слоты (спец. P2 §8, §12.2). Без `GPG_IT_*` тест пропускается.

mod common;

use glafira_pg_lib::actions::conn;
use glafira_pg_lib::actions::sessions::{
    self as sa, DenyReason, SessionActionRequest, SessionRef, SessionTarget, SignalKind,
};
use glafira_pg_lib::diag::sql::BACKEND_START_US;

/// Сессия, простаивающая в транзакции: `BEGIN; SELECT 1` и ожидание. Возвращает pid; клиент держит сессию.
async fn idle_in_transaction(c: &tokio_postgres::Client) -> i32 {
    c.batch_execute("BEGIN; SELECT 1").await.unwrap();
    c.query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0)
}

/// `backend_start` сессии в микросекундах — тем же выражением, что в приложении.
async fn backend_start_us(su: &tokio_postgres::Client, pid: i32) -> i64 {
    su.query_one(
        &format!("SELECT {BACKEND_START_US} FROM pg_stat_activity a WHERE a.pid = $1"),
        &[&pid],
    )
    .await
    .unwrap()
    .get(0)
}

fn request(member: &str, target: SessionTarget) -> SessionActionRequest {
    SessionActionRequest {
        member: member.into(),
        kind: SignalKind::Terminate,
        target,
    }
}

#[tokio::test]
async fn action_connection_is_read_write_and_tagged() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let (_dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let c = conn::open(&st, "it", &member, None).await.unwrap();
    let row = c
        .client
        .query_one(
            "SELECT current_setting('default_transaction_read_only'), current_setting('application_name'), \
                    current_setting('statement_timeout'), current_setting('search_path')",
            &[],
        )
        .await
        .unwrap();
    let got: (String, String, String, String) = (row.get(0), row.get(1), row.get(2), row.get(3));
    assert_eq!(
        got,
        (
            "off".into(),
            "glafira-pg".into(),
            "5s".into(),
            "pg_catalog".into()
        )
    );
}

#[tokio::test]
async fn unknown_member_is_rejected_before_connecting() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let (_dir, st) = common::open_server(&addr).await;
    let Err(e) = conn::open(&st, "it", "no-such-node", None).await else {
        panic!("узла нет — соединения быть не должно")
    };
    assert_eq!(e.key, "errors.topology.unknownMember");
}

#[tokio::test]
async fn preview_lists_idle_sessions_with_rights() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let app = common::app(&addr).await;
    let su = common::superuser(&addr, "postgres").await;
    let app_pid = idle_in_transaction(&app).await;
    let su_pid = idle_in_transaction(&su).await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let (_dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let p = sa::preview(
        &st,
        "it",
        &SessionActionRequest {
            member: member.clone(),
            kind: SignalKind::Terminate,
            target: SessionTarget::IdleInTransaction { older_than_s: 1.0 },
        },
    )
    .await
    .unwrap();
    assert_eq!(p.member, member);
    let row = |pid| {
        p.rows
            .iter()
            .find(|r| r.session.pid == pid)
            .unwrap_or_else(|| panic!("нет {pid}: {p:?}"))
    };
    // glafira_diag — член pg_signal_backend: сессию glafira_app можно, суперпользователя — нет.
    assert!(row(app_pid).allowed, "{:?}", row(app_pid));
    assert_eq!(row(su_pid).reason, Some(DenyReason::SuperuserSession));
    assert!(!row(su_pid).allowed);
    assert_eq!(row(app_pid).session.user.as_deref(), Some("glafira_app"));
    assert_eq!(
        row(app_pid).session.backend_start_us,
        backend_start_us(&su, app_pid).await
    );
    // Выше порога — сессий нет; порог ниже возраста простоя — те же строки.
    let none = sa::preview(
        &st,
        "it",
        &request(
            &member,
            SessionTarget::IdleInTransaction {
                older_than_s: 3600.0,
            },
        ),
    )
    .await
    .unwrap();
    assert!(none.rows.iter().all(|r| r.session.pid != app_pid));
}

#[tokio::test]
async fn preview_addresses_a_session_by_pid_and_backend_start() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let app = common::app(&addr).await;
    let su = common::superuser(&addr, "postgres").await;
    let pid = idle_in_transaction(&app).await;
    let start = backend_start_us(&su, pid).await;
    let (_dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let pick = async |target| sa::preview(&st, "it", &request(&member, target)).await;
    // Верная пара — одна строка с описанием сессии.
    let ok = pick(SessionTarget::Selected {
        sessions: vec![SessionRef {
            pid,
            backend_start_us: start,
        }],
    })
    .await
    .unwrap();
    assert_eq!(ok.rows.len(), 1, "{ok:?}");
    let s = &ok.rows[0].session;
    assert_eq!((s.pid, s.backend_start_us), (pid, start));
    assert_eq!(s.state.as_deref(), Some("idle in transaction"));
    assert_eq!(s.backend_type.as_deref(), Some("client backend"));
    assert_eq!(s.database.as_deref(), Some("orders"));
    // Последний запрос сессии — вызов pid в помощнике.
    assert_eq!(s.query.as_deref(), Some("SELECT pg_backend_pid()"));
    assert!(ok.rows[0].allowed);
    // PID тот же, а время запуска другое: это уже другая сессия, в предпросмотр она не попадает.
    for target in [
        SessionTarget::Selected {
            sessions: vec![SessionRef {
                pid,
                backend_start_us: start + 1,
            }],
        },
        SessionTarget::RootBlocker {
            pid,
            backend_start_us: start + 1,
        },
    ] {
        let p = pick(target).await.unwrap();
        assert!(p.rows.is_empty(), "{p:?}");
    }
    let root = pick(SessionTarget::RootBlocker {
        pid,
        backend_start_us: start,
    })
    .await
    .unwrap();
    assert_eq!(root.rows.len(), 1);
    // Пустой выбор и несуществующий pid дают пустой предпросмотр, а не ошибку.
    assert!(
        pick(SessionTarget::Selected { sessions: vec![] })
            .await
            .unwrap()
            .rows
            .is_empty()
    );
    // Неизвестный узел отвергается до обращения к серверу.
    let e = sa::preview(
        &st,
        "it",
        &request("no-such-node", SessionTarget::Selected { sessions: vec![] }),
    )
    .await
    .unwrap_err();
    assert_eq!(e.key, "errors.topology.unknownMember");
}

/// Роль без `pg_read_all_stats` не видит `backend_start` чужих сессий: такая сессия не адресуется парой
/// `(pid, backend_start)` и в предпросмотр не попадает (без паники на пустом значении); свою роль видно.
#[tokio::test]
async fn preview_skips_sessions_the_role_cannot_see() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let app = common::app(&addr).await;
    let su = common::superuser(&addr, "postgres").await;
    let app_pid = idle_in_transaction(&app).await;
    let su_pid = idle_in_transaction(&su).await;
    let app_start = backend_start_us(&su, app_pid).await;
    let su_start = backend_start_us(&su, su_pid).await;
    let (_dir, st) =
        common::open_server_as(&addr, "orders", common::APP_USER, common::APP_PASSWORD).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let p = sa::preview(
        &st,
        "it",
        &request(
            &member,
            SessionTarget::Selected {
                sessions: vec![
                    SessionRef {
                        pid: app_pid,
                        backend_start_us: app_start,
                    },
                    SessionRef {
                        pid: su_pid,
                        backend_start_us: su_start,
                    },
                ],
            },
        ),
    )
    .await
    .unwrap();
    let pids: Vec<i32> = p.rows.iter().map(|r| r.session.pid).collect();
    assert_eq!(pids, [app_pid], "{p:?}");
    // Своя роль — можно и без pg_signal_backend.
    assert!(p.rows[0].allowed, "{:?}", p.rows[0]);
}
