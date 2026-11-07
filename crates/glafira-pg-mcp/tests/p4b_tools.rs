//! Инструменты P4b через клиента rmcp: `run_select` (Task 2), предложения (Task 4), сторожок списка.

mod p4b;

use std::sync::Arc;
use std::time::Duration;

use glafira_mcp_core::error::ToolError;
use glafira_pg_mcp::model::{LlmOp, PgAnswer, PgOutput, ProposedOut, SelectColumn, SelectOut};
use glafira_pg_mcp::server::{INSTRUCTIONS, TOOL_NAMES};
use p4b::{Fake, call, client, client_named, err, ok};
use serde_json::{Value, json};

fn two_rows(member: &str) -> PgAnswer {
    PgAnswer {
        body: PgOutput::Select(SelectOut {
            member: member.into(),
            columns: vec![
                SelectColumn {
                    name: "id".into(),
                    type_name: "int4".into(),
                },
                SelectColumn {
                    name: "note".into(),
                    type_name: "text".into(),
                },
            ],
            rows: vec![
                vec![Some("1".into()), None],
                vec![Some("2".into()), Some("x".into())],
            ],
            row_count: 2,
            truncated: false,
            truncated_values: 0,
            ms: 7,
        }),
        masked: None,
        rows: Some(2),
    }
}

#[tokio::test]
async fn run_select_passes_defaults_and_returns_rows_as_text() {
    let fake = Arc::new(
        Fake::new().on_select(|op| Ok(two_rows(op.0.member.as_deref().unwrap_or("pg-2")))),
    );
    let c = client(fake.clone()).await;
    let v = ok(call(
        &c,
        "run_select",
        json!({ "connectionId": "c1", "database": "shop", "sql": "SELECT id, note FROM t LIMIT 2" }),
    )
    .await);
    assert_eq!(v["member"], "pg-2");
    assert_eq!(v["columns"][1], json!({ "name": "note", "type": "text" }));
    assert_eq!(v["rows"], json!([["1", null], ["2", "x"]]));
    assert_eq!(v["rowCount"], 2);
    assert_eq!(v["truncated"], false);
    assert_eq!(v["truncatedValues"], 0);
    assert_eq!(v["ms"], 7);
    let seen = fake.selects.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].0.0.max_rows,
        Some(100),
        "maxRows по умолчанию — 100"
    );
    assert_eq!(seen[0].0.0.database.as_deref(), Some("shop"));
    assert_eq!(
        seen[0].1,
        Duration::from_secs(10),
        "срок вызова — statement_timeout run_select"
    );
}

#[tokio::test]
async fn run_select_checks_its_parameters_before_the_backend() {
    let fake = Arc::new(Fake::new());
    let c = client(fake.clone()).await;
    for args in [
        json!({ "connectionId": "c1", "database": "shop", "sql": "SELECT 1", "maxRows": 501 }),
        json!({ "connectionId": "c1", "database": "shop", "sql": "SELECT 1", "maxRows": 0 }),
        json!({ "connectionId": "c1", "database": "shop", "sql": "   " }),
        json!({ "connectionId": "c1", "sql": "" }),
    ] {
        let e = err(&call(&c, "run_select", args.clone()).await);
        assert!(e.starts_with("invalid_argument: "), "{args}: {e}");
    }
    assert!(fake.selects.lock().unwrap().is_empty());
}

/// `database` и `member` необязательны (решение владельца Q2, как §16.21 и E2): без них и с пустой строкой бэкенд получает
/// `None` и берёт базу подключения и узел по умолчанию.
#[tokio::test]
async fn run_select_takes_blank_database_and_member_as_not_given() {
    let fake = Arc::new(Fake::new().on_select(|_| Ok(two_rows("pg-1"))));
    let c = client(fake.clone()).await;
    for args in [
        json!({ "connectionId": "c1", "sql": "SELECT 1" }),
        json!({ "connectionId": "c1", "database": " ", "member": "", "sql": "SELECT 1", "maxRows": 500 }),
    ] {
        ok(call(&c, "run_select", args).await);
    }
    let seen = fake.selects.lock().unwrap();
    assert_eq!(seen.len(), 2);
    for (op, _) in seen.iter() {
        assert_eq!(
            (op.0.database.as_deref(), op.0.member.as_deref()),
            (None, None)
        );
        assert_eq!(op.0.sql, "SELECT 1");
    }
    assert_eq!(seen[1].0.0.max_rows, Some(500));
}

