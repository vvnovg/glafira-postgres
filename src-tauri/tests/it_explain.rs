//! EXPLAIN и EXPLAIN ANALYZE на стенде (спец. P3 §8): ярусы, параметры, GENERIC_PLAN, журнал, защита read-only
//! и prod, таймаут, вторая инструкция, реплика. `GPG_IT_PERF` — pg-perf, `GPG_IT_MATRIX` — PG 15 для отказа
//! GENERIC_PLAN, `GPG_IT_REPLICA` — реплика пары. Запускать с `--test-threads=1`.
//!
//! Роли: ярус чтения на pg-perf — `glafira_diag` (`pg_read_all_data`, писать не может); ярус записи и реплика —
//! `glafira_app` (права записи в `shop`; на паре у `glafira_diag` нет прав на `shop`). Стенд не меняется: запись всегда
//! откатывается, а побочный эффект в ярусе чтения проверяется на `lo_creat` (транзакционен, откат его отменяет), а не
//! на `nextval` (сдвиг последовательности откат не отменяет).

mod common;

use glafira_pg_lib::actions::Confirmation;
use glafira_pg_lib::actions::journal::Outcome;
use glafira_pg_lib::conn::profile::Environment;
use glafira_pg_lib::diag::queries::{QueriesMode, QueriesParams};
use glafira_pg_lib::diag::{self, DiagData, DiagRequest, Tool};
use glafira_pg_lib::error::ErrorCode;
use glafira_pg_lib::explain::llm::{self, LlmSource};
use glafira_pg_lib::explain::{self, ExplainRequest, Mode, Params, Tier};
use pg_advisor::queries::QuerySort;

fn req(member: &str, database: &str, sql: &str, mode: Mode, params: Params) -> ExplainRequest {
    ExplainRequest {
        member: member.into(),
        database: database.into(),
        sql: sql.into(),
        mode,
        params,
        timeout_s: 30,
        search_path: None,
        confirmation: Confirmation {
            confirm: None,
            typed_name: None,
        },
        force_write: false,
    }
}

fn values(v: &[&str]) -> Params {
    Params::Values {
        values: v.iter().map(|s| Some((*s).to_string())).collect(),
    }
}

async fn member_of(st: &glafira_pg_lib::state::AppState) -> String {
    st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone()
}

