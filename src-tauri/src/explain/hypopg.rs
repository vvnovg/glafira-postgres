//! Проверка гипотезы hypopg (спец. P3 §9.2): виртуальный индекс в сессии, план до и после, `hypopg_reset()`.
//! Индекс создаётся в транзакции `READ ONLY` (он живёт в памяти сессии и на диск не пишет), планы — пробами
//! `explain_in_tx` в `READ ONLY`; соединение EXPLAIN закрывается после проверки — на сервере не остаётся ничего.

use std::future::Future;
use std::time::Duration;

use pg_advisor::plan::json::from_json;
use pg_advisor::plan::{PlanNode, PlanTree};
use serde::{Deserialize, Serialize};
use tokio_postgres::Client;

use crate::actions::Confirmation;
use crate::conn::errors::from_pg;
use crate::diag::caps::V16;
use crate::diag::head;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::explain::params::normalize;
use crate::explain::run::{
    Context, Origin, Step, classify, explain_in_tx, explain_timeout, probe_options, within,
};
use crate::explain::{ExplainRequest, Mode, PROBE_TIMEOUT_S, Params, validate};
use crate::state::AppState;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HypoRequest {
    pub member: String,
    pub database: String,
    pub sql: String,
    pub params: Params,
    #[serde(default)]
    pub search_path: Option<String>,
    /// `CREATE INDEX ON …` без `CONCURRENTLY` (`Recommendation.hypo.indexDdl`).
    pub index_ddl: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HypoResult {
    /// Имя виртуального индекса (`<oid>btree_…`).
    pub index_name: String,
    /// Оценка размера индекса (`hypopg_relation_size`).
    pub estimated_bytes: i64,
    pub cost_before: f64,
    pub cost_after: f64,
    /// План после гипотезы использует виртуальный индекс.
    pub used: bool,
    pub plan_after: PlanTree,
}

fn bad_ddl() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.hypo.badDdl",
        "The hypothesis must be a single CREATE INDEX ON … statement without CONCURRENTLY or UNIQUE"
    )
}

/// `CREATE INDEX ON …`: одна инструкция, без `CONCURRENTLY` и `UNIQUE` (hypopg их не принимает / не проверяет).
pub fn validate_ddl(ddl: &str) -> AppResult<()> {
    let s = normalize(ddl);
    let upper = s.to_ascii_uppercase();
    let words: Vec<&str> = upper.split_whitespace().collect();
    if words.len() < 4
        || words[0] != "CREATE"
        || words[1] != "INDEX"
        || words[2] != "ON"
        || s.contains(';')
    {
        return Err(bad_ddl());
    }
    Ok(())
}

/// В дереве есть узел с именем индекса гипотезы (`<oid>btree_…`).
pub fn uses_index(t: &PlanTree, name: &str) -> bool {
    fn walk(n: &PlanNode, name: &str) -> bool {
        n.index_name.as_deref() == Some(name) || n.children.iter().any(|c| walk(c, name))
    }
    walk(&t.root, name)
}

const INSTALLED_SQL: &str = "SELECT 1 FROM pg_extension WHERE extname = 'hypopg'";

/// Сетевой шаг hypopg со сроком пробы: ошибка сервера — через `from_pg`.
async fn step<T>(f: impl Future<Output = Result<T, tokio_postgres::Error>>) -> AppResult<T> {
    match tokio::time::timeout(Duration::from_secs(u64::from(PROBE_TIMEOUT_S) + 5), f).await {
        Ok(r) => r.map_err(|e| from_pg(&e)),
        Err(_) => Err(explain_timeout()),
    }
}

/// План без выполнения в `READ ONLY` и его стоимость; позиция ошибки — в тексте пользователя (`original`).
async fn plan_cost(
    c: &Client,
    original: &str,
    sql: &str,
    params: &Params,
    search_path: Option<&str>,
) -> AppResult<(PlanTree, f64)> {
    let options = probe_options(params);
    let ctx = Context {
        origin: Some(Origin::new(&options, original)),
        write: false,
        read_only: true,
    };
    let raw = match within(
        PROBE_TIMEOUT_S,
        explain_in_tx(c, ctx, PROBE_TIMEOUT_S, search_path, &options, sql, params),
    )
    .await
    {
        Ok(v) => v,
        Err(Step::Pg(e)) => return Err(classify(&e, ctx)),
        Err(Step::App(e)) => return Err(e),
    };
    let tree = from_json(&raw)
        .map_err(|e| AppError::internal_with(format!("plan JSON: {}", e.message)))?;
    let cost = tree.root.total_cost;
    Ok((tree, cost))
}

