//! Инструменты диагностики на стенде (спец. P2 §5): SQL каждого набора на PG 13–18 (`GPG_IT_MATRIX`) и сценарии
//! на узлах стенда. Запускать с `--test-threads=1`.

mod common;

use std::panic::{AssertUnwindSafe, resume_unwind};
use std::time::Duration;

use common::open_server;
use futures_util::FutureExt;
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

#[tokio::test]
async fn locks_sql_runs_on_every_version() {
    for addr in sql_addrs() {
        let c = common::connect_as(
            &addr,
            "postgres",
            common::PG_USER,
            common::PG_PASSWORD_VALUE,
        )
        .await;
        let h = diag::head(&c).await.unwrap();
        diag::locks::collect(&c, Caps::new(h.version_num))
            .await
            .unwrap_or_else(|e| panic!("{addr}: {e:?}"));
    }
}

/// Ожидание блокировки собирается на каждой версии: на 14+ время ожидания точное (`pg_locks.waitstart`), на PG 13
/// оценено по `state_change` и помечено `approx`. Держатель освобождается и при панике: закрытое соединение
/// простаивающей в транзакции сессии откатывает её.
#[tokio::test]
async fn lock_wait_is_collected_on_every_version_with_its_precision() {
    for addr in common::writable_addrs().await {
        common::ensure_fixtures(&addr).await;
        let holder = common::app(&addr).await;
        holder
            .batch_execute("BEGIN; LOCK TABLE shop.orders IN ACCESS EXCLUSIVE MODE;")
            .await
            .unwrap();
        let holder_pid: i32 = holder
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get(0);
        let victim = common::app(&addr).await;
        let victim_pid: i32 = victim
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get(0);
        let waiting = tokio::spawn(async move {
            victim
                .batch_execute("SELECT count(*) FROM shop.orders")
                .await
        });
        let c =
            common::connect_as(&addr, "orders", common::PG_USER, common::PG_PASSWORD_VALUE).await;
        common::wait_until("SELECT ждёт", Duration::from_secs(5), async || {
            c.query_one(
                "SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND pid = $1",
                &[&victim_pid],
            )
            .await
            .unwrap()
            .get::<_, i64>(0)
                >= 1
        })
        .await;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let h = diag::head(&c).await.unwrap();
        let rows = diag::locks::collect(&c, Caps::new(h.version_num))
            .await
            .unwrap_or_else(|e| panic!("{addr}: {e:?}"));
        holder.batch_execute("ROLLBACK").await.unwrap();
        waiting.await.unwrap().unwrap();

        let by_pid = |pid: i32| rows.iter().find(|r| r.session.pid == pid);
        let held = by_pid(holder_pid).unwrap_or_else(|| panic!("{addr}: держатель виден"));
        assert!(
            held.waiting.is_none() && held.blocked_by.is_empty(),
            "{addr}"
        );
        let row = by_pid(victim_pid).unwrap_or_else(|| panic!("{addr}: жертва видна"));
        assert_eq!(row.blocked_by, [holder_pid], "{addr}");
        let w = row.waiting.as_ref().expect("жертва ждёт");
        assert_eq!(
            (
                w.locktype.as_deref(),
                w.mode.as_deref(),
                w.relation.as_deref()
            ),
            (
                Some("relation"),
                Some("AccessShareLock"),
                Some("shop.orders")
            ),
            "{addr}"
        );
        assert!(w.wait_s.is_some_and(|s| s >= 1.0), "{addr}: {w:?}");
        assert_eq!(w.approx, h.version_num < 140_000, "{addr}");
    }
}