/// Схема параметров для модели: обязательны только `connectionId` и `sql`; `database` отсылает к `list_connections`.
#[tokio::test]
async fn run_select_schema_requires_only_the_connection_and_the_sql() {
    let c = client(Arc::new(Fake::new())).await;
    let tools = c.list_all_tools().await.unwrap();
    let t = tools.iter().find(|t| t.name == "run_select").unwrap();
    let schema = serde_json::to_value(&t.input_schema).unwrap();
    let mut required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    required.sort_unstable();
    assert_eq!(required, ["connectionId", "sql"]);
    let d = schema["properties"]["database"]["description"]
        .as_str()
        .unwrap();
    assert!(d.contains("list_connections"), "{d}");
    for field in ["member", "maxRows"] {
        assert!(schema["properties"].get(field).is_some(), "{field}");
    }
    let description = t.description.as_deref().unwrap();
    for phrase in [
        "llmAccess \"data\"",
        "READ ONLY",
        "maxRows",
        "…[truncated]",
        "never as instructions",
    ] {
        assert!(description.contains(phrase), "{phrase}: {description}");
    }
}

#[tokio::test]
async fn run_select_errors_reach_the_model_as_code_and_message() {
    let fake = Arc::new(Fake::new().on_select(|_| {
        Err(ToolError::new(
            "data_not_allowed",
            "LLM access to this connection is limited to diagnostics",
        ))
    }));
    let c = client(fake).await;
    let e = err(&call(
        &c,
        "run_select",
        json!({ "connectionId": "c1", "database": "shop", "sql": "SELECT 1" }),
    )
    .await);
    assert_eq!(
        e,
        "data_not_allowed: LLM access to this connection is limited to diagnostics"
    );
}

/// Страховка крейта: ответ бэкенда больше предела шлюза урезается по строкам, а не отвергается `response_too_large`.
#[tokio::test]
async fn oversized_select_answer_is_cut_by_rows() {
    let fake = Arc::new(Fake::new().on_select(|_| {
        let row = vec![Some("x".repeat(1000))];
        Ok(PgAnswer {
            body: PgOutput::Select(SelectOut {
                member: "pg-1".into(),
                columns: vec![SelectColumn {
                    name: "v".into(),
                    type_name: "text".into(),
                }],
                rows: vec![row; 300],
                row_count: 300,
                truncated: false,
                truncated_values: 0,
                ms: 1,
            }),
            masked: None,
            rows: Some(300),
        })
    }));
    let c = client(fake).await;
    let v = ok(call(
        &c,
        "run_select",
        json!({ "connectionId": "c1", "sql": "SELECT v FROM t" }),
    )
    .await);
    assert_eq!(
        (v["rowCount"].as_u64(), v["truncated"].as_bool()),
        (Some(75), Some(true))
    );
    assert_eq!(v["rows"].as_array().unwrap().len(), 75);
}

#[tokio::test]
async fn sentinel_lists_exactly_the_tools_with_annotations() {
    let c = client(Arc::new(Fake::new())).await;
    let tools = c.list_all_tools().await.unwrap();
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    let mut expected: Vec<String> = TOOL_NAMES.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(names, expected, "TOOL_NAMES совпадает с маршрутизатором");
    assert_eq!(TOOL_NAMES.len(), 18);
    for name in [
        "run_select",
        "get_optimization_request",
        "propose_query_rewrite",
        "list_proposals",
    ] {
        assert!(TOOL_NAMES.contains(&name), "{name}");
    }
    let cyrillic = |s: &str| s.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c));
    for t in &tools {
        let a = t
            .annotations
            .as_ref()
            .unwrap_or_else(|| panic!("{}: нет аннотаций", t.name));
        // Все четыре подсказки явно у каждого инструмента (§5.1): Qwen Code повторяет после обрыва только
        // идемпотентные вызовы, каталоги клиентов отклоняют инструменты без подсказок. Единственный инструмент, который
        // меняет Glafira (не базу), — предложение; повтор с теми же данными возвращает то же предложение.
        let read_only = t.name != "propose_query_rewrite";
        assert_eq!(
            (
                a.read_only_hint,
                a.destructive_hint,
                a.idempotent_hint,
                a.open_world_hint
            ),
            (Some(read_only), Some(false), Some(true), Some(false)),
            "{}",
            t.name
        );
        // Тексты для модели — по-английски (описание и схема параметров).
        let description = t.description.as_deref().unwrap_or_default();
        assert!(description.len() > 30, "{}", t.name);
        assert!(!cyrillic(description), "{}", t.name);
        let schema = serde_json::to_string(&t.input_schema).unwrap();
        assert!(!cyrillic(&schema), "{}: {schema}", t.name);
    }
}

