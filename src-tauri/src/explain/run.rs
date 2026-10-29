//! Ход EXPLAIN (спец. P3 §8.3): соединение `for_explain`, пробный план в `READ ONLY`, ярус, выполнение, журнал.

use std::time::Instant;

use pg_advisor::plan::json::from_json;
use pg_advisor::plan::{PlanTree, relations};
use tokio_postgres::error::{ErrorPosition, SqlState};
use tokio_postgres::types::private::BytesMut;
use tokio_postgres::types::{Format, IsNull, ToSql, Type, to_sql_checked};
use tokio_postgres::{Client, SimpleQueryMessage};

use crate::actions::journal::{self, ActionRecord, ExplainRecord, RecordConnection};
use crate::conn::errors::{chain, from_pg};
use crate::conn::profile::Environment;
use crate::diag::caps::V16;
use crate::diag::head;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::explain::tier::{Tier, tier_of};
use crate::explain::{ExplainRequest, ExplainResult, Mode, PROBE_TIMEOUT_S, Params, validate};
use crate::guard::ensure_mutation_allowed;
use crate::state::AppState;

const PROBE_OPTIONS: &str = "FORMAT JSON, VERBOSE, SETTINGS";
const ANALYZE_OPTIONS: &str = "ANALYZE, BUFFERS, WAL, VERBOSE, SETTINGS, FORMAT JSON";

/// Ошибка одного EXPLAIN: сырая ошибка сервера нужна до перевода в `AppError` (25006, «вторая инструкция»).
enum Step {
    Pg(tokio_postgres::Error),
    App(AppError),
}

impl From<tokio_postgres::Error> for Step {
    fn from(e: tokio_postgres::Error) -> Self {
        Step::Pg(e)
    }
}

/// Значение параметра текстом: тип в Parse — `unknown`, его выводит сервер (как при выполнении приложением), и он
/// же разбирает текст значения. `None` — NULL. Текстовый формат нужен потому, что двоичный пришлось бы кодировать под
/// выведенный тип (`int8`, `timestamptz`, …). `BytesMut` — из `types::private`: тот же путь использует макрос
/// `to_sql_checked!` (крейта `bytes` в зависимостях нет).
#[derive(Debug)]
struct TextParam<'a>(Option<&'a str>);

impl ToSql for TextParam<'_> {
    fn to_sql(
        &self,
        _ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        match self.0 {
            Some(v) => {
                out.extend_from_slice(v.as_bytes());
                Ok(IsNull::No)
            }
            None => Ok(IsNull::Yes),
        }
    }

    fn accepts(_ty: &Type) -> bool {
        true
    }

    fn encode_format(&self, _ty: &Type) -> Format {
        Format::Text
    }

    to_sql_checked!();
}

fn is_read_only_violation(e: &tokio_postgres::Error) -> bool {
    e.code() == Some(&SqlState::READ_ONLY_SQL_TRANSACTION)
}

/// Начало текста команды EXPLAIN перед SQL пользователя.
fn explain_prefix(options: &str) -> String {
    format!("EXPLAIN ({options}) ")
}

/// Где SQL пользователя в команде EXPLAIN: позиция ошибки сервера пересчитывается в позицию в исходном тексте.
#[derive(Debug, Clone, Copy)]
struct Origin {
    /// Символов `EXPLAIN (…) ` перед SQL.
    prefix: usize,
    /// Ведущих пробельных символов исходного текста, снятых `normalize`.
    leading: usize,
}

impl Origin {
    fn new(options: &str, original: &str) -> Self {
        Self {
            prefix: explain_prefix(options).chars().count(),
            leading: original.chars().take_while(|c| c.is_whitespace()).count(),
        }
    }

    /// Позиция сервера (символы с 1) → позиция в исходном тексте; ошибка в самом префиксе — `None`.
    fn position(self, server: u32) -> Option<usize> {
        (server as usize)
            .checked_sub(self.prefix)
            .filter(|p| *p >= 1)
            .map(|p| p + self.leading)
    }
}

