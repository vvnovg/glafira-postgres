//! Инструменты диагностики на стенде (спец. P2 §5): SQL каждого набора на PG 13–18 (`GPG_IT_MATRIX`) и сценарии
//! на узлах стенда. Запускать с `--test-threads=1`.

mod common;

use std::panic::{AssertUnwindSafe, resume_unwind};
use std::time::Duration;

use common::open_server;
use futures_util::FutureExt;
use glafira_pg_lib::diag::caps::Caps;
use glafira_pg_lib::diag::{self, DiagData, DiagRequest, Tool};
use glafira_pg_lib::topology::model::Topology;
use pg_advisor::rules::horizon::HolderKind;

fn req(tool: Tool) -> DiagRequest {
    DiagRequest {
        tool,
        member: None,
        window_s: None,
    }
}

/// Находка относится к сессии `pid`.
fn is_session(f: &pg_advisor::findings::Finding, pid: i32) -> bool {
    matches!(
        f.target,
        Some(pg_advisor::findings::FindingTarget::Session { pid: p, .. }) if p == pid
    )
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
        common::wait_lock_wait(&c, victim_pid, "SELECT ждёт").await;
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
        common::wait_lock_wait(&probe, ddl_pid, "ALTER ждёт").await;
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
        common::wait_lock_wait(&probe, victim_pid, "SELECT встал за ALTER").await;

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
        let found = r
            .findings
            .iter()
            .find(|f| f.id == "ddlQueue" && is_session(f, ddl_pid))
            .expect("находка ddlQueue");
        assert_eq!(found.severity, pg_advisor::findings::Severity::Critical);
        assert_eq!(found.params["relation"], "shop.orders");
        assert!(
            !found.params.contains_key("relationOid")
                && !found.params.contains_key("relationDatabase"),
            "{:?}",
            found.params
        );

        // Из другой базы (`postgres`) имя отношения неизвестно: остаются oid и имя базы.
        let (_dir_other, other) = open_server(&addr).await;
        let r = diag::run(&other, "it", req(Tool::Locks)).await.unwrap();
        let DiagData::Locks(d) = &r.data else {
            panic!("{:?}", r.data)
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
        // В находке — без пустого имени: oid и база, из которых интерфейс составит «oid N в БД orders».
        let found = r
            .findings
            .iter()
            .find(|f| f.id == "ddlQueue" && is_session(f, ddl_pid))
            .expect("находка ddlQueue");
        assert!(!found.params.contains_key("relation"), "{:?}", found.params);
        assert_eq!(found.params["relationOid"].as_i64(), lock.relation_oid);
        assert_eq!(found.params["relationDatabase"], "orders");
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

#[tokio::test]
async fn horizon_and_replication_sql_run_on_every_version() {
    for addr in sql_addrs() {
        let c = common::connect_as(
            &addr,
            "postgres",
            common::PG_USER,
            common::PG_PASSWORD_VALUE,
        )
        .await;
        let h = diag::head(&c).await.unwrap();
        let caps = Caps::new(h.version_num);
        let empty = Topology::empty_for_tests();
        let horizon = diag::horizon::collect(&c, caps, &empty)
            .await
            .unwrap_or_else(|e| panic!("{addr}: {e:?}"));
        assert!(
            horizon.hot_standby_feedback.is_some(),
            "{addr}: настройка прочитана"
        );
        let (replication, snapshot) = diag::replication::collect(&c, caps, h.in_recovery, &empty)
            .await
            .unwrap_or_else(|e| panic!("{addr}: {e:?}"));
        assert_eq!(replication.in_recovery, h.in_recovery, "{addr}");
        assert_eq!(
            (
                replication.receiver.is_some(),
                replication.replay.is_some(),
                replication.conflicts.is_some(),
                snapshot.is_some()
            ),
            (h.in_recovery, h.in_recovery, h.in_recovery, h.in_recovery),
            "{addr}: приёмник, replay и конфликты — только на узле в recovery"
        );
        assert!(
            replication.synchronous_commit.is_some() && replication.hot_standby_feedback.is_some(),
            "{addr}: {replication:?}"
        );
        assert!(
            replication.max_standby_streaming_delay_ms.is_some(),
            "{addr}"
        );
    }
}

/// Пауза replay видна с обеих сторон: на primary отправитель отстаёт в байтах, на реплике приёмник жив, а replay на
/// паузе. Стенд возвращается при любом выходе: replay возобновлён, таблица `it_lag` удалена (порядок восстановления —
/// обратный порядку объявления).
#[tokio::test]
async fn paused_replay_shows_on_both_sides() {
    let (Some(primary), Some(replica)) =
        (common::env("GPG_IT_PRIMARY"), common::env("GPG_IT_REPLICA"))
    else {
        return;
    };
    let _drop_table = common::restore_on_drop({
        let primary = primary.clone();
        move || common::run_sql_blocking(&primary, "postgres", "DROP TABLE IF EXISTS it_lag")
    });
    let _resume = common::restore_on_drop({
        let replica = replica.clone();
        move || common::run_sql_blocking(&replica, "postgres", "SELECT pg_wal_replay_resume()")
    });
    common::superuser(&replica, "postgres")
        .await
        .batch_execute("SELECT pg_wal_replay_pause()")
        .await
        .unwrap();
    let w = common::superuser(&primary, "postgres").await;
    w.batch_execute(
        "CREATE TABLE IF NOT EXISTS it_lag (x int); INSERT INTO it_lag SELECT generate_series(1, 200000);",
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let (_d1, st) = open_server(&primary).await;
    let r = diag::run(&st, "it", req(Tool::Replication)).await.unwrap();
    let DiagData::Replication(d) = r.data else {
        panic!()
    };
    assert!(!d.in_recovery);
    assert!(
        d.senders
            .iter()
            .any(|s| s.replay_lag_bytes.unwrap_or(0) > 0),
        "{:?}",
        d.senders
    );

    let (_d2, st) = open_server(&replica).await;
    let r = diag::run(&st, "it", req(Tool::Replication)).await.unwrap();
    let DiagData::Replication(d) = &r.data else {
        panic!()
    };
    assert!(d.in_recovery);
    assert_eq!(
        d.receiver.as_ref().and_then(|x| x.status.as_deref()),
        Some("streaming")
    );
    assert!(matches!(
        d.replay.as_ref().and_then(|x| x.pause.as_deref()),
        Some("paused" | "pause requested")
    ));
    assert!(r.findings.iter().any(|f| f.id == "replayPaused"));
}

/// Prepared transaction и логический слот лаборатории держат горизонт. Prepared transaction откатывается при любом
/// выходе: `ROLLBACK PREPARED` нельзя выполнить из блока `DO`, поэтому — `rollback_prepared_blocking`.
#[tokio::test]
async fn prepared_transaction_and_logical_slot_hold_the_lab_horizon() {
    let Some(lab) = common::lab_addr() else {
        return;
    };
    let _cleanup = common::restore_on_drop({
        let lab = lab.clone();
        move || common::rollback_prepared_blocking(&lab, "orders", "it_horizon")
    });
    let c = common::superuser(&lab, "orders").await;
    c.batch_execute(
        "BEGIN; UPDATE shop.customers SET email = email WHERE id = 1; PREPARE TRANSACTION 'it_horizon';",
    )
    .await
    .unwrap();
    let (_dir, st) = open_server(&lab).await;
    let r = diag::run(&st, "it", req(Tool::Horizon)).await.unwrap();
    let DiagData::Horizon(d) = r.data else {
        panic!()
    };
    assert!(
        d.holders
            .iter()
            .any(|h| h.kind == HolderKind::PreparedXact && h.gid.as_deref() == Some("it_horizon")),
        "{:?}",
        d.holders
    );
    assert!(
        d.holders
            .iter()
            .any(|h| h.kind == HolderKind::Slot && h.name == "lab_sub_slot" && h.catalog),
        "{:?}",
        d.holders
    );
    assert!(
        d.holders.windows(2).all(|w| w[0].xmin_age >= w[1].xmin_age),
        "по возрасту"
    );
}

/// Откат prepared transaction для восстановления стенда: повторный вызов, когда транзакции уже нет, не падает.
#[tokio::test]
async fn rollback_prepared_helper_is_idempotent() {
    let Some(lab) = common::lab_addr() else {
        return;
    };
    let _cleanup = common::restore_on_drop({
        let lab = lab.clone();
        move || common::rollback_prepared_blocking(&lab, "orders", "it_helper")
    });
    let c = common::superuser(&lab, "orders").await;
    c.batch_execute(
        "BEGIN; UPDATE shop.customers SET email = email WHERE id = 1; PREPARE TRANSACTION 'it_helper';",
    )
    .await
    .unwrap();
    let left = || async {
        c.query_one(
            "SELECT count(*) FROM pg_prepared_xacts WHERE gid = 'it_helper'",
            &[],
        )
        .await
        .unwrap()
        .get::<_, i64>(0)
    };
    assert_eq!(left().await, 1);
    common::rollback_prepared_blocking(&lab, "orders", "it_helper");
    assert_eq!(left().await, 0);
    common::rollback_prepared_blocking(&lab, "orders", "it_helper");
}

/// Сессия с назначенным XID держит горизонт: видна по `pid` и `backend_start`, держатель — клиентская транзакция.
#[tokio::test]
async fn a_session_with_an_xid_holds_the_horizon() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let app = common::app(&addr).await;
    app.batch_execute("BEGIN; SELECT txid_current();")
        .await
        .unwrap();
    let pid: i32 = app
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    tokio::time::sleep(Duration::from_millis(1200)).await;

    let (_dir, st) = open_server(&addr).await;
    let r = diag::run(&st, "it", req(Tool::Horizon)).await.unwrap();
    assert_eq!((r.tool, r.in_recovery), (Tool::Horizon, Some(false)));
    let DiagData::Horizon(d) = r.data else {
        panic!()
    };
    let h = d
        .holders
        .iter()
        .find(|h| h.kind == HolderKind::Session && h.pid == Some(pid))
        .unwrap_or_else(|| panic!("сессия {pid} не держатель: {:?}", d.holders));
    assert_eq!(h.name, pid.to_string());
    assert_eq!(h.user.as_deref(), Some("glafira_app"));
    assert_eq!(h.database.as_deref(), Some("orders"));
    assert!(
        h.backend_start_us
            .is_some_and(|us| us > 1_600_000_000_000_000)
    );
    assert!(h.duration_s.is_some_and(|s| s >= 1.0), "{h:?}");
    app.batch_execute("ROLLBACK").await.unwrap();
}

/// Обратная связь реплики (`hot_standby_feedback`) держит горизонт primary. Реплика стенда подключена через
/// физический слот `replica1`: тогда xmin записывается в слот (`pg_replication_slots.xmin`), а
/// `pg_stat_replication.backend_xmin` пуст — держатель виден как слот. Без слота он был бы `StandbyFeedback`.
/// Настройка возвращается при любом выходе: `ALTER SYSTEM RESET` и reload — отдельными командами.
#[tokio::test]
async fn standby_feedback_is_a_holder_on_the_primary() {
    let (Some(primary), Some(replica)) =
        (common::env("GPG_IT_PRIMARY"), common::env("GPG_IT_REPLICA"))
    else {
        return;
    };
    common::ensure_fixtures(&primary).await;
    let _reset = common::restore_on_drop({
        let replica = replica.clone();
        move || {
            common::run_sql_blocking(
                &replica,
                "postgres",
                "ALTER SYSTEM RESET hot_standby_feedback",
            );
            common::run_sql_blocking(&replica, "postgres", "SELECT pg_reload_conf()");
        }
    });
    let su = common::superuser(&replica, "postgres").await;
    su.batch_execute("ALTER SYSTEM SET hot_standby_feedback = on")
        .await
        .unwrap();
    su.batch_execute("SELECT pg_reload_conf()").await.unwrap();
    let reader = common::app(&replica).await;
    reader
        .batch_execute("BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT count(*) FROM shop.orders;")
        .await
        .unwrap();
    let probe = common::superuser(&primary, "postgres").await;
    common::wait_until(
        "feedback дошёл до primary",
        Duration::from_secs(20),
        async || {
            probe
                .query_one(
                    "SELECT (SELECT count(*) FROM pg_replication_slots WHERE xmin IS NOT NULL) \
                      + (SELECT count(*) FROM pg_stat_replication WHERE backend_xmin IS NOT NULL)",
                    &[],
                )
                .await
                .unwrap()
                .get::<_, i64>(0)
                >= 1
        },
    )
    .await;
    let (_dir, st) = open_server(&primary).await;
    let DiagData::Horizon(d) = diag::run(&st, "it", req(Tool::Horizon)).await.unwrap().data else {
        panic!()
    };
    let holder = d
        .holders
        .iter()
        .find(|h| match h.kind {
            HolderKind::StandbyFeedback => true,
            HolderKind::Slot => {
                h.slot_type.as_deref() == Some("physical") && h.active == Some(true) && !h.catalog
            }
            _ => false,
        })
        .unwrap_or_else(|| panic!("{:?}", d.holders));
    // Держатель — отправитель WAL, то есть сама реплика: у слота реплики известны приложение и адрес клиента, чтобы
    // находка вела к реплике, а не к слоту. Имя приложения стенда (`walreceiver`) не совпадает ни с именем, ни с
    // хостом единственного узла списка, поэтому участник не сопоставляется (сопоставление проверяют юнит-тесты).
    assert!(
        holder.application.as_deref().is_some_and(|a| !a.is_empty()),
        "{holder:?}"
    );
    assert!(holder.client_addr.is_some(), "{holder:?}");
    assert_eq!(holder.member, None, "{holder:?}");
    // Горизонт на самой реплике: её настройка включена, а читающая транзакция видна как сессия.
    let (_dir_replica, st) = open_server(&replica).await;
    let DiagData::Horizon(d) = diag::run(&st, "it", req(Tool::Horizon)).await.unwrap().data else {
        panic!()
    };
    assert_eq!(d.hot_standby_feedback, Some(true));
    assert!(
        d.holders
            .iter()
            .any(|h| h.kind == HolderKind::Session && h.user.as_deref() == Some("glafira_app")),
        "{:?}",
        d.holders
    );
    reader.batch_execute("ROLLBACK").await.unwrap();
}

/// Без `pg_read_all_stats` сервер прячет у чужих сессий состояние приёмника и отправителей WAL. Сборщик это замечает:
/// данные помечены `detailsHidden`, пустые поля не превращаются в `walReceiverDown` и прочие ложные находки, а отчёт
/// говорит, какой части не хватает. С `pg_monitor` (роль диагностики) ничего не скрыто. Роль `glafira_app` реплицируется
/// на реплику вместе с остальными ролями primary.
#[tokio::test]
async fn hidden_statistics_are_reported_not_turned_into_findings() {
    let (Some(primary), Some(replica)) =
        (common::env("GPG_IT_PRIMARY"), common::env("GPG_IT_REPLICA"))
    else {
        return;
    };
    common::ensure_fixtures(&primary).await;
    let probe = common::superuser(&replica, "postgres").await;
    common::wait_until(
        "роль glafira_app дошла до реплики",
        Duration::from_secs(20),
        async || {
            probe
                .query_one(
                    "SELECT count(*) FROM pg_roles WHERE rolname = 'glafira_app'",
                    &[],
                )
                .await
                .unwrap()
                .get::<_, i64>(0)
                == 1
        },
    )
    .await;
    let false_alarms = [
        "walReceiverDown",
        "replicaNotStreaming",
        "syncStandbyMissing",
        "replicaReplyStale",
    ];
    let empty = Topology::empty_for_tests();
    for addr in [&primary, &replica] {
        // Сборщик: роль приложения ничего не видит, роль диагностики видит всё.
        let app =
            common::connect_as(addr, "postgres", common::APP_USER, common::APP_PASSWORD).await;
        let h = diag::head(&app).await.unwrap();
        let (d, _) =
            diag::replication::collect(&app, Caps::new(h.version_num), h.in_recovery, &empty)
                .await
                .unwrap_or_else(|e| panic!("{addr}: {e:?}"));
        assert!(d.details_hidden, "{addr}: {d:?}");
        if h.in_recovery {
            assert_eq!(
                d.receiver.as_ref().and_then(|r| r.status.as_deref()),
                None,
                "{addr}: состояние приёмника скрыто"
            );
        }
        let found = pg_advisor::rules::replication::evaluate(None, &d);
        assert!(
            found.iter().all(|f| !false_alarms.contains(&f.id.as_str())),
            "{addr}: {found:?}"
        );
        let diag_role =
            common::connect_as(addr, "postgres", common::PG_USER, common::PG_PASSWORD_VALUE).await;
        let (d, _) =
            diag::replication::collect(&diag_role, Caps::new(h.version_num), h.in_recovery, &empty)
                .await
                .unwrap_or_else(|e| panic!("{addr}: {e:?}"));
        assert!(!d.details_hidden, "{addr}: {d:?}");
        if h.in_recovery {
            assert_eq!(
                d.receiver.as_ref().and_then(|r| r.status.as_deref()),
                Some("streaming"),
                "{addr}"
            );
        }

        // Отчёт: без права — пометка о недостающей части и ни одной ложной находки; с правом — пометок нет.
        let (_dir, st) =
            common::open_server_as(addr, "postgres", common::APP_USER, common::APP_PASSWORD).await;
        let r = diag::run(&st, "it", req(Tool::Replication)).await.unwrap();
        let hidden = r
            .unavailable
            .iter()
            .find(|u| u.part == "replicationDetails");
        assert_eq!(
            hidden.map(|u| u.reason.key.as_str()),
            Some("diag.unavailable.privilege"),
            "{addr}: {:?}",
            r.unavailable
        );
        assert!(
            r.findings
                .iter()
                .all(|f| !false_alarms.contains(&f.id.as_str())),
            "{addr}: {:?}",
            r.findings
        );
        let r = diag::run(&st, "it", req(Tool::Horizon)).await.unwrap();
        assert!(
            r.unavailable.iter().any(|u| u.part == "otherSessionsXmin"),
            "{addr}: {:?}",
            r.unavailable
        );
        let (_dir, st) = open_server(addr).await;
        for tool in [Tool::Replication, Tool::Horizon] {
            let r = diag::run(&st, "it", req(tool)).await.unwrap();
            assert!(
                r.unavailable
                    .iter()
                    .all(|u| u.reason.key != "diag.unavailable.privilege"),
                "{addr} {tool:?}: {:?}",
                r.unavailable
            );
        }
    }
}

/// Рост конфликтов recovery — по двум отчётам одного узла: первый роста не знает, второй видит прирост (на спокойной
/// реплике — ноль).
#[tokio::test]
async fn conflict_growth_appears_with_the_second_report() {
    let Some(replica) = common::env("GPG_IT_REPLICA") else {
        return;
    };
    let (_dir, st) = open_server(&replica).await;
    let growth = |r: diag::DiagReport| match r.data {
        DiagData::Replication(d) => d.conflicts.expect("конфликты на реплике").growth,
        other => panic!("{other:?}"),
    };
    let first = diag::run(&st, "it", req(Tool::Replication)).await.unwrap();
    assert_eq!(growth(first), None);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let second = diag::run(&st, "it", req(Tool::Replication)).await.unwrap();
    assert_eq!(growth(second), Some(0));
}