#[tokio::test]
async fn plan_and_analyze_read_tier_on_perf() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let m = member_of(&st).await;
    // Пробный план с параметром-значением: тип выводит сервер.
    let r = explain::run::run(
        &st,
        "it",
        &req(
            &m,
            "perf",
            "SELECT count(*) FROM shop.orders WHERE status = $1",
            Mode::Plan,
            values(&["disputed"]),
        ),
    )
    .await
    .unwrap();
    assert_eq!((r.mode, r.tier), (Mode::Plan, Tier::Read));
    assert!(r.plan.root.actual.is_none());
    assert!(
        r.relations
            .iter()
            .any(|x| x.schema.as_deref() == Some("shop") && x.name == "orders"),
        "{:?}",
        r.relations
    );
    assert!(r.raw.is_array());
    // GENERIC_PLAN на 17.
    let r = explain::run::run(
        &st,
        "it",
        &req(
            &m,
            "perf",
            "SELECT * FROM shop.customers WHERE id = $1",
            Mode::Plan,
            Params::Generic,
        ),
    )
    .await
    .unwrap();
    assert!(r.warnings.iter().any(|w| w == "genericPlan"));
    assert_eq!(r.plan.root.node_type, "Index Scan");
    assert!(
        r.plan
            .root
            .index_cond
            .as_deref()
            .is_some_and(|c| c.contains("$1")),
        "обобщённый план, а не по значению: {:?}",
        r.plan.root.index_cond
    );
    // Тип `$1` не выводится (42P18 при разборе расширенным протоколом), но инструкция одна — обобщённый план строится.
    let r = explain::run::run(
        &st,
        "it",
        &req(
            &m,
            "perf",
            "SELECT 1 WHERE $1 IS NULL",
            Mode::Plan,
            Params::Generic,
        ),
    )
    .await
    .unwrap();
    assert_eq!(r.plan.root.node_type, "Result");
    // Ошибки разбора и значений (классы 42 и 22) — INVALID_INPUT; позиция — если сервер её дал.
    let e = explain::run::run(
        &st,
        "it",
        &req(
            &m,
            "perf",
            "SELECT * FROM shop.nope",
            Mode::Plan,
            Params::None,
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str(), e.details.as_deref()),
        (
            ErrorCode::InvalidInput,
            "errors.explain.server",
            Some("position 15")
        ),
        "{e:?}"
    );
    let e = explain::run::run(
        &st,
        "it",
        &req(
            &m,
            "perf",
            "SELECT * FROM shop.customers WHERE id = $1",
            Mode::Plan,
            values(&["abc"]),
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str(), e.details.as_deref()),
        (ErrorCode::InvalidInput, "errors.explain.server", None),
        "{e:?}"
    );
    // EXPLAIN пишущей инструкции без прав записи (роль чтения): 42501 уже на пробном плане, подсказка — о правах записи.
    let e = explain::run::run(
        &st,
        "it",
        &req(
            &m,
            "perf",
            "UPDATE shop.balances SET balance = balance WHERE customer_id = $1",
            Mode::Plan,
            values(&["1"]),
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str()),
        (
            ErrorCode::NotAuthorized,
            "errors.explain.notAuthorizedWrite"
        ),
        "{e:?}"
    );
    assert!(
        e.details.as_deref().is_some_and(|d| d.contains("balances")),
        "{e:?}"
    );
    // ANALYZE чтения — без подтверждения и без строки в журнале.
    let r = explain::run::run(
        &st,
        "it",
        &req(
            &m,
            "perf",
            "SELECT count(*) FROM shop.customers WHERE id < $1",
            Mode::Analyze,
            values(&["100"]),
        ),
    )
    .await
    .unwrap();
    assert_eq!((r.mode, r.tier), (Mode::Analyze, Tier::Read));
    assert!(r.plan.root.actual.is_some() && r.plan.execution_ms.is_some());
    assert!(r.plan.root.buffers.is_some(), "BUFFERS");
    assert!(
        st.actions.list(Some("it"), 10).is_empty(),
        "чтение в журнал не пишется"
    );
    // nextval без права USAGE на последовательность: сервер проверяет права раньше READ ONLY — 42501.
    let e = explain::run::run(
        &st,
        "it",
        &req(
            &m,
            "perf",
            "SELECT nextval('shop.customers_id_seq')",
            Mode::Analyze,
            Params::None,
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str()),
        (ErrorCode::NotAuthorized, "errors.explain.notAuthorized")
    );
    // Побочный эффект в ярусе чтения: lo_creat в READ ONLY → EXPLAIN_WRITES.
    let su = common::superuser(&perf, "perf").await;
    let los = "SELECT count(*) FROM pg_largeobject_metadata";
    let before: i64 = su.query_one(los, &[]).await.unwrap().get(0);
    let e = explain::run::run(
        &st,
        "it",
        &req(
            &m,
            "perf",
            "SELECT lo_creat(-1)",
            Mode::Analyze,
            Params::None,
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str()),
        (ErrorCode::ExplainWrites, "errors.explain.writes")
    );
    // «Выполнить как запись»: тот же запрос с forceWrite идёт ярусом записи (dev — без подтверждения) и пишется в журнал.
    let mut forced = req(
        &m,
        "perf",
        "SELECT lo_creat(-1)",
        Mode::Analyze,
        Params::None,
    );
    forced.force_write = true;
    let r = explain::run::run(&st, "it", &forced).await.unwrap();
    assert_eq!(r.tier, Tier::Write);
    let records = st.actions.list(Some("it"), 10);
    assert_eq!(records.len(), 1, "ярус записи — строка журнала");
    let rec = &records[0];
    assert_eq!(
        (rec.action.as_str(), rec.outcome),
        ("explainAnalyze", Outcome::Ok)
    );
    let ex = rec.explain.as_ref().expect("поле explain");
    assert_eq!((ex.database.as_str(), ex.timeout_s), ("perf", 30));
    assert!(
        ex.tables.is_empty(),
        "у lo_creat таблиц нет: {:?}",
        ex.tables
    );
    assert_eq!(ex.sql_head, "SELECT lo_creat(-1)");
    let after: i64 = su.query_one(los, &[]).await.unwrap().get(0);
    assert_eq!(before, after, "запись откатана");
    // Вторая инструкция и таймаут.
    let e = explain::run::run(
        &st,
        "it",
        &req(&m, "perf", "SELECT 1; SELECT 2", Mode::Plan, Params::None),
    )
    .await
    .unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str()),
        (ErrorCode::InvalidInput, "errors.explain.oneStatement")
    );
    let mut slow = req(
        &m,
        "perf",
        "SELECT pg_sleep(10)",
        Mode::Analyze,
        Params::None,
    );
    slow.timeout_s = 5;
    let e = explain::run::run(&st, "it", &slow).await.unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str()),
        (ErrorCode::Timeout, "errors.explain.timeout")
    );
    // Синтаксическая ошибка — сообщение сервера и позиция в тексте пользователя (с ведущими пробелами, без `EXPLAIN (…)`).
    let e = explain::run::run(
        &st,
        "it",
        &req(&m, "perf", "  SELEC 1", Mode::Plan, Params::None),
    )
    .await
    .unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str()),
        (ErrorCode::InvalidInput, "errors.explain.server")
    );
    assert_eq!(e.details.as_deref(), Some("position 3"), "{e:?}");
}