/// Перевод ошибки сервера: вторая инструкция, синтаксис с позицией, 57014, 42501 и прочие — через `from_pg`.
/// `origin` — для ошибки самой команды EXPLAIN; позиция во внутреннем запросе (тело функции) не показывается.
fn classify(e: &tokio_postgres::Error, origin: Option<Origin>) -> AppError {
    let db = e.as_db_error();
    let message = db
        .map(|d| d.message().to_string())
        .unwrap_or_else(|| chain(e));
    if e.code() == Some(&SqlState::SYNTAX_ERROR) {
        if message.contains("multiple commands") {
            return app_err!(
                ErrorCode::InvalidInput,
                "errors.explain.oneStatement",
                "Enter exactly one statement"
            );
        }
        let position = match (db.and_then(|d| d.position()), origin) {
            (Some(ErrorPosition::Original(n)), Some(o)) => o.position(*n),
            _ => None,
        };
        let err = app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.server",
            "The server rejected the query: {{message}}",
            message = message
        );
        return match position {
            Some(p) => err.with_details(format!("position {p}")),
            None => err,
        };
    }
    if e.code() == Some(&SqlState::QUERY_CANCELED) {
        return app_err!(
            ErrorCode::Timeout,
            "errors.explain.timeout",
            "The plan was not produced in time; execution was cancelled, nothing changed"
        );
    }
    if e.code() == Some(&SqlState::INSUFFICIENT_PRIVILEGE) {
        return app_err!(
            ErrorCode::NotAuthorized,
            "errors.explain.notAuthorized",
            "Not enough privileges on the query's tables: grant pg_read_all_data (14+) or SELECT"
        )
        .with_details(message);
    }
    if e.code() == Some(&SqlState::READ_ONLY_SQL_TRANSACTION) {
        return explain_writes();
    }
    from_pg(e)
}

fn explain_writes() -> AppError {
    app_err!(
        ErrorCode::ExplainWrites,
        "errors.explain.writes",
        "The query writes data (a volatile function or trigger): run it as a write with confirmation"
    )
}

/// Один EXPLAIN внутри транзакции: `BEGIN [READ ONLY]`, `SET LOCAL`, EXPLAIN, `ROLLBACK`.
async fn explain_in_tx(
    c: &Client,
    read_only: bool,
    timeout_s: u32,
    search_path: Option<&str>,
    options: &str,
    sql: &str,
    params: &Params,
) -> Result<serde_json::Value, Step> {
    let mut begin = format!(
        "BEGIN{}; SET LOCAL statement_timeout = '{timeout_s}s'; SET LOCAL lock_timeout = '1s'",
        if read_only { " READ ONLY" } else { "" }
    );
    if let Some(sp) = search_path {
        begin.push_str(&format!("; SET LOCAL search_path = {sp}"));
    }
    let result = match c.batch_execute(&begin).await {
        // Ошибка начала (например, `search_path`) — без позиции: она не в тексте пользователя.
        Err(e) => Err(Step::App(classify(&e, None))),
        Ok(()) => explain_query(c, &format!("{}{sql}", explain_prefix(options)), params).await,
    };
    // Откат — страховка: при ошибке транзакция уже прервана сервером, при обрыве соединение закроется.
    let _ = c.batch_execute("ROLLBACK").await;
    result
}

