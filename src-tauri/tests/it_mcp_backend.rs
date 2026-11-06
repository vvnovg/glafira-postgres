//! `AppBackend` MCP на pg-perf (`GPG_IT_PERF`): SQL модели — `explain_query` и `hypothetical_index` через `GuardTx`
//! (своё соединение `glafira-pg-mcp`, `READ ONLY`, `standard_conforming_strings = on` на всю сессию; после вызова и при
//! брошенном вызове сессии сервера нет — нет ни транзакции, ни виртуальных индексов), запрещённая функция за
//! представлением — по VERBOSE-плану, план текста pgss по `queryId` — замаскирован; наблюдение — `observe_activity` и
//! `observe_top_queries`; `run_select` (P4b) — обрезка значений на сервере. Запускать с `--test-threads=1`.

mod common;

use std::time::Duration;

use glafira_pg_lib::conn::profile::LlmAccess;
use glafira_pg_lib::explain::Params;
use glafira_pg_lib::mcp::guard;
use glafira_pg_lib::mcp::read::{EXPLAIN_TIMEOUT_S, observe_op, read_op};
use glafira_pg_lib::mcp::select;
use glafira_pg_lib::state::AppState;
use glafira_pg_mcp::model::{ObserveCtx, PgAnswer, PgObserve, PgOutput, PgRead, SelectOut};
use glafira_pg_mcp::params::RunSelectParams;
use serde_json::{Value, json};
use tokio_postgres::{Client, SimpleQueryMessage};

const MCP_APP: &str = "glafira-pg-mcp";

/// Подключение `it` к базе `perf`, открытое для LLM на уровне `diagnostics`; узел — единственный участник.
async fn open_for_llm(perf: &str) -> (tempfile::TempDir, AppState, String) {
    let (dir, st) = common::open_server_in(perf, "perf").await;
    let mut p = st.profile("it").unwrap();
    p.llm_access = LlmAccess::Diagnostics;
    st.connections.upsert(p).unwrap();
    let m = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    (dir, st, m)
}

fn explain(v: Value) -> PgRead {
    PgRead::Explain(serde_json::from_value(v).unwrap())
}

fn hypo(v: Value) -> PgRead {
    PgRead::HypoIndex(serde_json::from_value(v).unwrap())
}

fn ok(r: Result<PgAnswer, glafira_mcp_core::error::ToolError>) -> PgAnswer {
    r.unwrap_or_else(|e| panic!("{}: {}", e.code, e.message))
}

/// Сессий SQL модели на сервере (`application_name = glafira-pg-mcp`).
async fn mcp_sessions(probe: &Client) -> i64 {
    probe
        .query_one(
            "SELECT count(*) FROM pg_stat_activity WHERE application_name = $1",
            &[&MCP_APP],
        )
        .await
        .unwrap()
        .get(0)
}

async fn wait_no_mcp_sessions(probe: &Client, what: &str) {
    common::wait_until(what, Duration::from_secs(8), async || {
        mcp_sessions(probe).await == 0
    })
    .await;
}

/// Настройка сессии — вне пакета транзакции: `ROLLBACK` транзакции защиты не возвращает значение роли или базы
/// (на том же соединении после отката идут транзакции hypopg). Сессия начинается с `off`, как у базы с таким умолчанием.
#[tokio::test]
async fn standard_conforming_strings_stay_on_after_the_guard_rollback() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let c = common::superuser(&perf, "perf").await;
    c.batch_execute("SET standard_conforming_strings = off")
        .await
        .unwrap();
    guard::open_session(&c, EXPLAIN_TIMEOUT_S).await.unwrap();
    let show = async |c: &Client, name: &str| -> String {
        c.query_one(&format!("SHOW {name}"), &[])
            .await
            .unwrap()
            .get(0)
    };
    assert_eq!(show(&c, "transaction_read_only").await, "on");
    assert_eq!(show(&c, "statement_timeout").await, "5s");
    c.batch_execute("ROLLBACK").await.unwrap();
    assert_eq!(
        show(&c, "standard_conforming_strings").await,
        "on",
        "настройка сессии пережила ROLLBACK"
    );
}