// ---- Предложения (Task 4): get_optimization_request, propose_query_rewrite, list_proposals ----

fn proposed(id: &str) -> PgAnswer {
    PgAnswer {
        body: PgOutput::Proposed(ProposedOut {
            proposal_id: id.into(),
            status: "new".into(),
        }),
        masked: None,
        rows: None,
    }
}

/// Смоук P4b, G1, G2, G4 и ревью правки A (#3, #5, #7): описания говорят модели, что убрано из ответов — нулевые
/// счётчики, `own: false`, хвосты длинных текстов топа (поиск смотрит полный текст), интервалы ожиданий без сэмплов, а
/// простой узла — одной записью `idle` — и что история карточки — не больше 12 равных интервалов времени.
#[tokio::test]
async fn descriptions_say_what_compact_answers_omit() {
    let c = client(Arc::new(Fake::new())).await;
    let tools = c.list_all_tools().await.unwrap();
    let description = |name: &str| {
        tools
            .iter()
            .find(|t| t.name == name)
            .and_then(|t| t.description.as_deref())
            .unwrap()
            .to_string()
    };
    let top = description("get_top_queries");
    for phrase in [
        "counters that are 0 are omitted",
        "calls, totalExecMs, meanExecMs and rows are always present",
        "own is present only when true",
        "first 500 characters with textTruncated true",
        // Ревью правки A, #7: совпадение может быть за обрезкой текста строки.
        "search matches the full text",
        "the match may lie after the cut",
    ] {
        assert!(top.contains(phrase), "{phrase}: {top}");
    }
    let card = description("get_query_details");
    for phrase in [
        "counters that are 0 are omitted",
        "at most 12 points",
        // Ревью правки A, #5: точки — равные интервалы времени.
        "equal time bins",
        "historyBinS",
        "bins without points are omitted",
    ] {
        assert!(card.contains(phrase), "{phrase}: {card}");
    }
    // Финальное ревью правок смоука P4b, #2: приведения `date $1` — и для `sql`, и для текста по `queryId`; пустой
    // `params` — как без значений.
    let explain = description("explain_query");
    for phrase in [
        "Typed literals as pg_stat_statements writes them (date $1) are planned as $1::date for common types, in sql and in queryId texts",
        "without values (params omitted or empty) PostgreSQL 16+ builds a generic plan",
    ] {
        assert!(explain.contains(phrase), "{phrase}: {explain}");
    }
    let diag = description("run_diagnostic");
    for phrase in [
        // Ревью правки A, #3: подряд идущие интервалы без активных сессий — одна запись.
        "waits omits intervals in which the sampler did not run",
        "{startMs, endMs, sampledS, idle: true}",
        "the form of get_top_queries rows",
    ] {
        assert!(diag.contains(phrase), "{phrase}: {diag}");
    }
}

