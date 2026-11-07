//! `run_select` и уровень `data` на pg-lab (`GPG_IT_LAB`; спец. P4 §4.3 шаг 4, §4.5, §7, §13 «Интеграция»): уровень доступа
//! (`not_allowed`, `data_not_allowed`, `data_disabled` под потолком политики), враждебный набор на живом сервере — разбор
//! `pg_query`, проба плана (запрещённый вызов внутри представления), сверка функций с `pg_proc` (`user_function`:
//! прямой вызов, представление, запись через атрибут `t.f`, схема `net`), — пределы строк, байт, длинных значений и времени;
//! отмена клиента останавливает инструкцию на сервере; база после всего не изменилась; журнал без значений; на уровне
//! `data` тексты без маскирования и значения `pg_stats`; предупреждение о роли. Обрезка длинных значений на сервере —
//! в `it_mcp_backend::run_select_cuts_long_values_on_the_server`. Сервер MCP — в процессе (`tests/mcp_harness`). Роль
//! запросов модели — `glafira_app`: у неё есть права записи в `shop`, поэтому «ничего не изменилось» что-то значит; чужие
//! сессии и `pg_stats` читаются под `postgres`. Запускать с `--test-threads=1`.

mod common;
mod mcp_harness;

use std::time::{Duration, Instant};

use glafira_mcp_core::limits::HARD_LIMIT_BYTES;
use glafira_pg_lib::conn::profile::LlmAccess;
use glafira_pg_lib::policy::{McpMaxAccess, Policy, PolicyStatus};
use glafira_pg_lib::state::AppState;
use mcp_harness::{Gateway, set_access};
use pg_advisor::sqlcheck::{Purpose, check};
use serde_json::{Value, json};

/// Пометка обрезанного значения `run_select` (спец. §7.4).
const TRUNCATED: &str = "…[truncated]";
const MCP_APP: &str = "glafira-pg-mcp";

fn select(sql: &str) -> Value {
    json!({"connectionId": "it", "database": "orders", "sql": sql})
}

fn select_max(sql: &str, max_rows: u32) -> Value {
    json!({"connectionId": "it", "database": "orders", "sql": sql, "maxRows": max_rows})
}

/// Подключение `it` к базе `orders` под ролью `user` с уровнем `access` и сессия MCP к нему.
async fn gateway(
    lab: &str,
    user: &str,
    password: &str,
    access: LlmAccess,
) -> (tempfile::TempDir, AppState, Gateway) {
    common::ensure_fixtures(lab).await;
    let (dir, st) = common::open_server_as(lab, "orders", user, password).await;
    set_access(&st, "it", access);
    let gw = Gateway::start(&st, dir.path()).await;
    (dir, st, gw)
}

/// Отпечаток того, что враждебный SQL мог бы изменить: заказы, таблица из `SELECT INTO`, последовательность. Глобальные
/// advisory-блокировки не считаются: любая чужая сессия стенда делает отпечаток нестабильным.
async fn fingerprint(lab: &str) -> String {
    common::superuser(lab, "orders")
        .await
        .query_one(
            "SELECT concat_ws('|', \
               (SELECT count(*) FROM shop.orders), \
               (SELECT md5(coalesce(string_agg(id || ':' || status || ':' || total, ',' ORDER BY id), '')) FROM shop.orders), \
               (SELECT count(*) FROM pg_class WHERE relname = 'glafira_it_copy'), \
               (SELECT last_value || ':' || is_called FROM shop.invoice_seq), \
               (SELECT count(*) FROM pg_settings WHERE name = 'statement_timeout' AND source <> 'default'))",
            &[],
        )
        .await
        .unwrap()
        .get(0)
}

/// Сессий SQL модели на сервере (`application_name = glafira-pg-mcp`).
async fn mcp_sessions(lab: &str) -> i64 {
    common::superuser(lab, "orders")
        .await
        .query_one(
            "SELECT count(*) FROM pg_stat_activity WHERE application_name = $1",
            &[&MCP_APP],
        )
        .await
        .unwrap()
        .get(0)
}

/// Строка `columnStats` ответа `get_table_details` по имени колонки.
fn column<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["columnStats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["column"] == name)
        .unwrap_or_else(|| panic!("колонка {name}: {v}"))
}

