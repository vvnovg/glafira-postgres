//! Запросы на оптимизацию и предложения (спец. P4 §8, §9, §13 «Интеграция»): запрос «из консоли» на pg-perf
//! (`GPG_IT_PERF`) через `mcp::llm::create_request` → `get_optimization_request` → `propose_query_rewrite` (ошибки разбора
//! и планирования → `invalid_sql`, повтор → тот же id) → «Сравнить планы» (`compare_proposal`) с hypopg → ответ пользователя
//! (`answer_proposal`) → `list_proposals`; записи `requestCreated` и `proposalAnswered` в журнале MCP; база без hypopg;
//! `run_select` на pg-perf разрешает функции расширений; prompts и список инструментов через клиента rmcp (стенд не нужен).
//! Команды интерфейса (`create_optimization_request`, `compare_llm_proposal`, `answer_llm_proposal`) — обёртки над этими
//! функциями с событием для окна; `AppHandle` в тесте нет, и событие уходит в пустоту. Запускать с `--test-threads=1`.

mod common;
mod mcp_harness;

use std::sync::Arc;

use glafira_pg_lib::conn::profile::LlmAccess;
use glafira_pg_lib::mcp::llm::proposals::ProposalStatus;
use glafira_pg_lib::mcp::llm::requests::{OptimizationRequest, RequestSource, RequestStatus};
use glafira_pg_lib::mcp::llm::{
    CreateRequestArgs, answer_proposal, cancel_request, compare_proposal, create_request,
};
use glafira_pg_lib::secrets::MemoryStore;
use glafira_pg_lib::state::AppState;
use mcp_harness::{Gateway, audit_events, set_access};
use rmcp::model::GetPromptRequestParams;
use serde_json::{Value, json};

const ORIGINAL: &str = "SELECT count(*) FROM shop.orders WHERE status = 'disputed'";
const REWRITTEN: &str = "SELECT count(*) FROM shop.orders o WHERE o.status = 'disputed'";
const INDEX: &str = "CREATE INDEX ON shop.orders (status)";

/// Запрос на оптимизацию «из консоли» — то, что делает `create_optimization_request` без события окна.
async fn console_request(
    st: &AppState,
    member: &str,
    database: &str,
    sql: &str,
) -> OptimizationRequest {
    create_request(
        st,
        "it",
        CreateRequestArgs {
            member: member.into(),
            database: database.into(),
            source: RequestSource::Console,
            sql: sql.into(),
            params: vec![],
            plan: None,
        },
    )
    .await
    .unwrap()
}

/// Настоящих индексов на `shop.orders`: сравнение создаёт только виртуальные (hypopg).
async fn index_count(perf: &str) -> i64 {
    common::superuser(perf, "perf")
        .await
        .query_one(
            "SELECT count(*) FROM pg_indexes WHERE schemaname = 'shop' AND tablename = 'orders'",
            &[],
        )
        .await
        .unwrap()
        .get(0)
}