/// Административная функция (спец. §15 п. 10): READ ONLY её не остановит, поэтому вызов в VERBOSE-плане пробы —
/// ярус записи с предупреждением `adminFunction`. План без ANALYZE функцию не выполняет.
#[tokio::test]
async fn admin_function_call_moves_the_statement_to_the_write_tier() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st) =
        common::open_server_as(&perf, "perf", common::APP_USER, common::APP_PASSWORD).await;
    let m = member_of(&st).await;
    for sql in [
        "SELECT pg_reload_conf()",
        "SELECT pg_catalog.pg_cancel_backend(pid) FROM pg_stat_activity WHERE pid = 0",
    ] {
        let r = explain::run::run(&st, "it", &req(&m, "perf", sql, Mode::Plan, Params::None))
            .await
            .unwrap();
        assert_eq!(r.tier, Tier::Write, "{sql}");
        assert!(
            r.warnings.iter().any(|w| w == "adminFunction"),
            "{sql}: {:?}",
            r.warnings
        );
    }
    let r = explain::run::run(
        &st,
        "it",
        &req(
            &m,
            "perf",
            "SELECT count(*) FROM pg_stat_activity",
            Mode::Plan,
            Params::None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(r.tier, Tier::Read);
    assert!(r.warnings.iter().all(|w| w != "adminFunction"));
}

#[tokio::test]
async fn write_tier_rolls_back_journals_and_respects_guard() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st) =
        common::open_server_as(&perf, "perf", common::APP_USER, common::APP_PASSWORD).await;
    let m = member_of(&st).await;
    let su = common::superuser(&perf, "perf").await;
    let before: String = su
        .query_one(
            "SELECT balance::text FROM shop.balances WHERE customer_id = 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let sql = "UPDATE shop.balances SET balance = balance + 100 WHERE customer_id = $1";
    // Пробный план показывает ярус записи без выполнения.
    let r = explain::run::run(&st, "it", &req(&m, "perf", sql, Mode::Plan, values(&["1"])))
        .await
        .unwrap();
    assert_eq!(r.tier, Tier::Write);
    assert_eq!(
        r.relations
            .iter()
            .map(|x| x.name.as_str())
            .collect::<Vec<_>>(),
        ["balances"]
    );
    // Dev-профиль: подтверждения не нужно; выполнено и откатано; строка журнала.
    let r = explain::run::run(
        &st,
        "it",
        &req(&m, "perf", sql, Mode::Analyze, values(&["1"])),
    )
    .await
    .unwrap();
    assert_eq!((r.mode, r.tier), (Mode::Analyze, Tier::Write));
    assert!(r.plan.root.actual.is_some());
    let after: String = su
        .query_one(
            "SELECT balance::text FROM shop.balances WHERE customer_id = 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, after, "изменения откатаны");
    let records = st.actions.list(Some("it"), 10);
    assert_eq!(records.len(), 1);
    let rec = &records[0];
    assert_eq!(
        (rec.action.as_str(), rec.outcome, rec.member.as_deref()),
        ("explainAnalyze", Outcome::Ok, Some(m.as_str()))
    );
    let ex = rec.explain.as_ref().expect("поле explain");
    assert_eq!(
        (ex.database.as_str(), ex.tables.as_slice(), ex.timeout_s),
        ("perf", &["shop.balances".to_string()][..], 30)
    );
    assert!(ex.sql_head.starts_with("UPDATE shop.balances"));
    // Prod без имени подключения — отказ до сервера; с именем — проходит.
    let mut p = st.profile("it").unwrap();
    p.environment = Environment::Prod;
    st.connections.upsert(p.clone()).unwrap();
    let e = explain::run::run(
        &st,
        "it",
        &req(&m, "perf", sql, Mode::Analyze, values(&["1"])),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationRequired);
    let mut ok = req(&m, "perf", sql, Mode::Analyze, values(&["1"]));
    ok.confirmation.confirm = Some("it".into());
    let r = explain::run::run(&st, "it", &ok).await.unwrap();
    assert_eq!(r.tier, Tier::Write);
    assert!(st.actions.list(Some("it"), 10)[0].prod_confirmed);
    // Read-only профиль: ярус записи закрыт, ярус чтения работает.
    p.environment = Environment::Dev;
    p.read_only = true;
    st.connections.upsert(p).unwrap();
    let e = explain::run::run(
        &st,
        "it",
        &req(&m, "perf", sql, Mode::Analyze, values(&["1"])),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::ReadOnly);
    assert!(
        explain::run::run(
            &st,
            "it",
            &req(&m, "perf", "SELECT 1", Mode::Analyze, Params::None)
        )
        .await
        .is_ok()
    );
    assert_eq!(
        st.actions.list(Some("it"), 10).len(),
        2,
        "отказы guard в журнал не пишутся"
    );
}

/// «Выполнить как запись» (`forceWrite`) — только с ANALYZE (в режиме «план» — отказ до сервера): ограда проверяется до
/// пробного плана, а проба и ANALYZE идут вне READ ONLY — побочный эффект планирования (25006 уже в пробе) больше не
/// тупик. Режим транзакции виден по оценке и факту строк `LIMIT`: планировщик вычисляет в ней стабильную
/// `current_setting('transaction_read_only')`. Стенд не меняется.
#[tokio::test]
async fn force_write_checks_the_guard_first_and_probes_outside_read_only() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let m = member_of(&st).await;
    let sql = "SELECT g FROM generate_series(1, 100) g \
               LIMIT CASE WHEN current_setting('transaction_read_only') = 'on' THEN 1 ELSE 2 END";
    let plain = explain::run::run(&st, "it", &req(&m, "perf", sql, Mode::Plan, Params::None))
        .await
        .unwrap();
    assert_eq!(
        (plain.tier, plain.plan.root.plan_rows),
        (Tier::Read, 1.0),
        "без forceWrite проба — в READ ONLY"
    );
    // forceWrite в режиме «план» — отказ до сервера: проба вне READ ONLY без ANALYZE и журнала не строится.
    let mut forced = req(&m, "perf", sql, Mode::Plan, Params::None);
    forced.force_write = true;
    let e = explain::run::run(&st, "it", &forced).await.unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str()),
        (ErrorCode::InvalidInput, "errors.explain.forceWritePlan")
    );
    assert!(st.actions.list(Some("it"), 10).is_empty());
    // Prod без подтверждения: отказ ограды раньше пробы — даже ошибка разбора до сервера не доходит.
    let mut p = st.profile("it").unwrap();
    p.environment = Environment::Prod;
    st.connections.upsert(p.clone()).unwrap();
    let mut broken = req(
        &m,
        "perf",
        "SELEC lo_creat(-1)",
        Mode::Analyze,
        Params::None,
    );
    let e = explain::run::run(&st, "it", &broken).await.unwrap_err();
    assert_eq!(
        e.code,
        ErrorCode::InvalidInput,
        "без forceWrite — сначала проба"
    );
    broken.force_write = true;
    let e = explain::run::run(&st, "it", &broken).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationRequired, "{e:?}");
    // С подтверждением: проба и ANALYZE вне READ ONLY, ярус записи, строка журнала.
    let mut confirmed = req(&m, "perf", sql, Mode::Analyze, Params::None);
    confirmed.force_write = true;
    confirmed.confirmation.confirm = Some("it".into());
    let r = explain::run::run(&st, "it", &confirmed).await.unwrap();
    assert_eq!((r.mode, r.tier), (Mode::Analyze, Tier::Write));
    assert_eq!(
        (
            r.plan.root.plan_rows,
            r.plan.root.actual.as_ref().map(|a| a.rows)
        ),
        (2.0, Some(2.0)),
        "ANALYZE с forceWrite — вне READ ONLY"
    );
    assert!(
        r.warnings.iter().all(|w| w != "probeWithoutReadOnly"),
        "{:?}",
        r.warnings
    );
    let records = st.actions.list(Some("it"), 10);
    assert_eq!(records.len(), 1, "ярус записи — строка журнала");
    assert_eq!(
        (
            records[0].action.as_str(),
            records[0].outcome,
            records[0].prod_confirmed
        ),
        ("explainAnalyze", Outcome::Ok, true)
    );
    // Read-only профиль: forceWrite отказан до сервера и в журнал не пишется.
    p.environment = Environment::Dev;
    p.read_only = true;
    st.connections.upsert(p).unwrap();
    let mut ro = req(&m, "perf", sql, Mode::Analyze, Params::None);
    ro.force_write = true;
    let e = explain::run::run(&st, "it", &ro).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ReadOnly);
    assert_eq!(st.actions.list(Some("it"), 10).len(), 1);
}