/// Проверки запроса — те же, что у пробного плана EXPLAIN (текст, длина, число параметров, `search_path`).
fn validate_request(req: &HypoRequest) -> AppResult<String> {
    validate_ddl(&req.index_ddl)?;
    validate(&ExplainRequest {
        member: req.member.clone(),
        database: req.database.clone(),
        sql: req.sql.clone(),
        mode: Mode::Plan,
        params: req.params.clone(),
        timeout_s: PROBE_TIMEOUT_S,
        search_path: req.search_path.clone(),
        confirmation: Confirmation::default(),
        force_write: false,
        write_confirmed: false,
    })
}

pub async fn check(st: &AppState, id: &str, req: &HypoRequest) -> AppResult<HypoResult> {
    let sql = validate_request(req)?;
    let client =
        crate::actions::conn::open_explain(st, id, &req.member, Some(&req.database)).await?;
    let c = &client.client;
    if step(c.query_opt(INSTALLED_SQL, &[])).await?.is_none() {
        return Err(app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.hypo.notInstalled",
            "hypopg is not installed in this database"
        ));
    }
    if req.params == Params::Generic && step_head(c).await?.version_num < V16 {
        return Err(app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.explain.genericUnsupported",
            "GENERIC_PLAN needs PostgreSQL 16 or newer: enter parameter values"
        ));
    }
    let sp = req.search_path.as_deref();
    let (_, cost_before) = plan_cost(c, &req.sql, &sql, &req.params, sp).await?;
    // Гипотеза живёт в памяти сессии и не откатывается транзакцией: создаётся в READ ONLY (проверка, что сервер
    // ничего не пишет), видна пробе «после» на том же соединении и снимается `hypopg_reset()` при любом исходе.
    let outcome = match create(c, &normalize(&req.index_ddl)).await {
        Ok(created) => plan_cost(c, &req.sql, &sql, &req.params, sp)
            .await
            .map(|after| (created, after)),
        Err(e) => Err(e),
    };
    let _ = step(c.batch_execute("SELECT hypopg_reset()")).await;
    let ((index_name, estimated_bytes), (plan_after, cost_after)) = outcome?;
    Ok(HypoResult {
        used: uses_index(&plan_after, &index_name),
        index_name,
        estimated_bytes,
        cost_before,
        cost_after,
        plan_after,
    })
}

async fn step_head(c: &Client) -> AppResult<crate::diag::Head> {
    match tokio::time::timeout(Duration::from_secs(u64::from(PROBE_TIMEOUT_S) + 5), head(c)).await {
        Ok(h) => h,
        Err(_) => Err(explain_timeout()),
    }
}

/// Виртуальный индекс и оценка его размера — в транзакции `READ ONLY`. Ошибка разбора DDL сервером (hypopg разбирает
/// его сам: неизвестная таблица или колонка) — `INVALID_INPUT` с сообщением сервера.
async fn create(c: &Client, ddl: &str) -> AppResult<(String, i64)> {
    step(c.batch_execute("BEGIN READ ONLY")).await?;
    let made = async {
        let row = c
            .query_one(
                "SELECT indexrelid, indexname FROM hypopg_create_index($1)",
                &[&ddl],
            )
            .await?;
        let (indexrelid, name): (u32, String) = (row.get(0), row.get(1));
        let size: i64 = c
            .query_one("SELECT hypopg_relation_size($1)", &[&indexrelid])
            .await?
            .get(0);
        Ok::<_, tokio_postgres::Error>((name, size))
    };
    let made = match tokio::time::timeout(Duration::from_secs(u64::from(PROBE_TIMEOUT_S) + 5), made)
        .await
    {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(ddl_error(&e)),
        Err(_) => Err(explain_timeout()),
    };
    let _ = step(c.batch_execute("ROLLBACK")).await;
    made
}