/// `GuardTx`: своё соединение `glafira-pg-mcp` в транзакции `READ ONLY`; сброс без `finish` (тело вызова брошено) закрывает
/// соединение — сессия сервера завершается вместе с виртуальным индексом в её памяти.
#[tokio::test]
async fn dropped_guard_leaves_no_session_and_no_hypothesis() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st, m) = open_for_llm(&perf).await;
    let probe = common::superuser(&perf, "perf").await;
    let tx = guard::begin(&st, "it", Some(&m), "perf", EXPLAIN_TIMEOUT_S)
        .await
        .unwrap();
    assert_eq!(tx.member(), m);
    assert!(tx.version_num() >= 130_000);
    let row = tx
        .client()
        .query_one(
            "SELECT pg_backend_pid(), current_setting('application_name'), \
             current_setting('transaction_read_only'), current_setting('standard_conforming_strings')",
            &[],
        )
        .await
        .unwrap();
    let pid: i32 = row.get(0);
    assert_eq!(
        (
            row.get::<_, String>(1),
            row.get::<_, String>(2),
            row.get::<_, String>(3)
        ),
        (MCP_APP.to_string(), "on".to_string(), "on".to_string())
    );
    // Запись в транзакции защиты сервер не примет.
    let e = tx
        .client()
        .batch_execute("CREATE TEMP TABLE it_guard_write (a int)")
        .await
        .unwrap_err();
    assert_eq!(e.code().map(|c| c.code()), Some("25006"), "{e:?}");
    tx.rollback().await.unwrap();
    // Защита вглубь: сессия SQL модели — только чтение и вне транзакции защиты (`default_transaction_read_only`).
    let ro: String = tx
        .client()
        .query_one("SHOW default_transaction_read_only", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(ro, "on");
    let e = tx
        .client()
        .batch_execute("CREATE TEMP TABLE it_guard_write (a int)")
        .await
        .unwrap_err();
    assert_eq!(
        e.code().map(|c| c.code()),
        Some("25006"),
        "запись вне транзакции защиты: {e:?}"
    );
    // Виртуальный индекс в новой транзакции той же сессии, без сброса — и брошенный `GuardTx`.
    tx.client()
        .batch_execute(
            "BEGIN READ ONLY; SELECT * FROM hypopg_create_index('CREATE INDEX ON shop.orders (status)')",
        )
        .await
        .unwrap();
    let n: i64 = tx
        .client()
        .query_one("SELECT count(*) FROM hypopg_list_indexes", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(n, 1);
    drop(tx);
    common::wait_until(
        "сессия брошенного GuardTx завершена",
        Duration::from_secs(8),
        async || {
            probe
                .query_one(
                    "SELECT count(*) FROM pg_stat_activity WHERE pid = $1",
                    &[&pid],
                )
                .await
                .unwrap()
                .get::<_, i64>(0)
                == 0
        },
    )
    .await;
}

/// Брошенный на любом `await` вызов `hypothetical_index` (сессия MCP закрылась) не оставляет сессии модели: ни
/// открытой транзакции, ни виртуальных индексов. Точки обрыва — разные сроки; где бы ни оборвалось, сессий нет.
#[tokio::test]
async fn dropped_hypothesis_call_leaves_no_session() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st, m) = open_for_llm(&perf).await;
    let probe = common::superuser(&perf, "perf").await;
    let op = || {
        hypo(json!({
            "connectionId": "it", "member": m, "database": "perf",
            "sql": "SELECT count(*) FROM shop.orders WHERE status = $1", "params": ["disputed"],
            "indexes": ["CREATE INDEX ON shop.orders (status)", "CREATE INDEX ON shop.orders (total)"]
        }))
    };
    // Полный вызов — для сравнения: после него сессий тоже нет.
    ok(read_op(&st, op()).await);
    wait_no_mcp_sessions(&probe, "после полного вызова").await;
    for ms in [5, 15, 30, 60, 100, 200] {
        let cut = tokio::time::timeout(Duration::from_millis(ms), read_op(&st, op())).await;
        wait_no_mcp_sessions(
            &probe,
            &format!("после обрыва через {ms} мс ({})", cut.is_ok()),
        )
        .await;
    }
}

