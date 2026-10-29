//! EXPLAIN и EXPLAIN ANALYZE (спец. P3 §8): запрос, результат, проверки до сервера. Ход — `run`, параметры —
//! `params`, ярус — `tier`, данные для LLM — `llm`.

pub mod params;
pub mod run;
pub mod tier;

use pg_advisor::plan::{PlanTree, RelationRef};
use serde::{Deserialize, Serialize};

use crate::actions::Confirmation;
use crate::error::{AppResult, ErrorCode};

pub const PROBE_TIMEOUT_S: u32 = 15;
pub const DEFAULT_TIMEOUT_S: u32 = 30;
pub const TIMEOUT_RANGE: std::ops::RangeInclusive<u32> = 5..=600;
pub const MAX_SQL_BYTES: usize = 262_144;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    Plan,
    Analyze,
}

/// Параметры `$N`: нет, `GENERIC_PLAN` (16+, только без ANALYZE) или значения (`None` — NULL).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Params {
    None,
    Generic,
    Values { values: Vec<Option<String>> },
}

fn default_timeout() -> u32 {
    DEFAULT_TIMEOUT_S
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainRequest {
    pub member: String,
    pub database: String,
    pub sql: String,
    pub mode: Mode,
    pub params: Params,
    /// Только ANALYZE, секунды; пробный план всегда в `PROBE_TIMEOUT_S`.
    #[serde(default = "default_timeout")]
    pub timeout_s: u32,
    /// `None` — серверный по умолчанию для роли.
    #[serde(default)]
    pub search_path: Option<String>,
    #[serde(default)]
    pub confirmation: Confirmation,
    /// Выполнить как запись, хотя пробный план — чтение (кнопка после `EXPLAIN_WRITES`).
    #[serde(default)]
    pub force_write: bool,
}

pub use tier::Tier;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainResult {
    pub mode: Mode,
    pub tier: Tier,
    pub plan: PlanTree,
    /// JSON сервера как есть — кнопка «JSON».
    pub raw: serde_json::Value,
    pub relations: Vec<RelationRef>,
    pub elapsed_ms: u64,
    /// Ключи предупреждений интерфейса: `genericPlan`, `probeWithoutReadOnly`.
    pub warnings: Vec<String>,
}

/// Проверки до сервера; возвращает нормализованный SQL (без хвостовых `;`).
pub fn validate(req: &ExplainRequest) -> AppResult<String> {
    let sql = params::normalize(&req.sql);
    if sql.is_empty() {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.emptySql",
            "Enter a query"
        ));
    }
    if sql.len() > MAX_SQL_BYTES {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.sqlTooLong",
            "The query is longer than {{max}} bytes",
            max = MAX_SQL_BYTES
        ));
    }
    let expected = params::count_params(&sql);
    let got = match &req.params {
        Params::Values { values } => values.len(),
        Params::Generic => expected,
        Params::None => 0,
    };
    if got != expected {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.paramsCount",
            "The query has {{expected}} parameters, {{got}} values given",
            expected = expected,
            got = got
        ));
    }
    if req.params == Params::Generic && req.mode == Mode::Analyze {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.genericAnalyze",
            "GENERIC_PLAN cannot be combined with ANALYZE: enter parameter values"
        ));
    }
    if req.mode == Mode::Analyze && !TIMEOUT_RANGE.contains(&req.timeout_s) {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.badTimeout",
            "The timeout must be between 5 and 600 seconds"
        ));
    }
    if let Some(sp) = &req.search_path
        && !params::valid_search_path(sp)
    {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.badSearchPath",
            "search_path may contain only identifiers, quotes, commas and spaces"
        ));
    }
    Ok(sql)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::Confirmation;
    use crate::error::ErrorCode;

    fn req(sql: &str, mode: Mode, params: Params) -> ExplainRequest {
        ExplainRequest {
            member: "pg-1".into(),
            database: "perf".into(),
            sql: sql.into(),
            mode,
            params,
            timeout_s: DEFAULT_TIMEOUT_S,
            search_path: None,
            confirmation: Confirmation {
                confirm: None,
                typed_name: None,
            },
            force_write: false,
        }
    }

    #[test]
    fn request_json_shape() {
        let r: ExplainRequest = serde_json::from_value(serde_json::json!({
            "member": "pg-1", "database": "perf", "sql": "SELECT $1", "mode": "analyze",
            "params": {"kind": "values", "values": ["1", null]}, "timeoutS": 60, "searchPath": null,
            "confirmation": {"confirm": null, "typedName": null}
        }))
        .unwrap();
        assert_eq!((r.mode, r.timeout_s), (Mode::Analyze, 60));
        assert_eq!(
            r.params,
            Params::Values {
                values: vec![Some("1".into()), None]
            }
        );
        let r: ExplainRequest = serde_json::from_value(serde_json::json!({"member": "m", "database": "d", "sql": "SELECT 1", "mode": "plan", "params": {"kind": "generic"}})).unwrap();
        assert_eq!(
            (r.params, r.timeout_s, r.search_path, r.force_write),
            (Params::Generic, DEFAULT_TIMEOUT_S, None, false),
            "срок, search_path и forceWrite по умолчанию"
        );
    }

    #[test]
    fn validation() {
        assert_eq!(
            validate(&req("  SELECT 1; ", Mode::Plan, Params::None)).unwrap(),
            "SELECT 1"
        );
        assert_eq!(
            validate(&req("   ", Mode::Plan, Params::None))
                .unwrap_err()
                .key,
            "errors.explain.emptySql"
        );
        let long = "SELECT ".to_string() + &"1,".repeat(MAX_SQL_BYTES / 2);
        assert_eq!(
            validate(&req(&long, Mode::Plan, Params::None))
                .unwrap_err()
                .key,
            "errors.explain.sqlTooLong"
        );
        let e = validate(&req(
            "SELECT $1, $2",
            Mode::Plan,
            Params::Values {
                values: vec![Some("1".into())],
            },
        ))
        .unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::InvalidInput, "errors.explain.paramsCount")
        );
        assert_eq!(
            (e.params["expected"].as_str(), e.params["got"].as_str()),
            ("2", "1")
        );
        assert_eq!(
            validate(&req("SELECT $1", Mode::Plan, Params::None))
                .unwrap_err()
                .key,
            "errors.explain.paramsCount"
        );
        assert!(validate(&req("SELECT $1", Mode::Plan, Params::Generic)).is_ok());
        assert_eq!(
            validate(&req("SELECT $1", Mode::Analyze, Params::Generic))
                .unwrap_err()
                .key,
            "errors.explain.genericAnalyze"
        );
        let mut r = req("SELECT 1", Mode::Analyze, Params::None);
        r.timeout_s = 4;
        assert_eq!(validate(&r).unwrap_err().key, "errors.explain.badTimeout");
        r.timeout_s = 600;
        r.search_path = Some("public; drop".into());
        assert_eq!(
            validate(&r).unwrap_err().key,
            "errors.explain.badSearchPath"
        );
        r.search_path = Some("public".into());
        assert!(validate(&r).is_ok());
    }

    #[test]
    fn explain_writes_code_serializes() {
        assert_eq!(
            serde_json::to_value(ErrorCode::ExplainWrites).unwrap(),
            "EXPLAIN_WRITES"
        );
        assert!(
            !crate::error::AppError::localized(
                ErrorCode::ExplainWrites,
                ltext!("errors.explain.writes", "x")
            )
            .retryable
        );
    }
}