/// Смоук P4b, M1, M2 и дополнение к правке A: свежий `list_proposals` перед ответом о решениях пользователя, статистика
/// значений — только на уровне `data` (и подсказка об этом), текст исправления находок — один раз в `fixes`; экраны
/// Glafira вместо имён инструментов (M4).
#[tokio::test]
async fn descriptions_and_instructions_carry_the_smoke_texts() {
    let c = client(Arc::new(Fake::new())).await;
    let tools = c.list_all_tools().await.unwrap();
    let description = |name: &str| {
        tools
            .iter()
            .find(|t| t.name == name)
            .and_then(|t| t.description.as_deref())
            .unwrap()
            .to_string()
    };
    let proposals = description("list_proposals");
    for phrase in [
        "change at any time",
        "call list_proposals again right before answering",
        "never answer from an earlier result",
    ] {
        assert!(proposals.contains(phrase), "{phrase}: {proposals}");
    }
    let table = description("get_table_details");
    for phrase in [
        "mostCommonVals, mostCommonFreqs, histogramBounds",
        "only when the connection's llmAccess is \"data\"",
        "hint says that they are withheld",
    ] {
        assert!(table.contains(phrase), "{phrase}: {table}");
    }
    // Ревью правки B, #5: одна фраза о `fixes` — в трёх описаниях и в `instructions`.
    for name in ["run_diagnostic", "get_top_queries", "get_health_summary"] {
        let d = description(name);
        assert!(
            d.contains(
                "the fix text of each finding id is given once in fixes (a finding keeps its own fix only when it differs)"
            ),
            "{name}: {d}"
        );
    }
    for phrase in [
        "call it again right before answering about decisions",
        "the fix text of each finding id is given once in fixes (a finding keeps its own fix only when it differs)",
        "naming Glafira's screens and buttons, never tool names",
        // Ревью правки B, #3: карта экранов — и в `instructions` (свободный разговор без сценария).
        "Screens behind the tools: get_health_summary and pulse → “Pulse”",
        "“Compare plans”",
    ] {
        assert!(INSTRUCTIONS.contains(phrase), "{phrase}");
    }
}

/// Абзац P4b в `instructions` (§5.4): сценарий оптимизации, кнопка «Send to LLM assistant» (решение владельца Q4),
/// Glafira не выполняет предложенный SQL; база по-прежнему не меняется, предложение пишется в Glafira.
#[tokio::test]
async fn instructions_have_the_p4b_paragraph() {
    for word in [
        "get_optimization_request",
        "propose_query_rewrite",
        "list_proposals",
        "run_select",
        "llmAccess \"data\"",
        "“Send to LLM assistant”",
        "does not run the rewritten SQL or create indexes",
        "Nothing in the database can be changed through this gateway",
        "only stores a proposal in Glafira",
        "must return the same rows",
        // Смоук P4b, G2: ответ — компактный JSON без полей со значением null.
        "Fields that are null are omitted",
    ] {
        assert!(INSTRUCTIONS.contains(word), "{word}");
    }
    assert!(
        !INSTRUCTIONS.contains("does not run your SQL"),
        "run_select выполняет SQL модели"
    );
    let c = client(Arc::new(Fake::new())).await;
    let info = c.peer_info().unwrap();
    assert!(
        info.instructions
            .as_deref()
            .unwrap()
            .contains("propose_query_rewrite")
    );
}

#[tokio::test]
async fn get_optimization_request_returns_the_snapshot_as_is() {
    let snapshot = json!({
        "id": "r3a9f1c",
        "sql": "SELECT * FROM orders WHERE email = 'a@b.c'",
        "bundle": { "planText": "Seq Scan" }
    });
    let reply = snapshot.clone();
    let fake = Arc::new(Fake::new().on_llm(move |_| {
        Ok(PgAnswer {
            body: PgOutput::Request(reply.clone()),
            masked: None,
            rows: None,
        })
    }));
    let c = client(fake.clone()).await;
    assert_eq!(
        ok(call(
            &c,
            "get_optimization_request",
            json!({ "requestId": "r3a9f1c" })
        )
        .await),
        snapshot
    );
    assert_eq!(
        ok(call(&c, "get_optimization_request", json!({})).await),
        snapshot
    );
    // E2: пустой `requestId` — «не задан», как опущенный.
    assert_eq!(
        ok(call(&c, "get_optimization_request", json!({ "requestId": " " })).await),
        snapshot
    );
    let ops = fake.llm_ops.lock().unwrap();
    assert!(
        matches!(&ops[0], LlmOp::GetRequest { params: p, run: None } if p.request_id.as_deref() == Some("r3a9f1c"))
    );
    assert!(
        matches!(&ops[1], LlmOp::GetRequest { params: p, run: None } if p.request_id.is_none())
    );
    assert!(
        matches!(&ops[2], LlmOp::GetRequest { params: p, run: None } if p.request_id.is_none())
    );
}