#[tokio::test]
async fn model_sql_is_planned_without_execution() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st, m) = open_for_llm(&perf).await;
    let probe = common::superuser(&perf, "perf").await;
    let a = ok(read_op(
        &st,
        explain(json!({
            "connectionId": "it", "database": "perf",
            "sql": "  SELECT count(*) FROM shop.orders WHERE status = $1;", "params": ["disputed"]
        })),
    )
    .await);
    let PgOutput::Explain(out) = &a.body else {
        panic!("не план")
    };
    assert_eq!(a.masked, None, "SQL модели не маскируется");
    assert_eq!(out.member, m);
    assert!(out.plan_text.contains("'disputed'"), "{}", out.plan_text);
    assert!(
        out.tables.contains(&"shop.orders".to_string()),
        "{:?}",
        out.tables
    );
    assert!(out.summary.total_cost > 0.0);
    // Без значений на PG 16+ — обобщённый план.
    let a = ok(read_op(
        &st,
        explain(json!({
            "connectionId": "it", "database": "perf",
            "sql": "SELECT count(*) FROM shop.orders WHERE status = $1"
        })),
    )
    .await);
    let PgOutput::Explain(out) = &a.body else {
        panic!("не план")
    };
    assert!(
        out.warnings[0].contains("GENERIC_PLAN"),
        "{:?}",
        out.warnings
    );
    // DML доходит до пробы плана (sqlcheck его пропускает): EXPLAIN проверяет права записи и без выполнения, у роли
    // диагностики их нет — NOT_AUTHORIZED с подсказкой про запись.
    let e = read_op(
        &st,
        explain(json!({
            "connectionId": "it", "database": "perf",
            "sql": "DELETE FROM shop.orders WHERE status = 'disputed'"
        })),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, "NOT_AUTHORIZED", "{}", e.message);
    assert!(e.message.contains("write statement"), "{}", e.message);
    // Отказ сервера при разборе — invalid_sql с его текстом и позицией в тексте модели.
    let e = read_op(
        &st,
        explain(json!({
            "connectionId": "it", "database": "perf",
            "sql": "SELECT no_such_column FROM shop.orders"
        })),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, "invalid_sql", "{}", e.message);
    assert!(
        e.message.contains("no_such_column") && e.message.contains("(position 8)"),
        "{}",
        e.message
    );
    let e = read_op(
        &st,
        explain(json!({
            "connectionId": "it", "database": "perf",
            "sql": "SELECT count(*) FROM shop.orders WHERE status = $1", "params": ["a", "b"]
        })),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, "invalid_argument", "{}", e.message);
    wait_no_mcp_sessions(&probe, "после EXPLAIN").await;
}

/// Позиции ошибок сервера — в тексте, как его прислала модель: ведущие пробелы и переводы строк, которые снимает
/// нормализация, в счёте есть — и у `prepare` под точкой сохранения, и у пробы плана.
#[tokio::test]
async fn server_positions_refer_to_the_text_the_model_sent() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st, m) = open_for_llm(&perf).await;
    let probe = common::superuser(&perf, "perf").await;
    // «  SELECT » — 9 символов: `no_such_column` — 10-й.
    let e = read_op(
        &st,
        explain(json!({
            "connectionId": "it", "database": "perf",
            "sql": "  SELECT no_such_column FROM shop.orders;"
        })),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, "invalid_sql", "{}", e.message);
    assert!(e.message.ends_with("(position 10)"), "{}", e.message);
    let e = read_op(
        &st,
        hypo(json!({
            "connectionId": "it", "database": "perf", "sql": "\n\tSELECT no_such_column FROM shop.orders",
            "indexes": ["CREATE INDEX ON shop.orders (status)"]
        })),
    )
    .await
    .unwrap_err();
    assert!(e.message.ends_with("(position 10)"), "{}", e.message);
    // Слои `GuardTx` по отдельности: `prepare` и проба (без `prepare` — отказ даёт EXPLAIN пробы).
    let tx = guard::begin(&st, "it", Some(&m), "perf", EXPLAIN_TIMEOUT_S)
        .await
        .unwrap();
    let e = tx.prepare_one("\n  SELECT nope").await.unwrap_err();
    assert!(e.message.ends_with("(position 11)"), "{}", e.message);
    let e = tx
        .probe(" \n SELECT nope", &Params::None)
        .await
        .unwrap_err();
    assert_eq!(e.code, "invalid_sql", "{}", e.message);
    assert!(e.message.ends_with("(position 11)"), "{}", e.message);
    tx.finish().await;
    wait_no_mcp_sessions(&probe, "после проверки позиций").await;
}

