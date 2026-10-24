//! Инструменты диагностики на стенде (спец. P2 §5): SQL каждого набора на PG 13–18 (`GPG_IT_MATRIX`) и сценарии
//! на узлах стенда. Запускать с `--test-threads=1`.

mod common;

use std::time::Duration;

use common::open_server;
use glafira_pg_lib::diag::caps::Caps;
use glafira_pg_lib::diag::{self, DiagData, DiagRequest, Tool};

fn req(tool: Tool) -> DiagRequest {
    DiagRequest {
        tool,
        member: None,
        window_s: None,
    }
}

/// Все узлы, на которых проверяется SQL: стенд и матрица.
fn sql_addrs() -> Vec<String> {
    [
        "GPG_IT_SINGLE",
        "GPG_IT_PRIMARY",
        "GPG_IT_REPLICA",
        "GPG_IT_LAB",
    ]
    .into_iter()
    .filter_map(common::env)
    .chain(common::matrix_addrs())
    .collect()
}

#[tokio::test]
async fn sessions_sql_runs_on_every_version() {
    for addr in sql_addrs() {
        let c = common::connect_as(
            &addr,
            "postgres",
            common::PG_USER,
            common::PG_PASSWORD_VALUE,
        )
        .await;
        let h = diag::head(&c).await.unwrap();
        let d = diag::sessions::collect(&c, Caps::new(h.version_num))
            .await
            .unwrap_or_else(|e| panic!("{addr}: {e:?}"));
        assert!(d.settings.track_activity_query_size.is_some(), "{addr}");
        assert!(
            d.rows.iter().all(|r| r
                .backend_start_us
                .is_some_and(|us| us > 1_600_000_000_000_000)),
            "{addr}"
        );
    }
}

#[tokio::test]
async fn idle_in_transaction_session_is_reported_with_its_locks() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let app = common::app(&addr).await;
    app.batch_execute("BEGIN; UPDATE shop.orders SET status = status WHERE id = 1;")
        .await
        .unwrap();
    let pid: i32 = app
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let (_dir, st) = open_server(&addr).await;
    let r = diag::run(&st, "it", req(Tool::Sessions)).await.unwrap();
    assert_eq!(
        (r.tool, r.follows_leader, r.in_recovery),
        (Tool::Sessions, true, Some(false))
    );
    let DiagData::Sessions(d) = r.data else {
        panic!("{:?}", r.data)
    };
    let row = d.rows.iter().find(|x| x.pid == pid).expect("сессия видна");
    assert_eq!(row.state.as_deref(), Some("idle in transaction"));
    assert!(row.state_age_s.unwrap() >= 1.0);
    assert!(row.locks_held >= 2, "держит блокировку строки и таблицы");
    assert_eq!(row.user.as_deref(), Some("glafira_app"));
    assert!(
        d.rows.iter().any(|x| x.glafira),
        "соединения Glafira помечены"
    );
    app.batch_execute("ROLLBACK").await.unwrap();
}

#[tokio::test]
async fn patroni_tool_on_a_server_is_refused() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let (_dir, st) = open_server(&addr).await;
    let e = diag::run(&st, "it", req(Tool::Failover)).await.unwrap_err();
    assert_eq!(e.key, "errors.diag.patroniOnly");
}

#[tokio::test]
async fn pulse_sql_runs_on_every_version() {
    for addr in sql_addrs() {
        let c = common::connect_as(
            &addr,
            "postgres",
            common::PG_USER,
            common::PG_PASSWORD_VALUE,
        )
        .await;
        let h = diag::head(&c).await.unwrap();
        let (f, snap, unavailable) = diag::pulse::collect(&c, Caps::new(h.version_num))
            .await
            .unwrap_or_else(|e| panic!("{addr}: {e:?}"));
        assert!(f.used >= 1 && f.available > f.used, "{addr}");
        assert!(f.max_xid_age > 0, "{addr}");
        assert!(
            f.wal_dir_bytes.is_some() && unavailable.is_none(),
            "{addr}: у glafira_diag есть pg_monitor"
        );
        assert!(snap.values.contains_key("commits"), "{addr}");
    }
}

/// Сценарий на каждой версии: держатель простаивает в транзакции с блокировкой таблицы, жертва ждёт её.
/// На PG 13 ожидание считается по `state_change`, на 14+ — по `pg_locks.waitstart`.
#[tokio::test]
async fn pulse_sees_idle_in_transaction_and_lock_waits_on_every_version() {
    for addr in common::writable_addrs().await {
        common::ensure_fixtures(&addr).await;
        let holder = common::app(&addr).await;
        holder
            .batch_execute("BEGIN; LOCK TABLE shop.orders IN ACCESS EXCLUSIVE MODE;")
            .await
            .unwrap();
        let victim = common::app(&addr).await;
        let waiting = tokio::spawn(async move {
            victim
                .batch_execute("SELECT count(*) FROM shop.orders")
                .await
        });
        tokio::time::sleep(Duration::from_millis(1500)).await;

        let c = common::connect_as(
            &addr,
            "postgres",
            common::PG_USER,
            common::PG_PASSWORD_VALUE,
        )
        .await;
        let h = diag::head(&c).await.unwrap();
        let (f, _, _) = diag::pulse::collect(&c, Caps::new(h.version_num))
            .await
            .unwrap_or_else(|e| panic!("{addr}: {e:?}"));
        holder.batch_execute("ROLLBACK").await.unwrap();
        waiting.await.unwrap().unwrap();

        assert!(f.by_state.idle_in_transaction >= 1, "{addr}: {f:?}");
        assert!(
            f.longest_idle_in_transaction_s.is_some_and(|s| s >= 1.0),
            "{addr}: {f:?}"
        );
        assert!(f.lock_waiting >= 1, "{addr}: {f:?}");
        assert!(
            f.longest_lock_wait_s.is_some_and(|s| s >= 1.0),
            "{addr}: {f:?}"
        );
        assert!(
            f.longest_transaction_s.is_some_and(|s| s >= 1.0),
            "{addr}: {f:?}"
        );
    }
}

#[tokio::test]
async fn pulse_on_patroni_has_a_row_per_member_and_rates_from_the_second_call() {
    let Some((_dir, st)) = common::open_patroni().await else {
        return;
    };
    let req = DiagRequest {
        tool: Tool::Pulse,
        member: Some("ignored".into()),
        window_s: None,
    };
    let first = diag::run(&st, "it", req.clone()).await.unwrap();
    assert_eq!((first.member, first.follows_leader), (None, false));
    let DiagData::Pulse(d) = first.data else {
        panic!()
    };
    assert_eq!(d.nodes.len(), 3);
    assert!(
        d.nodes
            .iter()
            .all(|n| n.facts.is_some() && n.error.is_none()),
        "{:?}",
        d.nodes
    );
    assert!(
        d.nodes
            .iter()
            .all(|n| n.rates.as_ref().unwrap().tps.is_none()),
        "первый вызов — без скоростей"
    );
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let DiagData::Pulse(d) = diag::run(&st, "it", req).await.unwrap().data else {
        panic!()
    };
    assert!(
        d.nodes
            .iter()
            .all(|n| n.rates.as_ref().unwrap().tps.is_some())
    );
}