#[tokio::test]
async fn run_select_needs_the_data_level() {
    let Some(lab) = common::lab_addr() else {
        return;
    };
    let (_dir, st, gw) =
        gateway(&lab, common::APP_USER, common::APP_PASSWORD, LlmAccess::Off).await;
    let args = select("SELECT 1 AS one, 'x'::text AS t, NULL::int AS z");
    // Закрытое для модели подключение неотличимо от отсутствующего.
    let e = gw.err("run_select", args.clone()).await;
    assert!(e.starts_with("not_allowed:"), "{e}");
    set_access(&st, "it", LlmAccess::Diagnostics);
    let e = gw.err("run_select", args.clone()).await;
    assert!(
        e.starts_with("data_not_allowed:") && e.contains("Diagnostics and data"),
        "{e}"
    );
    set_access(&st, "it", LlmAccess::Data);
    let v = gw.ok("run_select", args).await;
    assert_eq!(v["member"], mcp_harness::member(&st).as_str());
    let columns = v["columns"].as_array().unwrap();
    let names: Vec<&str> = columns
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["one", "t", "z"]);
    assert!(
        columns
            .iter()
            .all(|c| c["type"].as_str().is_some_and(|t| !t.is_empty())),
        "{v}"
    );
    assert_eq!(v["rows"], json!([["1", "x", null]]));
    assert_eq!(v["rowCount"], 1);
    assert_eq!(v["truncated"], false);
    assert_eq!(v["truncatedValues"], 0);
    assert!(v["ms"].is_u64(), "{v}");

    // `database` необязателен (Q2): нет поля и пустая строка — база подключения.
    let no_db = json!({"connectionId": "it", "sql": "SELECT current_database() AS d"});
    let v = gw.ok("run_select", no_db).await;
    assert_eq!(v["rows"], json!([["orders"]]));
    let blank_db =
        json!({"connectionId": "it", "database": "", "sql": "SELECT current_database() AS d"});
    let v = gw.ok("run_select", blank_db).await;
    assert_eq!(v["rows"], json!([["orders"]]));
}

/// Уровень `data` под потолком политики `diagnostics` (`mcpMaxAccess`): `data_disabled`, а не `data_not_allowed`.
#[tokio::test]
async fn policy_ceiling_gives_data_disabled() {
    let Some(lab) = common::lab_addr() else {
        return;
    };
    common::ensure_fixtures(&lab).await;
    let (dir, st) =
        common::open_server_as(&lab, "orders", common::APP_USER, common::APP_PASSWORD).await;
    let st = st.with_policy(Policy {
        status: PolicyStatus {
            mcp_max_access: McpMaxAccess::Diagnostics,
            ..PolicyStatus::default()
        },
    });
    set_access(&st, "it", LlmAccess::Data);
    let gw = Gateway::start(&st, dir.path()).await;
    let e = gw.err("run_select", select("SELECT 1")).await;
    assert!(
        e.starts_with("data_disabled:") && e.contains("policy"),
        "{e}"
    );
}

