//! hypopg на pg-perf (`GPG_IT_PERF`): индекс-гипотеза снижает стоимость и используется планом; сессия после проверки
//! чиста; без расширения — FEATURE_UNAVAILABLE. Запускать с `--test-threads=1`.

mod common;

use glafira_pg_lib::error::ErrorCode;
use glafira_pg_lib::explain::Params;
use glafira_pg_lib::explain::hypopg::{self, HypoRequest};

#[tokio::test]
async fn hypothetical_index_changes_the_plan_and_leaves_no_trace() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let m = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let req = HypoRequest {
        member: m.clone(),
        database: "perf".into(),
        sql: "SELECT count(*) FROM shop.orders WHERE status = $1".into(),
        params: Params::Values {
            values: vec![Some("disputed".into())],
        },
        search_path: None,
        index_ddl: "CREATE INDEX ON shop.orders (status)".into(),
    };
    let r = hypopg::check(&st, "it", &req).await.unwrap();
    assert!(
        r.used,
        "план после гипотезы использует индекс: {}",
        r.plan_after.root.node_type
    );
    assert!(
        r.cost_after < r.cost_before,
        "{} < {}",
        r.cost_after,
        r.cost_before
    );
    assert!(
        r.estimated_bytes > 0 && r.index_name.contains("orders"),
        "{r:?}"
    );
    // Сессия чиста: виртуальных индексов в новом соединении нет (они живут в сессии, а она закрыта).
    let su = common::superuser(&perf, "perf").await;
    let n: i64 = su
        .query_one("SELECT count(*) FROM hypopg_list_indexes", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(n, 0);
    let bad = HypoRequest {
        index_ddl: "CREATE INDEX CONCURRENTLY ON shop.orders (status)".into(),
        ..req.clone()
    };
    assert_eq!(
        hypopg::check(&st, "it", &bad).await.unwrap_err().code,
        ErrorCode::InvalidInput
    );
    // Неизвестная колонка — сервер отвергает гипотезу, это ошибка ввода.
    let unknown = HypoRequest {
        index_ddl: "CREATE INDEX ON shop.orders (no_such_column)".into(),
        ..req.clone()
    };
    let e = hypopg::check(&st, "it", &unknown).await.unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str()),
        (ErrorCode::InvalidInput, "errors.hypo.rejected"),
        "{e:?}"
    );
    // База без hypopg (postgres) — недоступно.
    let none = HypoRequest {
        database: "postgres".into(),
        sql: "SELECT 1".into(),
        params: Params::None,
        ..req
    };
    let e = hypopg::check(&st, "it", &none).await.unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str()),
        (ErrorCode::FeatureUnavailable, "errors.hypo.notInstalled")
    );
}