#[tokio::test]
async fn generic_plan_is_refused_before_16_and_writes_are_refused_on_a_replica() {
    if let Some(pg15) = common::matrix_addrs()
        .into_iter()
        .find(|a| a.ends_with(":5515"))
    {
        let (_dir, st) = common::open_server(&pg15).await;
        let m = member_of(&st).await;
        let e = explain::run::run(
            &st,
            "it",
            &req(
                &m,
                "postgres",
                "SELECT $1::int",
                Mode::Plan,
                Params::Generic,
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (
                ErrorCode::FeatureUnavailable,
                "errors.explain.genericUnsupported"
            )
        );
        assert!(
            explain::run::run(
                &st,
                "it",
                &req(&m, "postgres", "SELECT $1::int", Mode::Plan, values(&["1"]))
            )
            .await
            .is_ok()
        );
    }
    if let Some(replica) = common::env("GPG_IT_REPLICA") {
        let (_dir, st) =
            common::open_server_as(&replica, "orders", common::APP_USER, common::APP_PASSWORD)
                .await;
        let m = member_of(&st).await;
        let sql = "UPDATE shop.orders SET status = status WHERE id = 1";
        let r = explain::run::run(&st, "it", &req(&m, "orders", sql, Mode::Plan, Params::None))
            .await
            .unwrap();
        assert_eq!(r.tier, Tier::Write, "пробный план строится и на реплике");
        let e = explain::run::run(
            &st,
            "it",
            &req(&m, "orders", sql, Mode::Analyze, Params::None),
        )
        .await
        .unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (
                ErrorCode::FeatureUnavailable,
                "errors.explain.writeOnReplica"
            )
        );
        // Побочный эффект на реплике: ярус чтения → EXPLAIN_WRITES; «выполнить как запись» отказывает до сервера
        // (`writeOnReplica`), а не снова EXPLAIN_WRITES — петли forceWrite нет.
        let lo = req(
            &m,
            "orders",
            "SELECT lo_creat(-1)",
            Mode::Analyze,
            Params::None,
        );
        let e = explain::run::run(&st, "it", &lo).await.unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::ExplainWrites, "errors.explain.writes")
        );
        let mut forced = lo.clone();
        forced.force_write = true;
        let e = explain::run::run(&st, "it", &forced).await.unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (
                ErrorCode::FeatureUnavailable,
                "errors.explain.writeOnReplica"
            )
        );
        // forceWrite в режиме «план» отказан ещё раньше — проверкой запроса, до соединения с сервером.
        forced.mode = Mode::Plan;
        let e = explain::run::run(&st, "it", &forced).await.unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::InvalidInput, "errors.explain.forceWritePlan")
        );
        assert!(
            st.actions.list(Some("it"), 10).is_empty(),
            "отказ до сервера в журнал не пишется"
        );
        assert!(
            explain::run::run(
                &st,
                "it",
                &req(
                    &m,
                    "orders",
                    "SELECT count(*) FROM shop.orders",
                    Mode::Analyze,
                    Params::None
                )
            )
            .await
            .is_ok()
        );
    }
}

