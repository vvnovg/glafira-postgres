//! Инструменты P4b через клиента rmcp: `run_select` (Task 2), предложения (Task 4), сторожок списка.

mod p4b;

use std::sync::Arc;
use std::time::Duration;

use glafira_mcp_core::error::ToolError;
use glafira_pg_mcp::model::{PgAnswer, PgOutput, SelectColumn, SelectOut};
use glafira_pg_mcp::server::TOOL_NAMES;
use p4b::{Fake, call, client, err, ok};
use serde_json::json;

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
    assert_eq!(TOOL_NAMES.len(), 15);
    assert!(TOOL_NAMES.contains(&"run_select"));
    let cyrillic = |s: &str| s.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c));
    for t in &tools {
        let a = t
            .annotations
            .as_ref()
            .unwrap_or_else(|| panic!("{}: нет аннотаций", t.name));
        // Все четыре подсказки явно у каждого инструмента (§5.1): Qwen Code повторяет после обрыва только
        // идемпотентные вызовы, каталоги клиентов отклоняют инструменты без подсказок.
        assert_eq!(
            (
                a.read_only_hint,
                a.destructive_hint,
                a.idempotent_hint,
                a.open_world_hint
            ),
            (Some(true), Some(false), Some(true), Some(false)),
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