/// Запрещённая функция за представлением: `sqlcheck` её не видит (в тексте только имя представления), VERBOSE-план —
/// видит (§7.3, шаг 3). Функция — с `EXECUTE` для всех (`set_config`): у закрытых от `PUBLIC` (`pg_read_file`) права
/// проверяет уже `ExecutorStart` пробы, и план не строится вовсе (`NOT_AUTHORIZED`).
#[tokio::test]
async fn denied_function_behind_a_view_is_found_in_the_plan() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st, _m) = open_for_llm(&perf).await;
    let su = common::superuser(&perf, "perf").await;
    su.batch_execute(
        "CREATE OR REPLACE VIEW public.glafira_it_mcp_denied AS \
         SELECT set_config('glafira.it', 'x', true) AS v; \
         GRANT SELECT ON public.glafira_it_mcp_denied TO PUBLIC",
    )
    .await
    .unwrap();
    let perf_addr = perf.clone();
    let _drop_view = common::restore_on_drop(move || {
        common::run_sql_blocking(
            &perf_addr,
            "perf",
            "DROP VIEW IF EXISTS public.glafira_it_mcp_denied",
        )
    });
    let e = read_op(
        &st,
        explain(json!({
            "connectionId": "it", "database": "perf",
            "sql": "SELECT * FROM public.glafira_it_mcp_denied"
        })),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, "forbidden_function", "{}", e.message);
    assert!(e.message.contains("set_config"), "{}", e.message);
}

/// План текста pgss по `queryId` (§6.1): не SQL модели — выражения узлов маскируются (значения модели для `$N` дают
/// в плане литерал, и он тоже уходит); без значений на PG 16+ — обобщённый план.
#[tokio::test]
async fn plan_of_a_pgss_query_is_masked() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st, _m) = open_for_llm(&perf).await;
    // pgss хранит текст первого запроса той же формы (псевдонимы и литералы в `queryid` не входят): id — из
    // `EXPLAIN (VERBOSE)` того же запроса («Query Identifier», `compute_query_id`).
    const PROBE: &str = "SELECT count(*) FROM shop.orders WHERE status = 'disputed'";
    let su = common::superuser(&perf, "perf").await;
    su.batch_execute(PROBE).await.unwrap();
    let plan: Value = su
        .query_one(&format!("EXPLAIN (VERBOSE, FORMAT JSON) {PROBE}"), &[])
        .await
        .unwrap()
        .get(0);
    let qid = plan[0]["Query Identifier"]
        .as_i64()
        .unwrap_or_else(|| panic!("{plan}"));
    let a = ok(read_op(
        &st,
        explain(json!({
            "connectionId": "it", "database": "perf", "queryId": qid.to_string(), "params": ["disputed"]
        })),
    )
    .await);
    let PgOutput::Explain(out) = &a.body else {
        panic!("не план")
    };
    assert!(a.masked.is_some_and(|n| n >= 1), "{:?}", a.masked);
    let v = serde_json::to_string(out).unwrap();
    assert!(!v.contains("disputed"), "{v}");
    assert!(out.plan_text.contains("$1"), "{}", out.plan_text);
    let a = ok(read_op(
        &st,
        explain(json!({"connectionId": "it", "database": "perf", "queryId": qid.to_string()})),
    )
    .await);
    let PgOutput::Explain(out) = &a.body else {
        panic!("не план")
    };
    assert!(
        out.warnings[0].contains("GENERIC_PLAN"),
        "{:?}",
        out.warnings
    );
    let e = read_op(
        &st,
        explain(json!({"connectionId": "it", "database": "perf", "queryId": "1"})),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, "not_found", "{}", e.message);
}

/// queryid запроса `sql` в pgss: запрос выполняется суперпользователем, id — из `EXPLAIN (VERBOSE)` того же запроса.
async fn pgss_query_id(perf: &str, sql: &str) -> i64 {
    let su = common::superuser(perf, "perf").await;
    su.batch_execute(sql).await.unwrap();
    let plan: Value = su
        .query_one(&format!("EXPLAIN (VERBOSE, FORMAT JSON) {sql}"), &[])
        .await
        .unwrap()
        .get(0);
    plan[0]["Query Identifier"]
        .as_i64()
        .unwrap_or_else(|| panic!("{plan}"))
}