#[tokio::test]
async fn propose_checks_its_arguments_before_the_backend() {
    let fake = Arc::new(Fake::new());
    let c = client(fake.clone()).await;
    let base = json!({ "requestId": "r1", "rewrittenSql": "SELECT 1", "rationale": "faster" });
    let with = |patch: Value| {
        let mut v = base.clone();
        for (k, x) in patch.as_object().unwrap() {
            if x.is_null() {
                v.as_object_mut().unwrap().remove(k);
            } else {
                v[k] = x.clone();
            }
        }
        v
    };
    for (args, words) in [
        // M6: поля прямой формы рядом с requestId сверяет приложение; queryId — только число.
        (
            with(json!({ "queryId": "x7" })),
            "with requestId pass only rewrittenSql, indexes, rationale and expectedEffect",
        ),
        // M5: id запроса на оптимизацию вместо queryId.
        (
            with(json!({ "requestId": null, "connectionId": "c1", "queryId": "rb61c47" })),
            "rb61c47 is an optimization request id, not a queryId; pass it as requestId",
        ),
        (with(json!({ "requestId": null })), "pass requestId"),
        (with(json!({ "requestId": " " })), "pass requestId"),
        (
            with(json!({ "requestId": null, "database": "shop", "sql": "SELECT 2" })),
            "connectionId is required without requestId",
        ),
        (
            with(json!({ "requestId": null, "connectionId": "c1", "database": "shop" })),
            "pass exactly one of sql and queryId: neither was given",
        ),
        (
            with(
                json!({ "requestId": null, "connectionId": "c1", "sql": "SELECT 2", "queryId": "7" }),
            ),
            "pass exactly one of sql and queryId: both were given",
        ),
        (
            with(json!({ "requestId": null, "connectionId": "c1", "queryId": "12x" })),
            "64-bit integer",
        ),
        (
            with(json!({ "rewrittenSql": " " })),
            "rewrittenSql must not be empty",
        ),
        (
            with(json!({ "rationale": "" })),
            "rationale must not be empty",
        ),
        (
            with(json!({ "rationale": "x".repeat(4097) })),
            "rationale is longer than 4 KB",
        ),
        (
            with(json!({ "expectedEffect": "x".repeat(1025) })),
            "expectedEffect is longer than 1 KB",
        ),
        (
            with(json!({ "indexes": vec!["CREATE INDEX ON t (a)"; 6] })),
            "at most 5 indexes",
        ),
        (
            with(json!({ "indexes": [" "] })),
            "indexes must not be empty",
        ),
    ] {
        let e = err(&call(&c, "propose_query_rewrite", args.clone()).await);
        assert!(
            e.starts_with("invalid_argument: ") && e.contains(words),
            "{args}: {e}"
        );
    }
    assert!(fake.llm_ops.lock().unwrap().is_empty());
}

/// Смоук P4b, M6: модели присылают рядом с requestId поля прямой формы (connectionId, database, member, sql, queryId)
/// и раньше получали «not both» по 2–3 раза за прогон. Теперь они доходят до приложения, которое принимает совпадающие с
/// запросом; queryId, равный requestId, снимается; member не входит в параметры и не мешает.
#[tokio::test]
async fn propose_with_request_id_passes_the_direct_fields_to_the_app() {
    let fake = Arc::new(Fake::new().on_llm(|_| Ok(proposed("p0b12de"))));
    let c = client(fake.clone()).await;
    ok(call(
        &c,
        "propose_query_rewrite",
        json!({
            "requestId": "rb61c47", "connectionId": "perf", "database": "perf", "member": "pg-1",
            "sql": "SELECT 1", "queryId": "rb61c47", "rewrittenSql": "SELECT 2", "rationale": "faster"
        }),
    )
    .await);
    let ops = fake.llm_ops.lock().unwrap();
    let LlmOp::Propose { params, .. } = &ops[0] else {
        panic!("{ops:?}")
    };
    assert_eq!(params.request_id.as_deref(), Some("rb61c47"));
    assert_eq!(
        (
            params.connection_id.as_deref(),
            params.database.as_deref(),
            params.sql.as_deref(),
            params.query_id.as_deref()
        ),
        (Some("perf"), Some("perf"), Some("SELECT 1"), None)
    );
}

