//! hypopg на pg-perf (`GPG_IT_PERF`): индекс-гипотеза снижает стоимость и используется планом; в сессии после проверки
//! виртуальных индексов нет — и после успеха, и после ошибки пробы «после»; `search_path` запроса действует и на
//! гипотезу; без расширения — FEATURE_UNAVAILABLE. Запускать с `--test-threads=1`.

mod common;

use glafira_pg_lib::error::ErrorCode;
use glafira_pg_lib::explain::Params;
use glafira_pg_lib::explain::hypopg::{self, HypoRequest};
use pg_advisor::plan::relations;
use tokio_postgres::Client;

async fn hypo_count(c: &Client) -> i64 {
    c.query_one("SELECT count(*) FROM hypopg_list_indexes", &[])
        .await
        .unwrap()
        .get(0)
}

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
    // Та же сессия, что у проверки: после неё виртуальных индексов нет.
    let client = glafira_pg_lib::actions::conn::open_explain(&st, "it", &m, Some("perf"))
        .await
        .unwrap();
    let c = &client.client;
    let r = hypopg::check_on(c, &req).await.unwrap();
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
    assert_eq!(hypo_count(c).await, 0, "после успеха");
    // Проба «после» падает (таблицы нет) уже при созданном индексе: сброс всё равно выполнен.
    let schema = hypopg::schema_of(c).await.unwrap().unwrap();
    let broken = HypoRequest {
        sql: "SELECT count(*) FROM shop.nope".into(),
        params: Params::None,
        ..req.clone()
    };
    let e = hypopg::hypothesize(c, &schema, &broken, &broken.sql)
        .await
        .unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str()),
        (ErrorCode::InvalidInput, "errors.explain.serverMissing"),
        "{e:?}"
    );
    assert_eq!(hypo_count(c).await, 0, "после ошибки пробы «после»");
    // Неизвестная колонка — сервер (hypopg) отвергает гипотезу, это ошибка ввода; следов тоже нет.
    let unknown = HypoRequest {
        index_ddl: "CREATE INDEX ON shop.orders (no_such_column)".into(),
        ..req.clone()
    };
    let e = hypopg::check_on(c, &unknown).await.unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str()),
        (ErrorCode::InvalidInput, "errors.hypo.rejected"),
        "{e:?}"
    );
    assert_eq!(hypo_count(c).await, 0, "после отказа гипотезе");
    drop(client);
    // Через команду (своё соединение): тот же итог; CONCURRENTLY — отказ до сервера.
    assert!(hypopg::check(&st, "it", &req).await.unwrap().used);
    let bad = HypoRequest {
        index_ddl: "CREATE INDEX CONCURRENTLY ON shop.orders (status)".into(),
        ..req.clone()
    };
    assert_eq!(
        hypopg::check(&st, "it", &bad).await.unwrap_err().code,
        ErrorCode::InvalidInput
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

/// Неквалифицированные имена в запросе и в DDL гипотезы разрешаются по `search_path` запроса: `orders` есть только в
/// `shop` — без `SET LOCAL search_path` в транзакции гипотезы `hypopg_create_index` её бы не нашёл.
#[tokio::test]
async fn hypothesis_uses_the_search_path_of_the_query() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let m = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let req = HypoRequest {
        member: m,
        database: "perf".into(),
        sql: "SELECT count(*) FROM orders WHERE status = $1".into(),
        params: Params::Values {
            values: vec![Some("disputed".into())],
        },
        search_path: Some("shop".into()),
        index_ddl: "CREATE INDEX ON orders (status)".into(),
    };
    let r = hypopg::check(&st, "it", &req).await.unwrap();
    assert!(r.used && r.cost_after < r.cost_before, "{r:?}");
    let rels = relations(&r.plan_after);
    assert!(
        rels.iter()
            .any(|x| x.schema.as_deref() == Some("shop") && x.name == "orders"),
        "{rels:?}"
    );
    // Без search_path таблицы `orders` нет — ошибка пробы до гипотезы.
    let plain = HypoRequest {
        search_path: None,
        ..req
    };
    let e = hypopg::check(&st, "it", &plain).await.unwrap_err();
    assert_eq!(e.key, "errors.explain.serverMissing", "{e:?}");
}