/// Смоук P4b, G5: pgss хранит запрос с типизированными литералами (`timestamptz '…'`, `interval '…'`) как
/// `timestamptz $1 - interval $2`, и раньше `explain_query` и `hypothetical_index` по queryId отвечали `invalid_sql`.
/// Теперь шлюз приводит частые типы сам (`$1::timestamptz - $2::interval`), и план строится.
#[tokio::test]
async fn pgss_text_with_typed_literals_is_planned() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st, _m) = open_for_llm(&perf).await;
    let qid = pgss_query_id(
        &perf,
        "SELECT count(*) FROM shop.orders o \
         WHERE o.created_at >= timestamptz '2026-01-01 00:00:00+00' - interval '1 day'",
    )
    .await;
    // Значения для $1 и $2: без них на PG < 16 раньше сработала бы проверка GENERIC_PLAN.
    let params = json!(["2026-01-01 00:00:00+00", "1 day"]);
    let a = ok(read_op(
        &st,
        explain(json!({
            "connectionId": "it", "database": "perf", "queryId": qid.to_string(), "params": params
        })),
    )
    .await);
    let PgOutput::Explain(out) = &a.body else {
        panic!("не план")
    };
    assert!(out.tables.iter().any(|t| t == "shop.orders"), "{out:?}");
    let a = ok(read_op(
        &st,
        hypo(json!({
            "connectionId": "it", "database": "perf", "queryId": qid.to_string(), "params": params,
            "indexes": ["CREATE INDEX ON shop.orders (created_at)"]
        })),
    )
    .await);
    assert!(matches!(a.body, PgOutput::Hypo(_)));
}

/// E5: тип, которого шлюз не приводит (`point '(1,2)'` → `point $1`), — сервер текст не разбирает (SQLSTATE 42601):
/// `invalid_sql` с текстом и позицией сервера и подсказкой передать `sql` с приведениями `$N::type` или значениями — у
/// `explain_query` и у `hypothetical_index`; у SQL модели с той же ошибкой подсказки нет.
#[tokio::test]
async fn syntax_error_of_a_pgss_text_carries_the_hint() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st, _m) = open_for_llm(&perf).await;
    let qid = pgss_query_id(
        &perf,
        "SELECT count(*) FROM shop.orders o WHERE o.id > (point '(1,2)' <-> point '(3,4)')",
    )
    .await;
    let params = json!(["(1,2)", "(3,4)"]);
    let hinted = |e: &glafira_mcp_core::error::ToolError| {
        assert_eq!(e.code, "invalid_sql", "{}", e.message);
        assert!(
            e.message.starts_with("syntax error at or near \"$")
                && e.message.contains("(position "),
            "текст и позиция сервера — как раньше: {}",
            e.message
        );
        assert!(
            e.message.contains(&format!(
                "). pg_stat_statements stores query {qid} in normalized form"
            )) && e
                .message
                .contains("written with $N::type casts (for example $1::date)"),
            "{}",
            e.message
        );
    };
    let e = read_op(
        &st,
        explain(json!({
            "connectionId": "it", "database": "perf", "queryId": qid.to_string(), "params": params
        })),
    )
    .await
    .unwrap_err();
    hinted(&e);
    let e = read_op(
        &st,
        hypo(json!({
            "connectionId": "it", "database": "perf", "queryId": qid.to_string(), "params": params,
            "indexes": ["CREATE INDEX orders_created_idx ON shop.orders (created_at)"]
        })),
    )
    .await
    .unwrap_err();
    hinted(&e);
    let e = read_op(
        &st,
        explain(json!({
            "connectionId": "it", "database": "perf", "params": params,
            "sql": "SELECT count(*) FROM shop.orders o WHERE o.id > (point $1 <-> point $2)"
        })),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, "invalid_sql", "{}", e.message);
    assert!(
        e.message.starts_with("syntax error") && !e.message.contains("normalized form"),
        "SQL модели — без подсказки: {}",
        e.message
    );
}

/// Смоук P4b, G6: `get_settings` с `names` и с `changedOnly` в смоуке дали ответы одной длины (2910 байт) — совпадение:
/// `names` выбирает ровно названные настройки (без учёта регистра, неизвестные имена пропускаются, `changedOnly` при них
/// не действует), без `names` — только изменённые (источник не `default`, не `override` и не настройки соединения
/// Glafira).
#[tokio::test]
async fn settings_by_names_and_changed_only() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st, _m) = open_for_llm(&perf).await;
    let settings = |v: Value| PgRead::Settings(serde_json::from_value(v).unwrap());
    let rows = |a: &PgAnswer| -> Vec<(String, String)> {
        let PgOutput::Settings(list) = &a.body else {
            panic!("не настройки")
        };
        let mut v: Vec<(String, String)> = list
            .iter()
            .map(|s| (s.name.to_lowercase(), s.source.clone()))
            .collect();
        v.sort();
        v
    };
    let changed = rows(&ok(read_op(
        &st,
        settings(json!({"connectionId": "it", "changedOnly": true})),
    )
    .await));
    assert!(!changed.is_empty());
    assert!(
        changed
            .iter()
            .all(|(_, src)| !["default", "override", "client", "session"].contains(&src.as_str())),
        "{changed:?}"
    );
    assert!(
        !changed.iter().any(|(n, _)| n == "autovacuum_naptime"),
        "на стенде autovacuum_naptime — по умолчанию: {changed:?}"
    );
    for changed_only in [json!(true), json!(false), Value::Null] {
        let named = rows(&ok(read_op(
            &st,
            settings(json!({
                "connectionId": "it", "changedOnly": changed_only,
                "names": ["work_mem", "TimeZone", " autovacuum_naptime", "no_such_setting"]
            })),
        )
        .await));
        let names: Vec<&str> = named.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            ["autovacuum_naptime", "timezone", "work_mem"],
            "changedOnly {changed_only}"
        );
        assert_eq!(named[0].1, "default", "{named:?}");
    }
    let all = rows(&ok(read_op(
        &st,
        settings(json!({"connectionId": "it", "changedOnly": false})),
    )
    .await));
    assert!(
        all.len() > changed.len() + 100,
        "{} / {}",
        all.len(),
        changed.len()
    );
}