/// Враждебный набор (спец. P4 §13): SQL, код отказа, фрагмент сообщения (пустой — не проверяется).
const HOSTILE: &[(&str, &str, &str)] = &[
    (
        "SELECT query_to_xml('SELECT * FROM shop.orders', true, true, '')",
        "forbidden_function",
        "query_to_xml",
    ),
    (
        "WITH d AS (DELETE FROM shop.orders WHERE id = -1 RETURNING *) SELECT * FROM d",
        "invalid_sql",
        "",
    ),
    (
        "SELECT set_config('statement_timeout', '0', false)",
        "forbidden_function",
        "set_config",
    ),
    (
        "SELECT \"pg_catalog\".\"set_config\"('glafira.it', 'x', false)",
        "forbidden_function",
        "set_config",
    ),
    ("SELECT pg_sleep(1)", "forbidden_function", "pg_sleep"),
    (
        "SELECT lo_import('/etc/passwd')",
        "forbidden_function",
        "lo_import",
    ),
    (
        "SELECT pg_advisory_lock(42)",
        "forbidden_function",
        "pg_advisory_lock",
    ),
    (
        "SELECT * FROM dblink('dbname=orders', 'SELECT 1') AS t(x int)",
        "forbidden_function",
        "dblink",
    ),
    (
        "SELECT x FROM (SELECT pg_read_file('/etc/passwd') AS x) s",
        "forbidden_function",
        "pg_read_file",
    ),
    // Имена из Q1: запрещены по имени, без обращения к расширению (его на стенде нет).
    (
        "SELECT * FROM pg_background_launch('SELECT 1')",
        "forbidden_function",
        "pg_background",
    ),
    (
        "SELECT * FROM crosstab('SELECT 1, 2') AS t(a int, b int)",
        "forbidden_function",
        "crosstab",
    ),
    (
        "SELECT * FROM connectby('shop.orders', 'id', 'id', '1', 0) AS t(a int, b int, c int)",
        "forbidden_function",
        "connectby",
    ),
    (
        "SELECT * FROM xpath_table('id', 'x', 'shop.orders', '/a', 'true') AS t(a int)",
        "forbidden_function",
        "xpath_table",
    ),
    (
        "SELECT * FROM pgr_dijkstra('SELECT 1', 1, 2)",
        "forbidden_function",
        "pgr_dijkstra",
    ),
    ("SELECT * FROM shop.orders FOR UPDATE", "invalid_sql", ""),
    (
        "SELECT * FROM (SELECT * FROM shop.orders FOR SHARE) s",
        "invalid_sql",
        "",
    ),
    (
        "SELECT * INTO glafira_it_copy FROM shop.orders",
        "invalid_sql",
        "",
    ),
    ("SELECT 1; SELECT 2", "invalid_sql", ""),
    (
        "UPDATE shop.orders SET status = 'x' WHERE id = -1",
        "invalid_sql",
        "",
    ),
    ("DO $$ BEGIN PERFORM 1; END $$", "invalid_sql", ""),
    ("COPY shop.orders TO STDOUT", "invalid_sql", ""),
    ("CALL shop.close_order(1)", "invalid_sql", ""),
    // Тело пользовательской функции разбор не видит: сверка с pg_proc (§7.3 шаг 4).
    ("SELECT shop.order_count(1)", "user_function", "order_count"),
];

#[tokio::test]
async fn run_select_rejects_the_hostile_set_on_a_live_server() {
    let Some(lab) = common::lab_addr() else {
        return;
    };
    let (_dir, _st, gw) = gateway(
        &lab,
        common::APP_USER,
        common::APP_PASSWORD,
        LlmAccess::Data,
    )
    .await;
    let before = fingerprint(&lab).await;
    for (sql, code, part) in HOSTILE {
        let e = gw.err("run_select", select(sql)).await;
        assert!(
            e.starts_with(&format!("{code}:")) && e.contains(part),
            "{sql}: {e}"
        );
    }
    // nextval проходит все проверки (функция pg_catalog), но транзакция READ ONLY его не выполняет.
    let r = gw
        .call("run_select", select("SELECT nextval('shop.invoice_seq')"))
        .await;
    assert_eq!(r.is_error, Some(true), "{}", mcp_harness::text(&r));
    assert_eq!(
        fingerprint(&lab).await,
        before,
        "база после враждебного набора"
    );
}

/// Вызов `set_config` спрятан в представлении: в тексте модели функций нет (`pg_query` пропускает), а VERBOSE-план пробы
/// показывает вызов в выражениях узла — `forbidden_function` (§7.3 шаг 3). `set_config`, а не `pg_sleep`: колонку
/// псевдотипа `void` представление не примет. Проба запрос не выполняет — настройка не меняется.
#[tokio::test]
async fn plan_probe_catches_a_denied_call_hidden_in_a_view() {
    let Some(lab) = common::lab_addr() else {
        return;
    };
    common::ensure_fixtures(&lab).await;
    let _drop = common::restore_on_drop({
        let lab = lab.clone();
        move || {
            common::run_sql_blocking(
                &lab,
                "orders",
                "DROP VIEW IF EXISTS public.glafira_it_hidden",
            )
        }
    });
    common::superuser(&lab, "orders")
        .await
        .batch_execute(
            "CREATE OR REPLACE VIEW public.glafira_it_hidden AS SELECT set_config('glafira.it', 'x', false) AS s; \
             GRANT SELECT ON public.glafira_it_hidden TO glafira_app",
        )
        .await
        .unwrap();
    let sql = "SELECT * FROM public.glafira_it_hidden";
    let checked = check(sql, Purpose::Select).expect("разбор пропускает представление");
    assert!(checked.functions.is_empty(), "{:?}", checked.functions);
    let (dir, st) =
        common::open_server_as(&lab, "orders", common::APP_USER, common::APP_PASSWORD).await;
    set_access(&st, "it", LlmAccess::Data);
    let gw = Gateway::start(&st, dir.path()).await;
    let e = gw.err("run_select", select(sql)).await;
    assert!(
        e.starts_with("forbidden_function:") && e.contains("set_config"),
        "{e}"
    );
}