#[tokio::test]
async fn llm_bundle_has_text_stats_plan_and_ddl() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let m = member_of(&st).await;
    // Строка pgss — от самого теста, `heavy.sql` не нужен: суперпользователь выполняет точечный выбор с литералами
    // простым протоколом, pgss нормализует его в `id = $1`. Выбор с одним `$1` строится GENERIC_PLAN (у `$1 + $2` с типом
    // `unknown` у обоих параметров — «operator is not unique»).
    let seed = common::superuser(&perf, "perf").await;
    for i in 1..=50 {
        seed.batch_execute(&format!("SELECT * FROM shop.customers WHERE id = {i}"))
            .await
            .unwrap();
    }
    let r = diag::run(
        &st,
        "it",
        DiagRequest {
            tool: Tool::Queries,
            member: None,
            window_s: None,
            queries: Some(QueriesParams {
                mode: QueriesMode::SinceReset,
                sort: QuerySort::Calls,
                limit: 200,
                database: None,
                toplevel_only: true,
            }),
        },
    )
    .await
    .unwrap();
    let DiagData::Queries(d) = &r.data else {
        unreachable!()
    };
    let keys: Vec<_> = d
        .rows
        .iter()
        .filter(|x| x.rolname.as_deref() == Some("postgres"))
        .map(|x| x.key)
        .collect();
    let qid = diag::queries::texts(&st, "it", Some(&m), keys)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.text.as_deref() == Some("SELECT * FROM shop.customers WHERE id = $1"))
        .map(|t| t.key.query_id)
        .expect("точечный выбор покупателя в строках pgss");
    let b = llm::bundle(
        &st,
        "it",
        &m,
        "perf",
        LlmSource::Query { query_id: qid },
        None,
    )
    .await
    .unwrap();
    assert!(b.text.as_deref().is_some_and(|t| !t.is_empty()));
    assert!(b.stats.as_ref().is_some_and(|s| s.since_reset.is_some()));
    assert!(
        b.plan_text.as_deref().is_some_and(|p| p.contains("(cost=")),
        "{:?} {:?} {:?}",
        b.plan_text,
        b.warnings,
        b.text
    );
    assert!(
        !b.tables.is_empty() && b.tables.iter().all(|t| t.ddl.contains("CREATE TABLE")),
        "{:?}",
        b.tables
    );
    assert!(b.server_version_num >= 170_000 && b.pgss_version.is_some());
    assert!(
        b.warnings.is_empty() && b.tables_omitted == 0,
        "{:?} {}",
        b.warnings,
        b.tables_omitted
    );
    assert!(
        b.tables.iter().any(|t| t.name == "customers"),
        "{:?}",
        b.tables
    );
    // Источник — SQL консоли: статистики нет, план и таблицы есть.
    let b = llm::bundle(
        &st,
        "it",
        &m,
        "perf",
        LlmSource::Sql {
            sql: "SELECT count(*) FROM shop.orders WHERE status = 'new'".into(),
        },
        None,
    )
    .await
    .unwrap();
    assert!(b.stats.is_none() && b.plan_text.is_some());
    assert!(
        b.warnings.is_empty() && b.tables_omitted == 0,
        "{:?} {}",
        b.warnings,
        b.tables_omitted
    );
    assert_eq!(
        b.tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["orders"]
    );
}