#[tokio::test]
async fn hypotheses_through_the_gateway() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st, _m) = open_for_llm(&perf).await;
    let probe = common::superuser(&perf, "perf").await;
    let a = ok(read_op(
        &st,
        hypo(json!({
            "connectionId": "it", "database": "perf",
            "sql": "SELECT count(*) FROM shop.orders WHERE status = $1", "params": ["disputed"],
            "indexes": ["CREATE INDEX ON shop.orders (status)", "CREATE INDEX ON shop.orders (total);"]
        })),
    )
    .await);
    let PgOutput::Hypo(out) = &a.body else {
        panic!("не гипотеза")
    };
    assert_eq!(a.masked, None);
    assert!(
        out.indexes[0].used && !out.indexes[1].used,
        "{:?}",
        out.indexes
    );
    assert!(out.cost_after < out.cost_before);
    assert!(out.plan_text.contains("status"), "{}", out.plan_text);
    // E4: имя гипотезы от модели снимается — hypopg даёт своё, в ответе проверенный текст без имени.
    let a = ok(read_op(
        &st,
        hypo(json!({
            "connectionId": "it", "database": "perf",
            "sql": "SELECT count(*) FROM shop.orders WHERE status = $1", "params": ["disputed"],
            "indexes": ["CREATE INDEX IF NOT EXISTS \"idx_orders_status\" ON shop.orders (status)"]
        })),
    )
    .await);
    let PgOutput::Hypo(out) = &a.body else {
        panic!("не гипотеза")
    };
    assert_eq!(out.indexes[0].ddl, "CREATE INDEX ON shop.orders (status)");
    assert!(out.indexes[0].used, "{:?}", out.indexes);
    // Отказ сервера одной из гипотез — invalid_sql; база без hypopg — FEATURE_UNAVAILABLE с подсказкой.
    let e = read_op(
        &st,
        hypo(json!({
            "connectionId": "it", "database": "perf", "sql": "SELECT 1",
            "indexes": ["CREATE INDEX ON shop.orders (no_such_column)"]
        })),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, "invalid_sql", "{}", e.message);
    let e = read_op(
        &st,
        hypo(json!({
            "connectionId": "it", "database": "postgres", "sql": "SELECT 1",
            "indexes": ["CREATE INDEX ON pg_class (relname)"]
        })),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, "FEATURE_UNAVAILABLE", "{}", e.message);
    assert!(
        e.message.contains("CREATE EXTENSION hypopg"),
        "{}",
        e.message
    );
    wait_no_mcp_sessions(&probe, "после гипотез").await;
}

/// Наблюдение активности: подписка на сэмплер узла на окно и снятие её после окна.
#[tokio::test]
async fn activity_is_observed_and_the_subscription_is_released() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st, m) = open_for_llm(&perf).await;
    let a = ok(observe_op(
        &st,
        PgObserve::Activity(serde_json::from_value(json!({"connectionId": "it"})).unwrap()),
        ObserveCtx::new(Duration::from_secs(3)),
    )
    .await);
    let PgOutput::ObservedActivity(o) = &a.body else {
        panic!("не наблюдение")
    };
    assert!(o.samples >= 1, "{o:?}");
    assert!(o.duration_ms >= 2_500, "{o:?}");
    assert_eq!(o.summary["member"], json!(m));
    assert!(a.masked.is_some(), "diagnostics — счёт литералов есть");
    assert!(!st.samplers.is_running("it", &m), "подписка окна снята");
}

