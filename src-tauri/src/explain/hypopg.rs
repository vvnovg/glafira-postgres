//! Проверка гипотезы hypopg (спец. P3 §9.2): план до, виртуальный индекс, план после, `hypopg_reset()`.
//! Гипотеза — одна транзакция `READ ONLY`: `SET LOCAL` (срок, тот же `search_path`, что у пробы), `hypopg_create_index`,
//! EXPLAIN «после», `hypopg_reset()`, `ROLLBACK`. Сброс пробуется и при ошибке (в прерванной транзакции — ещё раз после
//! отката: виртуальный индекс живёт в памяти сессии и откатом не снимается); последняя гарантия — `check` закрывает
//! соединение EXPLAIN. Функции hypopg вызываются со схемой расширения (`extnamespace`), а не через `search_path`.

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
    Context, Origin, ROLLBACK_SQL, Step, begin_tx, classify, explain_in_open_tx, explain_in_tx,
    probe_options, within,
};
use crate::explain::{ExplainRequest, Mode, PROBE_TIMEOUT_S, Params, validate};
use crate::product::SQL_MARKER;
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

/// Гипотеза в одной транзакции: имя и размер виртуального индекса, план после.
#[derive(Debug, Clone, PartialEq)]
pub struct Hypothesis {
    pub index_name: String,
    pub estimated_bytes: i64,
    pub plan_after: PlanTree,
}

fn bad_ddl() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.hypo.badDdl",
        "The hypothesis must be a single CREATE INDEX ON … statement without CONCURRENTLY or UNIQUE"
    )
}

/// Признак второй инструкции или способа её спрятать. Вне кавычек `'…'` / `"…"` (удвоенная кавычка — закрыть и открыть)
/// запрещены `;`, начала комментариев `--` и `/*` и `$` (долларовые кавычки, `$N`): в комментарии или `$$…$$` кавычка
/// сбила бы разбор, и `;` за ней оказалась бы «внутри». Обратная косая черта запрещена везде: `E'\''` (и `'\''` при
/// `standard_conforming_strings = off`) экранирует кавычку, которую этот разбор считает закрывающей. Предложенные
/// команды (`plan_rules::index_ddl`) берут идентификаторы в кавычки и ничего из этого не содержат.
fn has_hidden_statement(s: &str) -> bool {
    if s.contains('\\') {
        return true;
    }
    let mut quote: Option<char> = None;
    let mut prev: Option<char> = None;
    for ch in s.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => {}
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch == ';' || ch == '$' => return true,
            None if (prev == Some('-') && ch == '-') || (prev == Some('/') && ch == '*') => {
                return true;
            }
            None => {}
        }
        prev = if quote.is_none() { Some(ch) } else { None };
    }
    false
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
        || has_hidden_statement(&s)
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

const SCHEMA_SQL: &str = marked_sql!(
    "SELECT n.nspname FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace \
    WHERE e.extname = 'hypopg'"
);

/// Служебные запросы гипотезы к функциям hypopg в схеме расширения, с меткой `/* glafira */` (в топе «Запросов» они —
/// собственные). EXPLAIN самого запроса пользователя метки не получает, как в P3a.
struct ServiceSql {
    create: String,
    size: String,
    reset: String,
}