/// Пользовательская функция там, где разбор её не видит (Q1; функции plpgsql: SQL-функцию планировщик подставляет в запрос, и в плане вызова нет): в представлении, в записи через атрибут (`SELECT o.f FROM
/// shop.orders o` — это `f(o)`), в схеме `net` (запрещена целиком, в том числе при `net` в `search_path`). Объекты
/// временные: схема `net` и функции создаются под охраной, созданной раньше `CREATE`.
#[tokio::test]
async fn user_functions_are_found_by_the_plan_and_net_is_denied() {
    let Some(lab) = common::lab_addr() else {
        return;
    };
    common::ensure_fixtures(&lab).await;
    let _drop = common::restore_on_drop({
        let lab = lab.clone();
        move || {
            for sql in [
                "ALTER ROLE glafira_app IN DATABASE orders RESET search_path",
                "DROP VIEW IF EXISTS public.glafira_it_userfn",
                "DROP FUNCTION IF EXISTS public.glafira_it_attr(shop.orders)",
                "DROP SCHEMA IF EXISTS net CASCADE",
            ] {
                common::run_sql_blocking(&lab, "orders", sql);
            }
        }
    });
    common::superuser(&lab, "orders")
        .await
        .batch_execute(
            "CREATE VIEW public.glafira_it_userfn AS SELECT shop.order_count(1) AS c; \
             GRANT SELECT ON public.glafira_it_userfn TO glafira_app; \
             CREATE FUNCTION public.glafira_it_attr(shop.orders) RETURNS int LANGUAGE plpgsql AS 'BEGIN RETURN 1; END'; \
             CREATE SCHEMA net; \
             GRANT USAGE ON SCHEMA net TO glafira_app; \
             CREATE FUNCTION net.glafira_it_get(int) RETURNS int LANGUAGE plpgsql AS 'BEGIN RETURN 1; END'",
        )
        .await
        .unwrap();
    let (dir, st) =
        common::open_server_as(&lab, "orders", common::APP_USER, common::APP_PASSWORD).await;
    set_access(&st, "it", LlmAccess::Data);
    let gw = Gateway::start(&st, dir.path()).await;

    let e = gw
        .err(
            "run_select",
            select("SELECT * FROM public.glafira_it_userfn"),
        )
        .await;
    assert!(
        e.starts_with("user_function:") && e.contains("order_count") && e.contains("view"),
        "{e}"
    );
    let e = gw
        .err(
            "run_select",
            select("SELECT o.glafira_it_attr FROM shop.orders o"),
        )
        .await;
    assert!(
        e.starts_with("user_function:") && e.contains("glafira_it_attr"),
        "{e}"
    );
    let e = gw
        .err("run_select", select("SELECT net.glafira_it_get(1)"))
        .await;
    assert!(
        e.starts_with("forbidden_function:") && e.contains("glafira_it_get"),
        "{e}"
    );
    // `net` в search_path роли: вызов без схемы находится по имени из плана.
    common::superuser(&lab, "orders")
        .await
        .batch_execute("ALTER ROLE glafira_app IN DATABASE orders SET search_path = net, public")
        .await
        .unwrap();
    let (dir2, st2) =
        common::open_server_as(&lab, "orders", common::APP_USER, common::APP_PASSWORD).await;
    set_access(&st2, "it", LlmAccess::Data);
    let gw2 = Gateway::start(&st2, dir2.path()).await;
    let e = gw2
        .err("run_select", select("SELECT glafira_it_get(1)"))
        .await;
    assert!(e.starts_with("forbidden_function:"), "{e}");
}