/// Ошибка `hypopg_create_index`: разбор и проверки самого сервера (классы 42 и 22) и проверки hypopg (XX000 с
/// сообщением «hypopg: …», например неизвестная колонка) — отказ гипотезе; прочее — через `from_pg`.
fn ddl_error(e: &tokio_postgres::Error) -> AppError {
    match e.as_db_error() {
        Some(db) if rejects_ddl(db.code().code(), db.message()) => {
            app_err!(
                ErrorCode::InvalidInput,
                "errors.hypo.rejected",
                "The server rejected the hypothetical index: {{message}}",
                message = db.message()
            )
        }
        _ => from_pg(e),
    }
}

fn rejects_ddl(code: &str, message: &str) -> bool {
    code.starts_with("42")
        || code.starts_with("22")
        || (code == "XX000" && message.starts_with("hypopg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_must_be_a_single_create_index_without_concurrently() {
        assert!(validate_ddl("CREATE INDEX ON \"shop\".\"orders\" (\"status\")").is_ok());
        assert!(validate_ddl("  create index on t (a, b)  ").is_ok());
        assert_eq!(
            validate_ddl("CREATE INDEX CONCURRENTLY ON t (a)")
                .unwrap_err()
                .key,
            "errors.hypo.badDdl"
        );
        assert_eq!(
            validate_ddl("CREATE INDEX ON t (a); DROP TABLE t")
                .unwrap_err()
                .key,
            "errors.hypo.badDdl"
        );
        assert_eq!(
            validate_ddl("CREATE UNIQUE INDEX ON t (a)")
                .unwrap_err()
                .key,
            "errors.hypo.badDdl",
            "уникальность hypopg не проверяет"
        );
        assert_eq!(
            validate_ddl("SELECT 1").unwrap_err().key,
            "errors.hypo.badDdl"
        );
    }

    #[test]
    fn uses_index_finds_the_hypothetical_name_in_the_plan() {
        let t = pg_advisor::plan::json::from_json(&serde_json::json!({"Plan": {"Node Type": "Index Scan", "Index Name": "<13543>btree_orders_status", "Relation Name": "orders", "Startup Cost": 0.0, "Total Cost": 8.0, "Plan Rows": 1, "Plan Width": 4}})).unwrap();
        assert!(uses_index(&t, "<13543>btree_orders_status"));
        assert!(!uses_index(&t, "orders_pkey"));
    }

    #[test]
    fn hypopg_and_parse_errors_reject_the_hypothesis() {
        assert!(rejects_ddl("42P01", "relation \"t\" does not exist"));
        assert!(rejects_ddl("XX000", "hypopg: column \"x\" does not exist"));
        assert!(!rejects_ddl("XX000", "could not read block"));
        assert!(!rejects_ddl(
            "57014",
            "canceling statement due to statement timeout"
        ));
    }

    #[test]
    fn request_checks_match_the_probe_plan() {
        let req = HypoRequest {
            member: "pg-1".into(),
            database: "perf".into(),
            sql: "SELECT * FROM t WHERE a = $1;".into(),
            params: Params::Values {
                values: vec![Some("1".into())],
            },
            search_path: None,
            index_ddl: "CREATE INDEX ON t (a)".into(),
        };
        assert_eq!(
            validate_request(&req).unwrap(),
            "SELECT * FROM t WHERE a = $1"
        );
        let none = HypoRequest {
            params: Params::None,
            ..req.clone()
        };
        assert_eq!(
            validate_request(&none).unwrap_err().key,
            "errors.explain.paramsCount"
        );
        let sp = HypoRequest {
            search_path: Some("public; drop".into()),
            ..req.clone()
        };
        assert_eq!(
            validate_request(&sp).unwrap_err().key,
            "errors.explain.badSearchPath"
        );
        let r: HypoRequest = serde_json::from_value(serde_json::json!({
            "member": "m", "database": "d", "sql": "SELECT 1", "params": {"kind": "none"},
            "indexDdl": "CREATE INDEX ON t (a)"
        }))
        .unwrap();
        assert_eq!(
            (r.search_path, r.index_ddl.as_str()),
            (None, "CREATE INDEX ON t (a)")
        );
    }
}