/// Наблюдение pgss: два чтения и разница — запрос, выполненный в окне, есть в ответе, его текст — без литералов.
#[tokio::test]
async fn top_queries_are_observed_as_a_difference() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st, _m) = open_for_llm(&perf).await;
    let su = common::superuser(&perf, "perf").await;
    let load = tokio::spawn(async move {
        for _ in 0..20 {
            su.batch_execute("SELECT count(*) FROM shop.orders WHERE status = 'paid'")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    let a = ok(observe_op(
        &st,
        PgObserve::TopQueries(
            serde_json::from_value(json!({"connectionId": "it", "sort": "calls", "limit": 100}))
                .unwrap(),
        ),
        ObserveCtx::new(Duration::from_secs(3)),
    )
    .await);
    load.await.unwrap();
    let PgOutput::ObservedQueries(r) = &a.body else {
        panic!("не наблюдение pgss")
    };
    assert_eq!(r.tool, "queries");
    assert_eq!(r.data["mode"]["kind"], "window");
    assert!(
        r.data["coverageS"].as_u64().is_some_and(|s| s >= 3),
        "{}",
        r.data
    );
    let rows = r.data["rows"].as_array().unwrap();
    // pgss хранит текст первого запроса той же формы — с `$1` вместо литерала.
    let mine = rows
        .iter()
        .find(|row| {
            row["text"]
                .as_str()
                .is_some_and(|t| t.contains("FROM shop.orders WHERE status = $1"))
        })
        .unwrap_or_else(|| panic!("{}", r.data));
    assert!(mine["calls"].as_i64().is_some_and(|c| c >= 1), "{mine}");
    assert!(!mine["text"].as_str().unwrap().contains("'paid'"), "{mine}");
    assert!(a.masked.is_some());
}

/// Без `database` инструменты работают с базой подключения по умолчанию (`database` из `list_connections`): профиль `it`
/// смотрит в `perf`, пустая строка — то же, что нет аргумента.
#[tokio::test]
async fn omitted_database_uses_the_connection_database() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st, _m) = open_for_llm(&perf).await;
    let probe = common::superuser(&perf, "perf").await;
    let a = ok(read_op(&st, PgRead::ListConnections).await);
    let PgOutput::Connections(list) = a.body else {
        panic!("не список подключений")
    };
    assert_eq!(list[0].database, "perf");
    for blank in [None, Some("")] {
        let mut v = json!({"connectionId": "it", "sql": "SELECT count(*) FROM shop.orders"});
        if let Some(b) = blank {
            v["database"] = json!(b);
        }
        let a = ok(read_op(&st, explain(v)).await);
        let PgOutput::Explain(out) = &a.body else {
            panic!("не план")
        };
        assert!(
            out.tables.contains(&"shop.orders".to_string()),
            "{:?}",
            out.tables
        );
    }
    let a = ok(read_op(
        &st,
        hypo(json!({
            "connectionId": "it", "sql": "SELECT count(*) FROM shop.orders WHERE status = $1",
            "params": ["disputed"], "indexes": ["CREATE INDEX ON shop.orders (status)"]
        })),
    )
    .await);
    assert!(matches!(a.body, PgOutput::Hypo(_)));
    let a = ok(read_op(
        &st,
        PgRead::TableDetails(
            serde_json::from_value(
                json!({"connectionId": "it", "schema": "shop", "table": "orders"}),
            )
            .unwrap(),
        ),
    )
    .await);
    let PgOutput::Table(t) = &a.body else {
        panic!("не таблица")
    };
    assert_eq!(serde_json::to_value(t).unwrap()["table"]["name"], "orders");
    // Карточка реального запроса из pgss базы `perf` без `database`: база подставлена из профиля, карточка — этого запроса.
    const PROBE: &str = "SELECT count(*) FROM shop.orders WHERE status = 'disputed'";
    probe.batch_execute(PROBE).await.unwrap();
    let row = probe
        .query_one(
            "SELECT queryid::text, query FROM pg_stat_statements \
             WHERE dbid = (SELECT oid FROM pg_database WHERE datname = 'perf') \
               AND query LIKE '%FROM shop.orders WHERE status = $1%' \
               AND query NOT LIKE '%pg_stat_statements%' \
             ORDER BY calls DESC LIMIT 1",
            &[],
        )
        .await
        .unwrap();
    let (qid, pgss_text): (String, String) = (row.get(0), row.get(1));
    let a = ok(read_op(
        &st,
        PgRead::QueryDetails(
            serde_json::from_value(json!({"connectionId": "it", "queryId": qid})).unwrap(),
        ),
    )
    .await);
    let PgOutput::QueryDetails(v) = &a.body else {
        panic!("не карточка")
    };
    assert_eq!(v["text"], json!(pgss_text), "карточка запроса {qid}: {v}");
    wait_no_mcp_sessions(&probe, "после вызовов без database").await;
}