#[tokio::test]
async fn run_select_respects_rows_bytes_values_and_time() {
    let Some(lab) = common::lab_addr() else {
        return;
    };
    let (_dir, st, gw) = gateway(
        &lab,
        common::APP_USER,
        common::APP_PASSWORD,
        LlmAccess::Data,
    )
    .await;
    let rows = "SELECT g FROM generate_series(1, 1000) g";
    // По умолчанию 100 строк.
    let v = gw.ok("run_select", select(rows)).await;
    assert_eq!(
        (v["rowCount"].as_u64(), v["truncated"].as_bool()),
        (Some(100), Some(true))
    );
    assert_eq!(v["rows"][0], json!(["1"]));
    let v = gw.ok("run_select", select_max(rows, 250)).await;
    assert_eq!(
        (v["rowCount"].as_u64(), v["truncated"].as_bool()),
        (Some(250), Some(true))
    );
    let v = gw
        .ok(
            "run_select",
            select_max("SELECT g FROM generate_series(1, 7) g", 500),
        )
        .await;
    assert_eq!(
        (v["rowCount"].as_u64(), v["truncated"].as_bool()),
        (Some(7), Some(false))
    );
    let e = gw.err("run_select", select_max(rows, 501)).await;
    assert!(e.starts_with("invalid_argument:"), "{e}");

    // 100 КБ значений: строки по 1500 байт — меньше сотни строк, и ответ укладывается в предел шлюза.
    let v = gw
        .ok(
            "run_select",
            select_max(
                "SELECT repeat('x', 1500) AS v FROM generate_series(1, 500)",
                500,
            ),
        )
        .await;
    let n = v["rowCount"].as_u64().unwrap();
    assert!(v["truncated"] == true && (50..100).contains(&n), "{n}");
    assert!(serde_json::to_vec(&v).unwrap().len() <= HARD_LIMIT_BYTES);

    // Значение длиннее 2 КБ обрезается с пометкой.
    let v = gw
        .ok("run_select", select("SELECT repeat('y', 5000) AS v"))
        .await;
    let s = v["rows"][0][0].as_str().unwrap();
    assert!(
        s.starts_with(&"y".repeat(100)) && s.ends_with(TRUNCATED),
        "{}…",
        &s[..40]
    );
    assert!(s.len() <= 2048 + TRUNCATED.len(), "{}", s.len());
    assert_eq!(v["truncatedValues"], 1);

    // Значения в журнал не пишутся (§7.4): в записи вызова есть счётчики и размер, но не строки результата.
    let v = gw
        .ok("run_select", select("SELECT 'p4b-audit-' || 'secret' AS v"))
        .await;
    assert_eq!(v["rows"], json!([["p4b-audit-secret"]]));
    let calls: Vec<Value> = st
        .mcp
        .audit_tail(1000)
        .into_iter()
        .filter(|e| e["event"] == "call" && e["tool"] == "run_select")
        .collect();
    assert!(calls.len() >= 6, "{}", calls.len());
    let log = std::fs::read_to_string(st.mcp.audit_path()).unwrap();
    assert!(!log.contains("p4b-audit-secret"), "значение в журнале");
    assert!(calls.iter().any(|c| c["outcome"] == "ok"), "{:?}", calls[0]);

    // statement_timeout 10 с: декартово произведение без запрещённых функций и без большого temp.
    let started = Instant::now();
    let e = gw
        .err(
            "run_select",
            select(
                "SELECT count(*) FROM generate_series(1, 100000) a CROSS JOIN generate_series(1, 100000) b",
            ),
        )
        .await;
    let took = started.elapsed();
    eprintln!("TIMEOUT case took {took:?}");
    assert!(e.starts_with("TIMEOUT"), "{e}");
    assert!(
        took >= Duration::from_secs(9) && took < Duration::from_secs(20),
        "{took:?}"
    );
    common::wait_until(
        "сессии SQL модели закрыты",
        Duration::from_secs(8),
        async || mcp_sessions(&lab).await == 0,
    )
    .await;
}