/// Сама команда EXPLAIN — одна инструкция расширенным протоколом (вторую сервер отвергнет при разборе).
async fn explain_query(c: &Client, text: &str, params: &Params) -> Result<serde_json::Value, Step> {
    let rows = match params {
        Params::Values { values } => {
            let stmt = c
                .prepare_typed(text, &vec![Type::UNKNOWN; values.len()])
                .await?;
            let values: Vec<TextParam> = values.iter().map(|v| TextParam(v.as_deref())).collect();
            let refs: Vec<&(dyn ToSql + Sync)> =
                values.iter().map(|v| v as &(dyn ToSql + Sync)).collect();
            c.query(&stmt, &refs).await?
        }
        Params::Generic => {
            // `$N` в Parse расширенного протокола становятся параметрами самой команды EXPLAIN, и план строится по
            // переданным значениям, а не обобщённый. Поэтому разбор той же строки расширенным протоколом только
            // проверяет, что инструкция одна, а план даёт простой протокол (без выполнения: GENERIC_PLAN
            // несовместим с ANALYZE).
            c.prepare(text).await?;
            let messages = c.simple_query(text).await?;
            let row = messages.iter().find_map(|m| match m {
                SimpleQueryMessage::Row(r) => Some(r),
                _ => None,
            });
            let json = row
                .and_then(|r| r.try_get(0).ok().flatten())
                .ok_or_else(|| Step::App(AppError::internal_with("EXPLAIN returned no rows")))?;
            return serde_json::from_str(json).map_err(|e| {
                Step::App(AppError::internal_with(format!(
                    "EXPLAIN output is not JSON: {e}"
                )))
            });
        }
        Params::None => c.query(text, &[]).await?,
    };
    let row = rows
        .first()
        .ok_or_else(|| Step::App(AppError::internal_with("EXPLAIN returned no rows")))?;
    row.try_get::<_, serde_json::Value>(0).map_err(|e| {
        Step::App(AppError::internal_with(format!(
            "EXPLAIN output is not JSON: {e}"
        )))
    })
}

fn probe_options(params: &Params) -> String {
    match params {
        Params::Generic => format!("{PROBE_OPTIONS}, GENERIC_PLAN"),
        _ => PROBE_OPTIONS.to_string(),
    }
}

fn result(
    mode: Mode,
    tier: Tier,
    tree: PlanTree,
    raw: serde_json::Value,
    started: Instant,
    warnings: Vec<String>,
) -> ExplainResult {
    let relations = relations(&tree);
    ExplainResult {
        mode,
        tier,
        plan: tree,
        raw,
        relations,
        elapsed_ms: started.elapsed().as_millis() as u64,
        warnings,
    }
}

fn parse(raw: &serde_json::Value) -> AppResult<PlanTree> {
    from_json(raw).map_err(|e| AppError::internal_with(format!("plan JSON: {}", e.message)))
}