/// `run_select` подключения `it` без узла и базы (узел — единственный, база — `perf`).
async fn run_select(st: &AppState, sql: &str) -> SelectOut {
    let p = RunSelectParams {
        connection_id: "it".into(),
        member: None,
        database: None,
        sql: sql.into(),
        max_rows: None,
    };
    let a = ok(select::run_select(st, p, ObserveCtx::new(Duration::from_secs(10))).await);
    let PgOutput::Select(out) = a.body else {
        panic!("не выборка")
    };
    assert_eq!(a.rows, Some(out.row_count), "в журнал — число строк");
    out
}

/// `run_select` (P4b, решение владельца Q5): значения обрезаются на сервере. Обёртка курсора (`select::cursor_query`)
/// отдаёт не больше 2049 символов значения — 10 млн символов по проводу не идут. `run_select` целиком: колонки — по
/// выражению модели, текст значений — как у функции вывода типа (`t`/`f`, `inet` без `/32`, `char(n)` с пробелами),
/// NULL остаётся NULL, порядок подзапроса сохраняется; хвост `; -- …` и выражение без колонок обёртке не мешают. После
/// вызовов сессий SQL модели нет.
#[tokio::test]
async fn run_select_cuts_long_values_on_the_server() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st, m) = open_for_llm(&perf).await;
    let mut p = st.profile("it").unwrap();
    p.llm_access = LlmAccess::Data;
    st.connections.upsert(p).unwrap();
    let probe = common::superuser(&perf, "perf").await;

    let tx = guard::begin(&st, "it", Some(&m), "perf", select::SELECT_TIMEOUT_S)
        .await
        .unwrap();
    let wrapped = select::cursor_query("SELECT repeat('€', 10000000), NULL::text, true", 3);
    let messages = tx.client().simple_query(&wrapped).await.unwrap();
    let row = messages
        .iter()
        .find_map(|m| match m {
            SimpleQueryMessage::Row(r) => Some(r),
            _ => None,
        })
        .unwrap();
    let value = row.get(0).unwrap();
    assert_eq!(value.chars().count(), select::SERVER_CUT_CHARS);
    assert_eq!(
        value.len(),
        3 * select::SERVER_CUT_CHARS,
        "по проводу — 2049 символов «€»"
    );
    assert_eq!((row.get(1), row.get(2)), (None, Some("t")));
    tx.finish().await;

    let started = std::time::Instant::now();
    let out = run_select(
        &st,
        "SELECT g, repeat('x', 1000000) AS v, NULL AS n, g > 1 AS b, '10.0.0.1'::inet AS ip, 'a '::char(3) AS c \
         FROM generate_series(1, 30) g ORDER BY g DESC; -- хвост",
    )
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    let columns: Vec<(&str, &str)> = out
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.type_name.as_str()))
        .collect();
    assert_eq!(
        columns,
        [
            ("g", "int4"),
            ("v", "text"),
            ("n", "text"),
            ("b", "bool"),
            ("ip", "inet"),
            ("c", "bpchar")
        ]
    );
    assert_eq!(
        (out.row_count, out.truncated, out.truncated_values),
        (30, false, 30)
    );
    let cut = format!(
        "{}{}",
        "x".repeat(select::MAX_VALUE_BYTES),
        select::TRUNCATED_MARK
    );
    let some = |v: &str| Some(v.to_string());
    assert_eq!(
        out.rows[0],
        [
            some("30"),
            Some(cut),
            None,
            some("t"),
            some("10.0.0.1"),
            some("a  ")
        ]
    );
    assert_eq!(
        (out.rows[29][0].as_deref(), out.rows[29][3].as_deref()),
        (Some("1"), Some("f"))
    );

    let out = run_select(&st, "SELECT FROM generate_series(1, 3)").await;
    assert!(out.columns.is_empty());
    assert_eq!(out.rows, vec![Vec::<Option<String>>::new(); 3]);
    wait_no_mcp_sessions(&probe, "после run_select сессий SQL модели нет").await;
}