impl ServiceSql {
    fn new(schema: &str) -> Self {
        let s = quote_ident(schema);
        Self {
            create: format!(
                "{SQL_MARKER}SELECT indexrelid, indexname FROM {s}.hypopg_create_index($1)"
            ),
            size: format!("{SQL_MARKER}SELECT {s}.hypopg_relation_size($1)"),
            reset: format!("{SQL_MARKER}SELECT {s}.hypopg_reset()"),
        }
    }
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn not_installed() -> AppError {
    app_err!(
        ErrorCode::FeatureUnavailable,
        "errors.hypo.notInstalled",
        "hypopg is not installed in this database"
    )
}

/// Ошибка шага: сырая ошибка сервера переводится в контексте пробы.
fn app(s: Step, ctx: Context) -> AppError {
    match s {
        Step::Pg(e) => classify(&e, ctx),
        Step::App(e) => e,
    }
}

/// Истёк клиентский срок `within`: соединение, возможно, висит — ни отката, ни сброса не ждём, его закроет `check`.
fn client_deadline(s: &Step) -> bool {
    matches!(s, Step::App(e) if e.key == "errors.explain.timeout")
}

/// Схема расширения hypopg в базе соединения; `None` — не установлено.
pub async fn schema_of(c: &Client) -> AppResult<Option<String>> {
    within(PROBE_TIMEOUT_S, async {
        Ok(c.query_opt(SCHEMA_SQL, &[]).await?.map(|r| r.get(0)))
    })
    .await
    .map_err(|e| {
        app(
            e,
            Context {
                origin: None,
                write: false,
                read_only: true,
            },
        )
    })
}

fn probe_ctx(original: &str, params: &Params) -> Context {
    Context {
        origin: Some(Origin::new(&probe_options(params), original)),
        write: false,
        read_only: true,
    }
}

/// План без выполнения в `READ ONLY` и его стоимость.
async fn plan_cost(c: &Client, req: &HypoRequest, sql: &str) -> AppResult<f64> {
    let options = probe_options(&req.params);
    let ctx = probe_ctx(&req.sql, &req.params);
    let sp = req.search_path.as_deref();
    let raw = within(
        PROBE_TIMEOUT_S,
        explain_in_tx(c, ctx, PROBE_TIMEOUT_S, sp, &options, sql, &req.params),
    )
    .await
    .map_err(|e| app(e, ctx))?;
    Ok(parse(&raw)?.root.total_cost)
}

fn parse(raw: &serde_json::Value) -> AppResult<PlanTree> {
    from_json(raw).map_err(|e| AppError::internal_with(format!("plan JSON: {}", e.message)))
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
    // Проверка до соединения: неверный запрос не открывает сеанс (`check_on` повторит её — она дешёвая).
    validate_request(req)?;
    let client =
        crate::actions::conn::open_explain(st, id, &req.member, Some(&req.database)).await?;
    // Соединение закрывается при выходе (`Drop` у `ActionClient`) — последняя гарантия, что гипотез не осталось.
    check_on(&client.client, req).await
}

/// Проверка на готовом соединении EXPLAIN: после неё в сессии нет виртуальных индексов (при любом исходе, кроме
/// истёкшего клиентского срока — тогда соединение закрывает вызывающий).
pub async fn check_on(c: &Client, req: &HypoRequest) -> AppResult<HypoResult> {
    let sql = validate_request(req)?;
    let schema = schema_of(c).await?.ok_or_else(not_installed)?;
    if req.params == Params::Generic {
        let h = within(PROBE_TIMEOUT_S, async { head(c).await.map_err(Step::App) })
            .await
            .map_err(|e| app(e, probe_ctx(&req.sql, &req.params)))?;
        if h.version_num < V16 {
            return Err(app_err!(
                ErrorCode::FeatureUnavailable,
                "errors.explain.genericUnsupported",
                "GENERIC_PLAN needs PostgreSQL 16 or newer: enter parameter values"
            ));
        }
    }
    let cost_before = plan_cost(c, req, &sql).await?;
    let h = hypothesize(c, &schema, req, &sql).await?;
    Ok(HypoResult {
        used: uses_index(&h.plan_after, &h.index_name),
        cost_after: h.plan_after.root.total_cost,
        index_name: h.index_name,
        estimated_bytes: h.estimated_bytes,
        cost_before,
        plan_after: h.plan_after,
    })
}

/// Гипотеза в одной транзакции `READ ONLY` (`schema` — схема hypopg, `sql` — нормализованный текст запроса):
/// `begin_tx` (срок, `search_path` запроса), индекс, размер, EXPLAIN «после», `hypopg_reset()`, `ROLLBACK`.
pub async fn hypothesize(
    c: &Client,
    schema: &str,
    req: &HypoRequest,
    sql: &str,
) -> AppResult<Hypothesis> {
    let options = probe_options(&req.params);
    let ctx = probe_ctx(&req.sql, &req.params);
    let ServiceSql {
        create: create_sql,
        size: size_sql,
        reset: reset_sql,
    } = ServiceSql::new(schema);
    let ddl = normalize(&req.index_ddl);
    let work = within(PROBE_TIMEOUT_S, async {
        begin_tx(c, ctx, PROBE_TIMEOUT_S, req.search_path.as_deref()).await?;
        let row = c
            .query_one(&create_sql, &[&ddl])
            .await
            .map_err(|e| Step::App(ddl_error(&e)))?;
        let (indexrelid, index_name): (u32, String) = (row.get(0), row.get(1));
        let estimated_bytes: i64 = c.query_one(&size_sql, &[&indexrelid]).await?.get(0);
        let raw = explain_in_open_tx(c, &options, sql, &req.params).await?;
        Ok((index_name, estimated_bytes, raw))
    })
    .await;
    let work = match work {
        Err(e) if client_deadline(&e) => return Err(app(e, ctx)),
        w => w,
    };
    let cleanup = cleanup(c, &reset_sql).await;
    let (index_name, estimated_bytes, raw) = work.map_err(|e| app(e, ctx))?;
    cleanup?;
    Ok(Hypothesis {
        index_name,
        estimated_bytes,
        plan_after: parse(&raw)?,
    })
}

/// `hypopg_reset()` до `ROLLBACK`; если транзакция прервана ошибкой и сброс не прошёл — ещё раз после отката.
async fn cleanup(c: &Client, reset_sql: &str) -> AppResult<()> {
    let ctx = Context {
        origin: None,
        write: false,
        read_only: true,
    };
    let run = |q: &str| {
        let q = q.to_string();
        async move { within(PROBE_TIMEOUT_S, async { Ok(c.batch_execute(&q).await?) }).await }
    };
    let reset = run(reset_sql).await;
    let reset = match reset {
        Err(e) if client_deadline(&e) => return Err(app(e, ctx)),
        r => r,
    };
    run(ROLLBACK_SQL).await.map_err(|e| app(e, ctx))?;
    if reset.is_err() {
        run(reset_sql).await.map_err(|e| app(e, ctx))?;
    }
    Ok(())
}

/// Ошибка `hypopg_create_index` из-за самого DDL — отказ гипотезе: синтаксис (42601), нет таблицы, колонки, объекта
/// или функции (42P01, 42703, 42704, 42883), не тот тип объекта (42809, индекс на представлении), неверное
/// определение (42P17, например изменчивая функция в выражении), а также проверки hypopg (XX000 с «hypopg: …»).
/// Права (42501) и прочее — через `from_pg`.
fn ddl_error(e: &tokio_postgres::Error) -> AppError {
    match e.as_db_error() {
        Some(db) if rejects_ddl(db.code().code(), db.message()) => app_err!(
            ErrorCode::InvalidInput,
            "errors.hypo.rejected",
            "The server rejected the hypothetical index: {{message}}",
            message = db.message()
        ),
        _ => from_pg(e),
    }
}

const DDL_REJECTED: [&str; 7] = [
    "42601", "42P01", "42703", "42704", "42883", "42809", "42P17",
];

fn rejects_ddl(code: &str, message: &str) -> bool {
    DDL_REJECTED.contains(&code) || (code == "XX000" && message.starts_with("hypopg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Служебные запросы гипотезы помечены `/* glafira */`: в топе «Запросов» они — собственные, а не чужие.
    #[test]
    fn service_sql_is_marked() {
        let q = ServiceSql::new("hy\"po");
        for sql in [SCHEMA_SQL, &q.create, &q.size, &q.reset] {
            assert!(crate::product::is_marked_sql(sql), "{sql}");
        }
        assert_eq!(q.reset, "/* glafira */ SELECT \"hy\"\"po\".hypopg_reset()");
        assert!(
            q.create
                .ends_with("FROM \"hy\"\"po\".hypopg_create_index($1)")
        );
        assert!(
            q.size
                .ends_with("SELECT \"hy\"\"po\".hypopg_relation_size($1)")
        );
    }

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
    fn semicolon_inside_quotes_is_allowed() {
        assert!(validate_ddl("CREATE INDEX ON \"a;b\" (c) WHERE d = 'x;y'").is_ok());
        assert!(validate_ddl("CREATE INDEX ON t (a) WHERE b = 'it''s;'").is_ok());
        assert!(
            validate_ddl("CREATE INDEX ON \"q\"\"\" (a);").is_ok(),
            "хвостовая `;` снимается"
        );
        assert!(validate_ddl("CREATE INDEX ON t (a) WHERE b = 'x'; DROP TABLE t").is_err());
        assert!(validate_ddl("CREATE INDEX ON \"t\"; DROP TABLE t").is_err());
        for hidden in [
            "CREATE INDEX ON t (a) -- '\n; CREATE INDEX ON u (b)",
            "CREATE INDEX ON t (a) WHERE b = $$'$$; CREATE INDEX ON u (b)",
            "CREATE INDEX ON t (a) WHERE b = E'\\'' ; DROP TABLE t; --'",
            "CREATE INDEX ON t (a) WHERE b = '\\'' ; DROP TABLE t; --'",
            "CREATE INDEX ON t (a) /* ' */ ; DROP TABLE t",
            "CREATE INDEX ON t (a) WHERE b = $1",
        ] {
            assert!(validate_ddl(hidden).is_err(), "{hidden}");
        }
        assert!(
            validate_ddl("CREATE INDEX ON \"a--b\" (\"c/*d\") WHERE e = '$x--'").is_ok(),
            "внутри кавычек -- /* $ допустимы"
        );
        assert!(
            validate_ddl("CREATE INDEX ON t (a) /* ; */").is_err(),
            "в комментарии — осторожно, отказ"
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
        assert!(rejects_ddl("42703", "column \"x\" does not exist"));
        assert!(
            !rejects_ddl("42501", "permission denied for table t"),
            "права — не ошибка гипотезы"
        );
        assert!(!rejects_ddl("22023", "invalid parameter"));
        assert!(rejects_ddl("XX000", "hypopg: column \"x\" does not exist"));
        assert!(!rejects_ddl("XX000", "could not read block"));
        assert!(!rejects_ddl(
            "57014",
            "canceling statement due to statement timeout"
        ));
    }

    #[test]
    fn schema_is_quoted() {
        assert_eq!(quote_ident("public"), "\"public\"");
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
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