/// Данные без плана и без части DDL остаются доступны: ошибка пробного плана (в том числе прав) и ошибка чтения
/// DDL — предупреждения `planFailed` и `ddlFailed`, а не отказ команды.
#[tokio::test]
async fn llm_bundle_keeps_text_when_the_plan_or_the_ddl_cannot_be_read() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let m = member_of(&st).await;
    let sql_source = |sql: &str| LlmSource::Sql { sql: sql.into() };
    // `unknown + unknown`: GENERIC_PLAN не строится, сервер отвергает запрос.
    let b = llm::bundle(
        &st,
        "it",
        &m,
        "perf",
        sql_source("SELECT * FROM shop.customers WHERE id = $1 + $2"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(b.warnings, ["planFailed"]);
    assert!(b.plan_text.is_none() && b.tables.is_empty() && b.tables_omitted == 0);
    assert!(b.text.is_some());
    // Запись у `glafira_diag` без прав на таблицу: NOT_AUTHORIZED пробы — тоже предупреждение, текст остаётся.
    let update = "UPDATE shop.balances SET balance = balance WHERE customer_id = 1";
    let b = llm::bundle(&st, "it", &m, "perf", sql_source(update), None)
        .await
        .unwrap();
    assert_eq!(b.warnings, ["planFailed"]);
    assert!(b.plan_text.is_none() && b.tables.is_empty() && b.tables_omitted == 0);
    assert_eq!(b.text.as_deref(), Some(update));
    // Переданный план ссылается на таблицу, которой нет: DDL не читается — `ddlFailed` и счёт в `tables_omitted`.
    let ok = explain::run::run(
        &st,
        "it",
        &req(
            &m,
            "perf",
            "SELECT count(*) FROM shop.orders",
            Mode::Plan,
            Params::None,
        ),
    )
    .await
    .unwrap();
    let ghost = serde_json::to_string(&ok.plan)
        .unwrap()
        .replace("\"orders\"", "\"no_such_table\"");
    let plan = serde_json::from_str(&ghost).unwrap();
    let b = llm::bundle(
        &st,
        "it",
        &m,
        "perf",
        sql_source("SELECT count(*) FROM shop.orders"),
        Some(plan),
    )
    .await
    .unwrap();
    assert_eq!(b.warnings, ["ddlFailed"]);
    assert!(b.plan_text.is_some() && b.tables.is_empty() && b.tables_omitted == 1);
}