#[tokio::test]
async fn console_request_to_compared_and_answered_proposal() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (dir, st) = common::open_server_in(&perf, "perf").await;
    set_access(&st, "it", LlmAccess::Diagnostics);
    let gw = Gateway::start(&st, dir.path()).await; // включает MCP в настройках
    let m = mcp_harness::member(&st);
    let indexes_before = index_count(&perf).await;
    let req = console_request(&st, &m, "perf", ORIGINAL).await;
    assert!(req.id.starts_with('r') && req.id.len() == 7, "{}", req.id);
    let created = audit_events(&st, "requestCreated");
    assert_eq!(created.len(), 1, "{created:?}");
    assert_eq!(created[0]["requestId"], req.id.as_str());
    assert_eq!(created[0]["connection"]["id"], "it");
    assert!(
        !created[0].to_string().contains("disputed"),
        "SQL не в журнале"
    );

    // Модель забирает снимок: без requestId — последний ожидающий; литералы не замаскированы (§8.1).
    let v = gw.ok("get_optimization_request", json!({})).await;
    assert_eq!(v["id"], req.id.as_str());
    assert_eq!(
        (v["status"].as_str(), v["sql"].as_str()),
        (Some("waiting"), Some(ORIGINAL))
    );
    assert!(
        v["bundle"]["planText"]
            .as_str()
            .is_some_and(|p| p.contains("Seq Scan")),
        "{}",
        v["bundle"]
    );

    // Ошибки, которые модель исправит сама.
    let propose = |sql: &str, indexes: &[&str]| {
        json!({
            "requestId": req.id,
            "rewrittenSql": sql,
            "indexes": indexes,
            "rationale": "Index the filter column",
            "expectedEffect": "Index scan instead of a sequential scan"
        })
    };
    // Ошибка планирования (EXPLAIN пробы): такой таблицы нет.
    let e = gw
        .err(
            "propose_query_rewrite",
            propose(
                "SELECT count(*) FROM shop.no_such_table WHERE status = 'disputed'",
                &[],
            ),
        )
        .await;
    assert!(
        e.starts_with("invalid_sql:") && e.contains("does not exist"),
        "{e}"
    );
    let e = gw
        .err(
            "propose_query_rewrite",
            propose("DELETE FROM shop.orders WHERE status = 'disputed'", &[]),
        )
        .await;
    assert!(e.starts_with("invalid_sql:"), "другой вид инструкции: {e}");
    let e = gw
        .err(
            "propose_query_rewrite",
            propose(REWRITTEN, &["CREATE UNIQUE INDEX ON shop.orders (status)"]),
        )
        .await;
    assert!(e.starts_with("invalid_sql:"), "UNIQUE: {e}");
    let e = gw
        .err("propose_query_rewrite", propose("SELECT pg_sleep(1)", &[]))
        .await;
    assert!(e.starts_with("forbidden_function:"), "{e}");
    // Ничего из отказов не сохранилось.
    assert!(st.llm.proposals.list(&Default::default()).is_empty());

    // Предложение и его повтор — тот же id (idempotentHint).
    let v = gw
        .ok("propose_query_rewrite", propose(REWRITTEN, &[INDEX]))
        .await;
    let pid = v["proposalId"].as_str().unwrap().to_string();
    assert!(
        pid.starts_with('p') && pid.len() == 7 && v["status"] == "new",
        "{v}"
    );
    let again = gw
        .ok("propose_query_rewrite", propose(REWRITTEN, &[INDEX]))
        .await;
    assert_eq!(again["proposalId"], pid.as_str());
    let r = st.llm.requests.get(&req.id).unwrap();
    assert_eq!(r.status, RequestStatus::Answered);
    assert_eq!(r.proposal_ids, vec![pid.clone()]);
    // Ожидающих запросов больше нет.
    let e = gw.err("get_optimization_request", json!({})).await;
    assert!(e.starts_with("not_found:"), "{e}");

    // «Сравнить планы»: EXPLAIN обоих без выполнения на одном узле, предложенный — с индексом через hypopg.
    let p = compare_proposal(&st, &pid).await.unwrap();
    let c = p.comparison.expect("сравнение сохранено в предложении");
    assert!(!c.without_hypopg, "{c:?}");
    eprintln!("cost {} -> {}", c.cost_before, c.cost_after);
    assert!(
        c.cost_after < c.cost_before,
        "{} → {}",
        c.cost_before,
        c.cost_after
    );
    assert_eq!(c.indexes_used.len(), 1, "{c:?}");
    assert!(
        c.indexes_used[0].used && c.indexes_used[0].ddl == INDEX,
        "{c:?}"
    );
    assert_eq!(c.member, m);
    assert_eq!(
        index_count(&perf).await,
        indexes_before,
        "сравнение не создаёт настоящих индексов"
    );

    // Ответ пользователя виден модели; журнал получает `proposalAnswered`.
    let p = answer_proposal(
        &st,
        &pid,
        "rejected",
        Some("Must keep the same rows".into()),
    )
    .await
    .unwrap();
    assert_eq!(p.status, ProposalStatus::Rejected);
    let v = gw.ok("list_proposals", json!({"requestId": req.id})).await;
    let item = &v["proposals"][0];
    assert_eq!(item["proposalId"], pid.as_str());
    assert_eq!(
        (item["status"].as_str(), item["reason"].as_str()),
        (Some("rejected"), Some("Must keep the same rows"))
    );
    let (before, after) = (
        item["comparison"]["costBefore"].as_f64().unwrap(),
        item["comparison"]["costAfter"].as_f64().unwrap(),
    );
    assert!(after < before, "{item}");
    assert!(
        item["answeredAtMs"].is_i64() || item["answeredAtMs"].is_u64(),
        "{item}"
    );
    // Передумал: принято (повторный ответ разрешён) — вторая запись журнала.
    answer_proposal(&st, &pid, "accepted", None).await.unwrap();
    let v = gw.ok("list_proposals", json!({"requestId": req.id})).await;
    assert_eq!(v["proposals"][0]["status"], "accepted", "{v}");
    let answered = audit_events(&st, "proposalAnswered");
    assert_eq!(answered.len(), 2, "{answered:?}");
    assert_eq!(answered[0]["proposalId"], pid.as_str());
    assert_eq!(answered[0]["status"], "rejected");
    assert_eq!(answered[1]["status"], "accepted");
    assert!(
        !serde_json::to_string(&answered)
            .unwrap()
            .contains("Must keep"),
        "причина не в журнале"
    );
    let e = answer_proposal(&st, &pid, "maybe", None).await.unwrap_err();
    assert_eq!(e.key, "errors.llm.badStatus", "{e:?}");

    // Предложение по инициативе модели — без запроса; в списке подключения новые первыми.
    let v = gw
        .ok(
            "propose_query_rewrite",
            json!({
                "connectionId": "it",
                "database": "perf",
                "sql": ORIGINAL,
                "rewrittenSql": "SELECT count(*) FROM shop.orders WHERE status IN ('disputed')",
                "rationale": "Same filter written as an IN list"
            }),
        )
        .await;
    let own = v["proposalId"].as_str().unwrap().to_string();
    assert_ne!(own, pid);
    // То же без `database` и с пустой строкой — база подключения (Q2): повтор возвращает то же предложение.
    for db in [None, Some("")] {
        let mut args = json!({
            "connectionId": "it",
            "sql": ORIGINAL,
            "rewrittenSql": "SELECT count(*) FROM shop.orders WHERE status IN ('disputed')",
            "rationale": "Same filter written as an IN list"
        });
        if let Some(db) = db {
            args["database"] = db.into();
        }
        let v = gw.ok("propose_query_rewrite", args).await;
        assert_eq!(v["proposalId"], own.as_str(), "database = {db:?}");
    }
    let v = gw.ok("list_proposals", json!({"connectionId": "it"})).await;
    let ids: Vec<&str> = v["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["proposalId"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [own.as_str(), pid.as_str()]);
    assert!(
        v["proposals"][0]
            .get("requestId")
            .is_none_or(Value::is_null),
        "{v}"
    );

    // Отменённый запрос модели не отдаётся; подключение, закрытое для модели, неотличимо от отсутствующего.
    let cancelled = console_request(&st, &m, "perf", ORIGINAL).await;
    cancel_request(&st, &cancelled.id).await.unwrap();
    let e = gw
        .err(
            "get_optimization_request",
            json!({"requestId": cancelled.id}),
        )
        .await;
    assert!(e.starts_with("request_closed:"), "{e}");
    set_access(&st, "it", LlmAccess::Off);
    let e = gw
        .err("get_optimization_request", json!({"requestId": req.id}))
        .await;
    assert!(e.starts_with("not_allowed:"), "{e}");
    // Запрос на закрытом подключении приложение не создаёт.
    let e = create_request(
        &st,
        "it",
        CreateRequestArgs {
            member: m.clone(),
            database: "perf".into(),
            source: RequestSource::Console,
            sql: ORIGINAL.into(),
            params: vec![],
            plan: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(e.key, "errors.llm.accessOff");
    assert_eq!(index_count(&perf).await, indexes_before);
}

/// Запрос с типизированными литералами: pg_stat_statements хранит его как `timestamptz $1 - interval $2`.
const TYPED: &str = "SELECT count(*) FROM shop.orders o \
     WHERE o.created_at >= timestamptz '2026-01-01 00:00:00+00' - interval '1 day'";

/// Ревью правки B, #2 и #4: запрос на оптимизацию из карточки запроса — текст pgss с `timestamptz $1 - interval $2`, как
/// его отправляет “Send to LLM assistant” карточки. Снимок получает план, предложение по `requestId` (модель пишет в
/// том же стиле, `<тип> $N`) проходит пробу, «Сравнить планы» строит оба плана; хранятся тексты с приведениями `$N::тип`,
/// а `originalQueryId` связывает предложение с карточкой. Поле `sql` с приведениями — тот же запрос (M6). Предложение в
/// прямой форме по `queryId` хранит тот же текст с приведениями и тот же `originalQueryId`.
#[tokio::test]
async fn typed_literals_of_a_card_query_are_proposed_and_compared() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (dir, st) = common::open_server_in(&perf, "perf").await;
    set_access(&st, "it", LlmAccess::Diagnostics);
    let gw = Gateway::start(&st, dir.path()).await;
    let m = mcp_harness::member(&st);
    let qid = common::pgss_query_id(&perf, TYPED).await;
    let text: String = common::superuser(&perf, "perf")
        .await
        .query_one(
            "SELECT s.query FROM pg_stat_statements s JOIN pg_database d ON d.oid = s.dbid \
             WHERE s.queryid = $1 AND d.datname = 'perf' LIMIT 1",
            &[&qid],
        )
        .await
        .unwrap()
        .get(0);
    assert!(text.contains("timestamptz $1 - interval $2"), "{text}");
    let cast = text.replace(
        "timestamptz $1 - interval $2",
        "$1::timestamptz - $2::interval",
    );
    let req = create_request(
        &st,
        "it",
        CreateRequestArgs {
            member: m.clone(),
            database: "perf".into(),
            source: RequestSource::Query {
                query_id: qid.to_string(),
            },
            sql: text.clone(),
            params: vec![],
            plan: None,
        },
    )
    .await
    .unwrap();
    let v = gw
        .ok("get_optimization_request", json!({"requestId": req.id}))
        .await;
    assert_eq!(v["sql"], text.as_str(), "снимок — текст карточки как есть");
    assert!(
        v["bundle"]["planText"]
            .as_str()
            .is_some_and(|p| p.contains("orders")),
        "пробный план снимка — по тексту с приведениями: {}",
        v["bundle"]
    );

    let rewritten = "SELECT count(o.id) FROM shop.orders o \
                     WHERE o.created_at >= timestamptz $1 - interval $2";
    let propose = |extra: Value| {
        let mut args = json!({
            "requestId": req.id,
            "rewrittenSql": rewritten,
            "indexes": ["CREATE INDEX ON shop.orders (created_at)"],
            "rationale": "Index the date filter"
        });
        for (k, x) in extra.as_object().unwrap() {
            args[k] = x.clone();
        }
        args
    };
    let v = gw.ok("propose_query_rewrite", propose(json!({}))).await;
    let pid = v["proposalId"].as_str().unwrap().to_string();
    let again = gw
        .ok(
            "propose_query_rewrite",
            propose(json!({"sql": cast, "queryId": qid.to_string()})),
        )
        .await;
    assert_eq!(
        again["proposalId"],
        pid.as_str(),
        "тот же запрос и предложение"
    );
    let p = st.llm.proposals.get(&pid).unwrap();
    assert_eq!(p.original_sql, cast);
    assert_eq!(
        p.rewritten_sql,
        "SELECT count(o.id) FROM shop.orders o \
         WHERE o.created_at >= $1::timestamptz - $2::interval"
    );
    assert_eq!(p.original_query_id, Some(qid.to_string()));
    let c = compare_proposal(&st, &pid)
        .await
        .unwrap()
        .comparison
        .expect("сравнение сохранено");
    assert!(c.cost_before > 0.0 && c.cost_after > 0.0, "{c:?}");
    assert_eq!(c.indexes_used.len(), 1, "{c:?}");

    // #4: прямая форма по queryId — текст pgss с приведениями, связь с карточкой — по originalQueryId.
    let v = gw
        .ok(
            "propose_query_rewrite",
            json!({
                "connectionId": "it",
                "database": "perf",
                "queryId": qid.to_string(),
                "rewrittenSql": "SELECT count(*) FROM shop.orders WHERE created_at >= $1::timestamptz - $2::interval",
                "rationale": "The same query without the alias"
            }),
        )
        .await;
    let direct = st
        .llm
        .proposals
        .get(v["proposalId"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        (
            direct.original_sql.as_str(),
            direct.original_query_id.as_deref(),
            direct.request_id.as_deref(),
            direct.database.as_str()
        ),
        (cast.as_str(), Some(qid.to_string().as_str()), None, "perf")
    );
    let c = compare_proposal(&st, &direct.id)
        .await
        .unwrap()
        .comparison
        .expect("сравнение сохранено");
    assert!(c.cost_before > 0.0, "{c:?}");
}

/// В базе `postgres` стенда pg-perf hypopg нет: сравнение идёт без индексов и помечено (§8.4).
#[tokio::test]
async fn comparison_without_hypopg_is_marked() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (dir, st) = common::open_server_in(&perf, "postgres").await;
    set_access(&st, "it", LlmAccess::Diagnostics);
    let gw = Gateway::start(&st, dir.path()).await;
    let v = gw
        .ok(
            "propose_query_rewrite",
            json!({
                "connectionId": "it",
                "database": "postgres",
                "sql": "SELECT count(*) FROM pg_class WHERE relname = 'pg_proc'",
                "rewrittenSql": "SELECT count(*) FROM pg_class WHERE relname = 'pg_proc'::name",
                "indexes": ["CREATE INDEX ON pg_class (relkind)"],
                "rationale": "Explicit cast to name"
            }),
        )
        .await;
    let p = compare_proposal(&st, v["proposalId"].as_str().unwrap())
        .await
        .unwrap();
    let c = p.comparison.unwrap();
    assert!(c.without_hypopg, "{c:?}");
    assert!(c.indexes_used.iter().all(|u| !u.used), "{c:?}");
    assert!(c.cost_before > 0.0 && c.cost_after > 0.0, "{c:?}");
}

/// `run_select` на pg-perf: функции расширений (`pg_stat_statements`, `hypopg`) — не пользовательские, проверка по
/// `pg_proc` их пропускает (Q1).
#[tokio::test]
async fn run_select_allows_extension_functions() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (dir, st) = common::open_server_in(&perf, "perf").await;
    set_access(&st, "it", LlmAccess::Data);
    let gw = Gateway::start(&st, dir.path()).await;
    let v = gw
        .ok(
            "run_select",
            json!({"connectionId": "it", "sql": "SELECT count(*) >= 0 AS ok FROM pg_stat_statements(true)"}),
        )
        .await;
    assert_eq!(v["rows"], json!([["t"]]), "{v}");
    let v = gw
        .ok(
            "run_select",
            json!({"connectionId": "it", "sql": "SELECT count(*) FROM hypopg_list_indexes"}),
        )
        .await;
    assert_eq!(v["rows"], json!([["0"]]), "{v}");
}