pub async fn run(st: &AppState, id: &str, req: &ExplainRequest) -> AppResult<ExplainResult> {
    let sql = validate(req)?;
    let profile = st.profile(id)?;
    let started = Instant::now();
    let client =
        crate::actions::conn::open_explain(st, id, &req.member, Some(&req.database)).await?;
    let c = &client.client;
    let h = head(c).await?;
    if req.params == Params::Generic && h.version_num < V16 {
        return Err(app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.explain.genericUnsupported",
            "GENERIC_PLAN needs PostgreSQL 16 or newer: enter parameter values"
        ));
    }
    let mut warnings = Vec::new();
    if req.params == Params::Generic {
        warnings.push("genericPlan".to_string());
    }
    let sp = req.search_path.as_deref();
    // 1. Пробный план в READ ONLY; отвергнут 25006 (CREATE … AS на отдельных версиях) — повтор без READ ONLY,
    //    выполнения всё равно нет (уточнение 12 плана).
    let probe_opts = probe_options(&req.params);
    let probe_origin = Some(Origin::new(&probe_opts, &req.sql));
    let raw =
        match explain_in_tx(c, true, PROBE_TIMEOUT_S, sp, &probe_opts, &sql, &req.params).await {
            Ok(v) => v,
            Err(Step::Pg(e)) if is_read_only_violation(&e) => {
                warnings.push("probeWithoutReadOnly".to_string());
                match explain_in_tx(
                    c,
                    false,
                    PROBE_TIMEOUT_S,
                    sp,
                    &probe_opts,
                    &sql,
                    &req.params,
                )
                .await
                {
                    Ok(v) => v,
                    Err(Step::Pg(e)) => return Err(classify(&e, probe_origin)),
                    Err(Step::App(e)) => return Err(e),
                }
            }
            Err(Step::Pg(e)) => return Err(classify(&e, probe_origin)),
            Err(Step::App(e)) => return Err(e),
        };
    let tree = parse(&raw)?;
    let tier = if req.force_write {
        Tier::Write
    } else {
        tier_of(&tree, &sql)
    };
    if req.mode == Mode::Plan {
        return Ok(result(Mode::Plan, tier, tree, raw, started, warnings));
    }
    // 3–4. ANALYZE.
    let read_only = match tier {
        Tier::Read => true,
        Tier::Write => {
            ensure_mutation_allowed(&profile, req.confirmation.confirm.as_deref())?;
            if h.in_recovery {
                return Err(app_err!(
                    ErrorCode::FeatureUnavailable,
                    "errors.explain.writeOnReplica",
                    "A write cannot be executed on a replica: choose the primary"
                ));
            }
            false
        }
    };
    let analyzed = explain_in_tx(
        c,
        read_only,
        req.timeout_s,
        sp,
        ANALYZE_OPTIONS,
        &sql,
        &req.params,
    )
    .await;
    let analyze_origin = Some(Origin::new(ANALYZE_OPTIONS, &req.sql));
    let outcome = match &analyzed {
        Ok(_) => (journal::Outcome::Ok, None),
        Err(Step::Pg(e)) => {
            let err = classify(e, analyze_origin);
            (journal::Outcome::Error, Some(journal::code_name(err.code)))
        }
        Err(Step::App(e)) => (journal::Outcome::Error, Some(journal::code_name(e.code))),
    };
    if tier == Tier::Write {
        let tables = relations(&tree)
            .iter()
            .map(|r| match &r.schema {
                Some(s) => format!("{s}.{}", r.name),
                None => r.name.clone(),
            })
            .collect();
        let record = ActionRecord {
            ts: journal::now_ms(),
            action: "explainAnalyze".into(),
            connection: RecordConnection::of(&profile),
            member: Some(req.member.clone()),
            prod_confirmed: profile.environment == Environment::Prod,
            outcome: outcome.0,
            code: outcome.1.clone(),
            ms: started.elapsed().as_millis() as u64,
            sessions: vec![],
            slot: None,
            patroni: None,
            explain: Some(ExplainRecord {
                database: req.database.clone(),
                tables,
                timeout_s: req.timeout_s,
                sql_head: journal::cut(&sql, 200),
            }),
        };
        let j = st.actions.clone();
        let _ = crate::conn::run_blocking(move || Ok(j.append(&record))).await;
    }
    match analyzed {
        Ok(raw) => {
            let tree = parse(&raw)?;
            Ok(result(Mode::Analyze, tier, tree, raw, started, warnings))
        }
        Err(Step::Pg(e)) => Err(classify(&e, analyze_origin)),
        Err(Step::App(e)) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_position_maps_to_the_original_text() {
        let o = Origin::new(PROBE_OPTIONS, "  SELEC 1");
        let prefix = explain_prefix(PROBE_OPTIONS).chars().count() as u32;
        assert_eq!(
            o.position(prefix + 1),
            Some(3),
            "первый символ SQL после двух пробелов"
        );
        assert_eq!(
            o.position(prefix),
            None,
            "ошибка в префиксе EXPLAIN — без позиции"
        );
    }

    #[test]
    fn text_param_is_text_format_of_any_type_and_none_is_null() {
        let mut out = BytesMut::new();
        assert!(TextParam::accepts(&Type::INT8) && TextParam::accepts(&Type::TIMESTAMPTZ));
        assert!(matches!(
            TextParam(Some("42")).encode_format(&Type::INT8),
            Format::Text
        ));
        assert!(matches!(
            TextParam(Some("42")).to_sql(&Type::INT8, &mut out),
            Ok(IsNull::No)
        ));
        assert_eq!(&out[..], b"42");
        assert!(matches!(
            TextParam(None).to_sql(&Type::INT8, &mut out),
            Ok(IsNull::Yes)
        ));
        assert_eq!(&out[..], b"42", "NULL ничего не пишет");
    }
}