/// Классическая очередь за миграцией: держатель (`glafira_app`) не закончил транзакцию, `ALTER TABLE` от `postgres`
/// ждёт его, `SELECT` жертвы (`glafira_app`) встал за `ALTER`. `ALTER` внутри транзакции с `ROLLBACK`: схема не меняется.
/// Держатель освобождается при любом исходе проверок (в том числе при панике), иначе на стенде остались бы зависший
/// `ALTER` и заблокированные сессии.
#[tokio::test]
async fn ddl_queue_behind_a_long_transaction_is_found() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let holder = common::app(&addr).await;
    holder
        .batch_execute("BEGIN; UPDATE shop.orders SET status = status WHERE id = 1;")
        .await
        .unwrap();
    let mut ddl_task = None;
    let mut victim_task = None;

    let scenario = AssertUnwindSafe(async {
        let holder_pid: i32 = holder
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get(0);
        let ddl = common::superuser(&addr, "orders").await;
        let ddl_pid: i32 = ddl
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get(0);
        ddl_task = Some(tokio::spawn(async move {
            ddl.batch_execute("BEGIN; ALTER TABLE shop.orders ADD COLUMN it_tmp int; ROLLBACK;")
                .await
        }));
        let probe = common::superuser(&addr, "postgres").await;
        common::wait_until("ALTER ждёт", Duration::from_secs(5), async || {
            probe
                .query_one(
                    "SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND pid = $1",
                    &[&ddl_pid],
                )
                .await
                .unwrap()
                .get::<_, i64>(0)
                >= 1
        })
        .await;
        let victim = common::app(&addr).await;
        let victim_pid: i32 = victim
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get(0);
        victim_task = Some(tokio::spawn(async move {
            victim
                .query_one("SELECT count(*) FROM shop.orders", &[])
                .await
                .map(|_| ())
        }));
        common::wait_until("SELECT встал за ALTER", Duration::from_secs(5), async || {
            probe
                .query_one(
                    "SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND pid = $1",
                    &[&victim_pid],
                )
                .await
                .unwrap()
                .get::<_, i64>(0)
                >= 1
        })
        .await;

        // Диагностика смотрит в базу `orders`: только там `shop.orders` получает имя (см. `open_server_in`).
        let (_dir, st) = common::open_server_in(&addr, "orders").await;
        let r = diag::run(&st, "it", req(Tool::Locks)).await.unwrap();
        let DiagData::Locks(d) = &r.data else {
            panic!("{:?}", r.data)
        };
        let root = d
            .roots
            .iter()
            .find(|n| n.session.pid == holder_pid)
            .expect("держатель — корень");
        assert_eq!(
            root.children
                .iter()
                .map(|c| c.session.pid)
                .collect::<Vec<_>>(),
            [ddl_pid]
        );
        assert_eq!(
            root.children[0]
                .children
                .iter()
                .map(|c| c.session.pid)
                .collect::<Vec<_>>(),
            [victim_pid]
        );
        let q = d
            .ddl_queue
            .iter()
            .find(|q| q.session.pid == ddl_pid)
            .expect("DDL в очереди");
        assert_eq!(
            (q.mode.as_str(), q.relation.as_deref(), q.queued),
            ("AccessExclusiveLock", Some("shop.orders"), 1)
        );
        assert!(r.findings.iter().any(|f| f.id == "ddlQueue"
            && f.severity == pg_advisor::findings::Severity::Critical));

        // Из другой базы (`postgres`) имя отношения неизвестно: остаются oid и имя базы.
        let (_dir_other, other) = open_server(&addr).await;
        let DiagData::Locks(d) = diag::run(&other, "it", req(Tool::Locks)).await.unwrap().data
        else {
            panic!()
        };
        let ddl_node = &d
            .roots
            .iter()
            .find(|n| n.session.pid == holder_pid)
            .expect("держатель — корень")
            .children[0];
        let lock = ddl_node.lock.as_ref().expect("DDL ждёт блокировку");
        assert_eq!(
            (lock.relation.as_deref(), lock.relation_database.as_deref()),
            (None, Some("orders"))
        );
        assert!(lock.relation_oid.is_some());
    })
    .catch_unwind()
    .await;

    // Освобождение — всегда: после ROLLBACK держателя ALTER получает блокировку, откатывается и пропускает жертву.
    let released = holder.batch_execute("ROLLBACK").await;
    let ddl_done = match ddl_task {
        Some(t) => Some(tokio::time::timeout(Duration::from_secs(10), t).await),
        None => None,
    };
    let victim_done = match victim_task {
        Some(t) => Some(tokio::time::timeout(Duration::from_secs(10), t).await),
        None => None,
    };
    if let Err(panic) = scenario {
        resume_unwind(panic);
    }
    released.unwrap();
    ddl_done.unwrap().unwrap().unwrap().unwrap();
    victim_done.unwrap().unwrap().unwrap().unwrap();
}