/// Отмена клиентом (уведомление `notifications/cancelled`) останавливает инструкцию на сервере: сессии
/// `glafira-pg-mcp` не остаётся, ответ приходит сразу, а не по истечении 10 с.
#[tokio::test]
async fn client_cancel_stops_the_server_statement() {
    let Some(lab) = common::lab_addr() else {
        return;
    };
    let (_dir, _st, gw) = gateway(
        &lab,
        common::APP_USER,
        common::APP_PASSWORD,
        LlmAccess::Data,
    )
    .await;
    let mut p = rmcp::model::CallToolRequestParams::new("run_select".to_string());
    p = p.with_arguments(
        select("SELECT count(*) FROM generate_series(1, 100000) a CROSS JOIN generate_series(1, 100000) b")
            .as_object()
            .unwrap()
            .clone(),
    );
    let handle = gw
        .client
        .send_cancellable_request(
            rmcp::model::ClientRequest::CallToolRequest(rmcp::model::Request::new(p)),
            rmcp::service::PeerRequestOptions::no_options(),
        )
        .await
        .unwrap();
    // Инструкция началась на сервере.
    common::wait_until(
        "инструкция модели на сервере",
        Duration::from_secs(5),
        async || mcp_sessions(&lab).await == 1,
    )
    .await;
    let started = Instant::now();
    handle.cancel(Some("test".into())).await.unwrap();
    common::wait_until(
        "сессии SQL модели закрыты",
        Duration::from_secs(5),
        async || mcp_sessions(&lab).await == 0,
    )
    .await;
    let took = started.elapsed();
    eprintln!("cancel to no session: {took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
}

/// Предупреждение о роли (§4.5): факт проверяется при первом `run_select` сессии и виден модели в `list_connections`.
#[tokio::test]
async fn role_warning_names_dangerous_rights() {
    let Some(lab) = common::lab_addr() else {
        return;
    };
    let cases = [
        (
            common::SUPERUSER,
            common::SUPERUSER_PASSWORD,
            Some("superuser"),
        ),
        (
            common::PG_USER,
            common::PG_PASSWORD_VALUE,
            Some("pg_signal_backend"),
        ),
        (common::APP_USER, common::APP_PASSWORD, None),
    ];
    for (user, password, expect) in cases {
        let (_dir, _st, gw) = gateway(&lab, user, password, LlmAccess::Data).await;
        gw.ok("run_select", select("SELECT 1")).await;
        let v = gw.ok("list_connections", json!({})).await;
        let item = &v["connections"][0];
        assert_eq!(item["llmAccess"], "data", "{item}");
        let warning = item["roleWarning"].as_str();
        match expect {
            Some(word) => assert!(
                warning.is_some_and(|w| w.contains(user) && w.contains(word)),
                "{user}: {warning:?}"
            ),
            None => assert!(warning.is_none(), "{user}: {warning:?}"),
        }
    }
}

/// Уровень `data` (§4.1, §5.2, §6.1): литералы в текстах сессий видны (на `diagnostics` — `$N`), у колонок —
/// частые значения `pg_stats` (на `diagnostics` их нет).
#[tokio::test]
async fn data_level_shows_literals_and_pg_stats_values() {
    let Some(lab) = common::lab_addr() else {
        return;
    };
    let (_dir, st, gw) = gateway(
        &lab,
        common::SUPERUSER,
        common::SUPERUSER_PASSWORD,
        LlmAccess::Diagnostics,
    )
    .await;
    // Сессия «приложения» с литералом в тексте запроса.
    let app = common::app(&lab).await;
    app.batch_execute("SET application_name = 'glafira-it-mcp-data'")
        .await
        .unwrap();
    let busy = tokio::spawn(async move {
        app.batch_execute("SELECT pg_sleep(6), 'p4b-secret-literal'")
            .await
    });
    let args = json!({"connectionId": "it", "application": "glafira-it-mcp-data"});
    let mut masked = Value::Null;
    common::wait_until(
        "сессия приложения в get_activity",
        Duration::from_secs(5),
        async || {
            masked = gw.ok("get_activity", args.clone()).await;
            masked["sessions"].as_array().is_some_and(|s| !s.is_empty())
        },
    )
    .await;
    let q = masked["sessions"][0]["query"].as_str().unwrap();
    assert!(!q.contains("p4b-secret-literal") && q.contains('$'), "{q}");
    set_access(&st, "it", LlmAccess::Data);
    let v = gw.ok("get_activity", args).await;
    let q = v["sessions"][0]["query"].as_str().unwrap();
    assert!(q.contains("p4b-secret-literal"), "{q}");

    let table =
        json!({"connectionId": "it", "database": "orders", "schema": "shop", "table": "orders"});
    set_access(&st, "it", LlmAccess::Diagnostics);
    let v = gw.ok("get_table_details", table.clone()).await;
    let status = column(&v, "status");
    assert!(
        status.get("mostCommonVals").is_none_or(Value::is_null),
        "{status}"
    );
    // Смоук P4b, M2: ответ говорит, почему значений нет.
    assert!(
        v["hint"]
            .as_str()
            .is_some_and(|h| h.contains("“Diagnostics and data”")),
        "{v}"
    );
    set_access(&st, "it", LlmAccess::Data);
    let v = gw.ok("get_table_details", table).await;
    let status = column(&v, "status");
    assert!(
        status["mostCommonVals"].to_string().contains("new"),
        "{status}"
    );
    assert!(v.get("hint").is_none(), "{v}");
    busy.await.unwrap().unwrap();
}