/// Инструменты и prompts через бэкенд приложения: 18 инструментов, аннотации предложения, 4 prompts с аргументами,
/// текст `optimize_query` называет инструменты в порядке сценария (§9). Стенд не нужен.
#[tokio::test]
async fn tools_and_prompts_through_the_app_backend() {
    let dir = tempfile::tempdir().unwrap();
    let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
    let gw = Gateway::start(&st, dir.path()).await;

    let tools = gw.client.list_all_tools().await.unwrap();
    assert_eq!(
        tools.len(),
        18,
        "{:?}",
        tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
    );
    let hints = |name: &str| {
        let a = tools
            .iter()
            .find(|t| t.name == name)
            .and_then(|t| t.annotations.clone())
            .unwrap_or_else(|| panic!("{name}"));
        (
            a.read_only_hint,
            a.destructive_hint,
            a.idempotent_hint,
            a.open_world_hint,
        )
    };
    assert_eq!(
        hints("propose_query_rewrite"),
        (Some(false), Some(false), Some(true), Some(false))
    );
    assert_eq!(
        hints("run_select"),
        (Some(true), Some(false), Some(true), Some(false))
    );

    let prompts = gw.client.list_all_prompts().await.unwrap();
    let mut names: Vec<&str> = prompts.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "analyze_top_queries",
            "optimize_query",
            "review_config",
            "triage_incident"
        ]
    );
    let args = |name: &str| -> Vec<(String, bool)> {
        prompts
            .iter()
            .find(|p| p.name == name)
            .unwrap()
            .arguments
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|a| (a.name, a.required.unwrap_or(false)))
            .collect()
    };
    assert_eq!(args("optimize_query"), [("requestId".to_string(), false)]);
    assert_eq!(
        args("triage_incident"),
        [
            ("connectionId".to_string(), true),
            ("symptom".to_string(), false)
        ]
    );

    let text = |r: rmcp::model::GetPromptResult| -> String {
        r.messages
            .iter()
            .filter_map(|m| m.content.as_text().map(|t| t.text.clone()))
            .collect()
    };
    let r = gw
        .client
        .get_prompt(
            GetPromptRequestParams::new("optimize_query")
                .with_arguments(json!({"requestId": "r3a9f1c"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    let t = text(r);
    let get = t
        .find("get_optimization_request")
        .expect("get_optimization_request");
    let propose = t
        .find("propose_query_rewrite")
        .expect("propose_query_rewrite");
    assert!(get < propose && t.contains("r3a9f1c"), "{t}");
    let r = gw
        .client
        .get_prompt(
            GetPromptRequestParams::new("triage_incident")
                .with_arguments(json!({"connectionId": "it"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert!(text(r).contains("get_health_summary"));
}