/// E2 и решение владельца Q2: пустые необязательные строки — «не заданы» (пустой `connectionId` рядом с `requestId` — не
/// «оба источника»), `database` в прямой форме необязателен — приложение возьмёт базу подключения.
#[tokio::test]
async fn propose_takes_blank_strings_as_not_given_and_database_as_optional() {
    let fake = Arc::new(Fake::new().on_llm(|_| Ok(proposed("p0b12de"))));
    let c = client(fake.clone()).await;
    for args in [
        json!({
            "requestId": "r1", "connectionId": "", "database": " ", "sql": "", "queryId": "\t",
            "rewrittenSql": "SELECT 1", "rationale": "faster", "expectedEffect": " "
        }),
        json!({ "connectionId": "c1", "sql": "SELECT 2", "rewrittenSql": "SELECT 1", "rationale": "faster" }),
        json!({
            "connectionId": "c1", "database": "", "queryId": " -42 ", "sql": " ",
            "rewrittenSql": "SELECT 1", "rationale": "faster", "indexes": []
        }),
    ] {
        ok(call(&c, "propose_query_rewrite", args).await);
    }
    let ops = fake.llm_ops.lock().unwrap();
    let params: Vec<_> = ops
        .iter()
        .map(|op| match op {
            LlmOp::Propose { params, .. } => params.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        (
            params[0].request_id.as_deref(),
            params[0].connection_id.as_deref(),
            params[0].database.as_deref(),
            params[0].sql.as_deref(),
            params[0].query_id.as_deref(),
            params[0].expected_effect.as_deref()
        ),
        (Some("r1"), None, None, None, None, None)
    );
    assert_eq!(
        (
            params[1].request_id.as_deref(),
            params[1].connection_id.as_deref(),
            params[1].database.as_deref(),
            params[1].sql.as_deref()
        ),
        (None, Some("c1"), None, Some("SELECT 2"))
    );
    assert_eq!(
        (
            params[2].database.as_deref(),
            params[2].sql.as_deref(),
            params[2].query_id.as_deref()
        ),
        (None, None, Some("-42"))
    );
}

#[tokio::test]
async fn propose_passes_the_client_name_and_answers_id_and_status() {
    let fake = Arc::new(Fake::new().on_llm(|_| Ok(proposed("p0b12de"))));
    let c = client_named(fake.clone(), "qwen-code").await;
    let args = json!({
        "connectionId": "c1", "database": "shop", "queryId": "-42",
        "rewrittenSql": "SELECT id FROM orders WHERE status = $1",
        "indexes": ["CREATE INDEX ON orders (status)"],
        "rationale": "Index scan instead of Seq Scan", "expectedEffect": "cost 12000 → 40"
    });
    let v = ok(call(&c, "propose_query_rewrite", args).await);
    assert_eq!(v, json!({ "proposalId": "p0b12de", "status": "new" }));
    let ops = fake.llm_ops.lock().unwrap();
    let LlmOp::Propose {
        params,
        client,
        run,
    } = &ops[0]
    else {
        panic!("{:?}", ops[0])
    };
    assert_eq!(run.as_deref(), None, "обычная сессия — без запуска");
    assert_eq!(client.as_deref(), Some("qwen-code"));
    assert_eq!(params.query_id.as_deref(), Some("-42"));
    assert_eq!(params.database.as_deref(), Some("shop"));
    assert_eq!(params.indexes.as_ref().unwrap().len(), 1);
}

/// Схема параметров для модели: обязательны только `rewrittenSql` и `rationale`; `database` необязателен (Q2) и
/// отсылает к `list_connections`; описания — про кнопку «Send to LLM assistant» (Q4) и про то, что SQL не выполняется.
#[tokio::test]
async fn proposal_tools_describe_their_parameters() {
    let c = client(Arc::new(Fake::new())).await;
    let tools = c.list_all_tools().await.unwrap();
    let tool = |name: &str| tools.iter().find(|t| t.name == name).unwrap();
    let schema = serde_json::to_value(&tool("propose_query_rewrite").input_schema).unwrap();
    let mut required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    required.sort_unstable();
    assert_eq!(required, ["rationale", "rewrittenSql"]);
    let database = schema["properties"]["database"]["description"]
        .as_str()
        .unwrap();
    assert!(database.contains("list_connections"), "{database}");
    let indexes = schema["properties"]["indexes"]["description"]
        .as_str()
        .unwrap();
    assert!(indexes.contains("is ignored"), "{indexes}");
    // Финальное ревью правок смоука P4b, nit #4: поля прямой формы рядом с `requestId` сверяются с запросом.
    let request_id = schema["properties"]["requestId"]["description"]
        .as_str()
        .unwrap();
    assert!(
        request_id
            .contains("are accepted when they match the request (a mismatch is invalid_argument)")
            && !request_id.contains("ignored"),
        "{request_id}"
    );
    // Смоук 2026-11-07, R21: «Обоснование» и «Ожидаемый эффект» приходили по-английски при русском интерфейсе — язык текстов
    // для пользователя назван в описаниях полей и инструмента.
    let rationale = schema["properties"]["rationale"]["description"]
        .as_str()
        .unwrap();
    for phrase in [
        "in the user's language",
        "the language the user writes in",
        "in a background run started by Glafira, the language its prompt names",
    ] {
        assert!(rationale.contains(phrase), "{phrase}: {rationale}");
    }
    let effect = schema["properties"]["expectedEffect"]["description"]
        .as_str()
        .unwrap();
    assert!(
        effect.contains("in the same language as rationale"),
        "{effect}"
    );
    let propose = tool("propose_query_rewrite")
        .description
        .as_deref()
        .unwrap();
    for phrase in [
        "Nothing is executed",
        "invalid_sql",
        "same rows",
        "list_proposals",
        "database defaults to the connection's database",
        "are plain text for the user in the user's language",
    ] {
        assert!(propose.contains(phrase), "{phrase}: {propose}");
    }
    let get = tool("get_optimization_request")
        .description
        .as_deref()
        .unwrap();
    for phrase in [
        "“Send to LLM assistant”",
        "not masked",
        "request_closed",
        "hint",
    ] {
        assert!(get.contains(phrase), "{phrase}: {get}");
    }
    for name in ["get_optimization_request", "list_proposals"] {
        let s = serde_json::to_value(&tool(name).input_schema).unwrap();
        assert!(
            s.get("required")
                .and_then(Value::as_array)
                .is_none_or(|r| r.is_empty()),
            "{name}: {s}"
        );
    }
}

#[tokio::test]
async fn list_proposals_wraps_the_list() {
    let fake = Arc::new(Fake::new().on_llm(|_| {
        Ok(PgAnswer {
            body: PgOutput::Proposals(vec![
                json!({ "proposalId": "p1", "status": "rejected", "reason": "slower" }),
            ]),
            masked: None,
            rows: None,
        })
    }));
    let c = client(fake.clone()).await;
    let v = ok(call(&c, "list_proposals", json!({ "connectionId": "c1" })).await);
    assert_eq!(v["proposals"][0]["reason"], "slower");
    ok(call(
        &c,
        "list_proposals",
        json!({ "connectionId": "", "requestId": " " }),
    )
    .await);
    let ops = fake.llm_ops.lock().unwrap();
    assert!(matches!(
        &ops[0],
        LlmOp::ListProposals { params: p, run: None } if p.connection_id.as_deref() == Some("c1") && p.request_id.is_none()
    ));
    assert!(matches!(
        &ops[1],
        LlmOp::ListProposals { params: p, run: None } if p.connection_id.is_none() && p.request_id.is_none()
    ));
}

#[tokio::test]
async fn proposal_errors_reach_the_model_as_code_and_message() {
    let fake = Arc::new(Fake::new().on_llm(|_| {
        Err(ToolError::new(
            "request_closed",
            "Optimization request r1 was cancelled by the user",
        ))
    }));
    let c = client(fake).await;
    let e = err(&call(&c, "get_optimization_request", json!({ "requestId": "r1" })).await);
    assert_eq!(
        e,
        "request_closed: Optimization request r1 was cancelled by the user"
    );
}

/// Ответ бэкенда не того вида — `internal`, а не паника и не чужой JSON.
#[tokio::test]
async fn unexpected_backend_output_is_internal() {
    let fake = Arc::new(Fake::new().on_llm(|_| {
        Ok(PgAnswer {
            body: PgOutput::Diagnostics(vec![]),
            masked: None,
            rows: None,
        })
    }));
    let c = client(fake).await;
    let e = err(&call(&c, "list_proposals", json!({})).await);
    assert!(e.starts_with("internal: "), "{e}");
}
