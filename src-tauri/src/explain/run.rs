//! Ход EXPLAIN (спец. P3 §8.3): соединение `for_explain`, пробный план в `READ ONLY`, ярус, выполнение, журнал.

use std::future::Future;
use std::time::{Duration, Instant};

use bytes::BytesMut;
use pg_advisor::plan::json::from_json;
use pg_advisor::plan::{PlanTree, relations};
use tokio_postgres::error::{ErrorPosition, SqlState};
use tokio_postgres::types::{Format, IsNull, ToSql, Type, to_sql_checked};
use tokio_postgres::{Client, SimpleQueryMessage};

use crate::actions::journal::{self, ActionRecord, ExplainRecord, RecordConnection};
use crate::conn::errors::from_pg;
use crate::conn::profile::{ConnectionProfile, Environment};
use crate::diag::caps::V16;
use crate::diag::head;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::explain::params::first_word;
use crate::explain::tier::{Tier, admin_calls, tier_of};
use crate::explain::{
    ExplainRequest, ExplainResult, Mode, PROBE_TIMEOUT_S, Params, ensure_write_confirmed, validate,
};
use crate::guard::ensure_mutation_allowed;
use crate::product::SQL_MARKER;
use crate::state::AppState;

pub(crate) const PROBE_OPTIONS: &str = "FORMAT JSON, VERBOSE, SETTINGS";
const ANALYZE_OPTIONS: &str = "ANALYZE, BUFFERS, WAL, VERBOSE, SETTINGS, FORMAT JSON";
/// Запас клиентского срока над серверным `statement_timeout`: сервер успевает прервать запрос сам.
const CLIENT_MARGIN_S: u64 = 5;
/// Первые слова пишущих инструкций: даже EXPLAIN без выполнения проверяет у них права записи.
const WRITE_WORDS: [&str; 5] = ["insert", "update", "delete", "merge", "create"];

/// Ошибка одного EXPLAIN: сырая ошибка сервера нужна до перевода в `AppError` (25006, «вторая инструкция»).
pub(crate) enum Step {
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
/// выведенный тип (`int8`, `timestamptz`, …).
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
pub(crate) fn explain_prefix(options: &str) -> String {
    format!("EXPLAIN ({options}) ")
}

/// Обёртка EXPLAIN консоли Глафиры — свой запрос (замечание 13 смоука P3b): текст строки pgss начинается точным
/// префиксом команды с опциями Глафиры — пробный план (с `GENERIC_PLAN` и без) или выполнение. Метку `/* glafira */` в
/// начало команды Глафира не ставит: она сдвинула бы позицию ошибки сервера в тексте пользователя.
pub fn is_own_explain(text: &str) -> bool {
    let text = text.trim_start();
    [
        PROBE_OPTIONS.to_string(),
        probe_options(&Params::Generic),
        ANALYZE_OPTIONS.to_string(),
    ]
    .iter()
    .any(|o| text.starts_with(&explain_prefix(o)))
}

/// Где SQL пользователя в команде EXPLAIN: позиция ошибки сервера пересчитывается в позицию в исходном тексте.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Origin {
    /// Символов `EXPLAIN (…) ` перед SQL.
    prefix: usize,
    /// Ведущих пробельных символов исходного текста, снятых `normalize`.
    leading: usize,
}

impl Origin {
    pub(crate) fn new(options: &str, original: &str) -> Self {
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

/// Инструкция пишет: ярус записи или первое слово INSERT / UPDATE / DELETE / MERGE / CREATE (`tier` неизвестен, пока
/// пробный план не построен). `WITH … UPDATE` по первому слову не распознаётся — до плана подсказка о правах чтения.
fn is_write_statement(tier: Option<Tier>, sql: &str) -> bool {
    tier == Some(Tier::Write)
        || WRITE_WORDS.contains(&first_word(sql).to_ascii_lowercase().as_str())
}

/// Что известно о команде при переводе ошибки сервера.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Context {
    /// Где SQL пользователя в тексте команды; `None` — ошибка не в команде EXPLAIN (`BEGIN`, `SET LOCAL`).
    pub(crate) origin: Option<Origin>,
    /// Пишущая инструкция (`is_write_statement`): подсказка о правах записи, а не чтения.
    pub(crate) write: bool,
    /// Транзакция `READ ONLY` (пробный план, ярус чтения): 25006 в ней — `EXPLAIN_WRITES`; вне её — сервер не
    /// принимает запись вообще (повтор «как запись» ничего бы не дал).
    pub(crate) read_only: bool,
}

/// Поля ошибки сервера, по которым она переводится.
#[derive(Debug, Clone, Copy)]
struct ServerError<'a> {
    code: &'a SqlState,
    message: &'a str,
    position: Option<&'a ErrorPosition>,
    /// Функция сервера, поднявшая ошибку (поле `R`): признак, не зависящий от `lc_messages`.
    routine: Option<&'a str>,
}

/// Перевод ошибки сервера; не ответ сервера и прочие коды — через `from_pg`.
pub(crate) fn classify(e: &tokio_postgres::Error, ctx: Context) -> AppError {
    e.as_db_error()
        .and_then(|db| {
            classify_server(
                ServerError {
                    code: db.code(),
                    message: db.message(),
                    position: db.position(),
                    routine: db.routine(),
                },
                ctx,
            )
        })
        .unwrap_or_else(|| from_pg(e))
}

/// Вторая инструкция, 57014, 42501 (подсказка по типу инструкции), классы 42 и 22 (ошибки разбора и значений — с
/// позицией в тексте пользователя), 25006 (по режиму транзакции). Прочее — `None`. Позиция во внутреннем запросе
/// (тело функции) не показывается.
fn classify_server(e: ServerError, ctx: Context) -> Option<AppError> {
    let code = e.code;
    // Вторая инструкция: Parse расширенного протокола отвергает её в `exec_parse_message` (сообщение переводится по
    // `lc_messages`, поэтому английский текст — лишь запасной признак).
    if *code == SqlState::SYNTAX_ERROR
        && (e.routine == Some("exec_parse_message") || e.message.contains("multiple commands"))
    {
        return Some(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.oneStatement",
            "Enter exactly one statement"
        ));
    }
    if *code == SqlState::QUERY_CANCELED {
        return Some(explain_timeout());
    }
    if *code == SqlState::INSUFFICIENT_PRIVILEGE {
        let err = if ctx.write {
            app_err!(
                ErrorCode::NotAuthorized,
                "errors.explain.notAuthorizedWrite",
                "Not enough privileges: EXPLAIN of a write statement needs INSERT/UPDATE/DELETE privileges on its tables even without executing it"
            )
        } else {
            app_err!(
                ErrorCode::NotAuthorized,
                "errors.explain.notAuthorized",
                "Not enough privileges on the query's tables: grant pg_read_all_data (14+) or SELECT"
            )
        };
        return Some(err.with_details(e.message));
    }
    if code.code().starts_with("42") || code.code().starts_with("22") {
        let position = match (e.position, ctx.origin) {
            (Some(ErrorPosition::Original(n)), Some(o)) => o.position(*n),
            _ => None,
        };
        let err = app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.server",
            "The server rejected the query: {{message}}",
            message = e.message
        );
        return Some(match position {
            Some(p) => err.with_details(format!("position {p}")),
            None => err,
        });
    }
    if *code == SqlState::READ_ONLY_SQL_TRANSACTION {
        return Some(if ctx.read_only {
            explain_writes()
        } else {
            app_err!(
                ErrorCode::ReadOnly,
                "errors.explain.serverReadOnly",
                "The server does not accept writes in this session (read-only transaction default or standby)"
            )
            .with_details(e.message)
        });
    }
    None
}

fn explain_writes() -> AppError {
    app_err!(
        ErrorCode::ExplainWrites,
        "errors.explain.writes",
        "The query writes data (a volatile function or trigger): run it as a write with confirmation"
    )
}

pub(crate) fn explain_timeout() -> AppError {
    app_err!(
        ErrorCode::Timeout,
        "errors.explain.timeout",
        "The plan was not produced in time; execution was cancelled, nothing changed"
    )
}

/// Клиентский срок сетевого шага: серверный срок `secs` плюс `CLIENT_MARGIN_S`. По истечении вызывающий возвращает
/// ошибку и закрывает соединение (`Drop` у `ActionClient`), а сервер откатывает открытую транзакцию.
pub(crate) async fn within<T>(
    secs: u32,
    step: impl Future<Output = Result<T, Step>>,
) -> Result<T, Step> {
    tokio::time::timeout(Duration::from_secs(u64::from(secs) + CLIENT_MARGIN_S), step)
        .await
        .unwrap_or_else(|_| Err(Step::App(explain_timeout())))
}

/// Один EXPLAIN внутри транзакции: `begin_tx`, EXPLAIN, `ROLLBACK`.
/// Ошибка самой команды EXPLAIN возвращается сырой (`Step::Pg`): её переводит вызывающий.
pub(crate) async fn explain_in_tx(
    c: &Client,
    ctx: Context,
    timeout_s: u32,
    search_path: Option<&str>,
    options: &str,
    sql: &str,
    params: &Params,
) -> Result<serde_json::Value, Step> {
    let result = match begin_tx(c, ctx, timeout_s, search_path).await {
        Err(e) => Err(e),
        Ok(()) => explain_in_open_tx(c, options, sql, params).await,
    };
    // Откат — страховка: при ошибке транзакция уже прервана сервером, при обрыве соединение закроется.
    let _ = c.batch_execute(ROLLBACK_SQL).await;
    result
}

/// Откат транзакции EXPLAIN и гипотезы hypopg.
pub(crate) const ROLLBACK_SQL: &str = marked_sql!("ROLLBACK");

/// Служебные инструкции транзакции EXPLAIN — каждая со своей меткой (замечание 17 смоука P3b): pgss хранит текст каждой
/// инструкции пакета отдельно.
fn begin_sql(read_only: bool, timeout_s: u32, search_path: Option<&str>) -> String {
    let mut begin = format!(
        "{SQL_MARKER}BEGIN{}; {SQL_MARKER}SET LOCAL statement_timeout = '{timeout_s}s'; \
         {SQL_MARKER}SET LOCAL lock_timeout = '1s'",
        if read_only { " READ ONLY" } else { "" }
    );
    if let Some(sp) = search_path {
        begin.push_str(&format!("; {SQL_MARKER}SET LOCAL search_path = {sp}"));
    }
    begin
}

/// Начало транзакции EXPLAIN: `BEGIN [READ ONLY]` (по `ctx.read_only`), `SET LOCAL` сроков и `search_path`. Ошибка
/// начала (например, `search_path`) переводится сразу и без позиции: она не в тексте пользователя.
pub(crate) async fn begin_tx(
    c: &Client,
    ctx: Context,
    timeout_s: u32,
    search_path: Option<&str>,
) -> Result<(), Step> {
    let begin = begin_sql(ctx.read_only, timeout_s, search_path);
    c.batch_execute(&begin).await.map_err(|e| {
        Step::App(classify(
            &e,
            Context {
                origin: None,
                ..ctx
            },
        ))
    })
}

/// EXPLAIN в уже открытой транзакции (`begin_tx`): проба гипотезы hypopg идёт в одной транзакции с виртуальным индексом.
pub(crate) async fn explain_in_open_tx(
    c: &Client,
    options: &str,
    sql: &str,
    params: &Params,
) -> Result<serde_json::Value, Step> {
    explain_query(c, &format!("{}{sql}", explain_prefix(options)), params).await
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
            // 42P18 (тип параметра не выведен, как у `$1 IS NULL`) поднимается уже при анализе, после проверки
            // числа инструкций, — значит, инструкция одна; обобщённый план сервер такой строит. Ошибка разбора
            // прерывает транзакцию, поэтому проверка — под точкой сохранения, к которой тогда и откатываемся.
            c.batch_execute(marked_sql!("SAVEPOINT one_statement"))
                .await?;
            match c.prepare(text).await {
                Ok(_) => {}
                Err(e) if e.code() == Some(&SqlState::INDETERMINATE_DATATYPE) => {
                    c.batch_execute(marked_sql!("ROLLBACK TO SAVEPOINT one_statement"))
                        .await?;
                }
                Err(e) => return Err(e.into()),
            }
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

/// Контекст пробного плана. Обычно — `READ ONLY` (побочный эффект ловит сервер), подсказка 42501 — по первому слову.
/// «Выполнить как запись» (`force_write`, ограда уже пройдена) строит его вне `READ ONLY` и ярусом записи, как повтор
/// у CREATE … AS: иначе побочный эффект, вычисленный при планировании (25006 уже в пробе), был бы тупиком.
fn probe_context(force_write: bool, options: &str, original: &str, sql: &str) -> Context {
    Context {
        origin: Some(Origin::new(options, original)),
        write: is_write_statement(force_write.then_some(Tier::Write), sql),
        read_only: !force_write,
    }
}

/// Ограда яруса записи до выполнения: профиль (read-only, подтверждение prod), подтверждение в интерфейсе
/// (`write_confirmed`) и реплика (запись не примет). С `force_write` — до пробного плана, иначе — после него.
fn ensure_write_allowed(
    profile: &ConnectionProfile,
    req: &ExplainRequest,
    in_recovery: bool,
) -> AppResult<()> {
    ensure_mutation_allowed(profile, req.confirmation.confirm.as_deref())?;
    ensure_write_confirmed(req)?;
    if in_recovery {
        return Err(app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.explain.writeOnReplica",
            "A write cannot be executed on a replica: choose the primary"
        ));
    }
    Ok(())
}

pub(crate) fn probe_options(params: &Params) -> String {
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
    let report = pg_advisor::plan_rules::evaluate(&tree);
    ExplainResult {
        mode,
        tier,
        plan: tree,
        raw,
        relations,
        elapsed_ms: started.elapsed().as_millis() as u64,
        warnings,
        recommendations: report.recommendations,
        recommendations_skipped: report.skipped,
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
    let h = match tokio::time::timeout(
        Duration::from_secs(u64::from(PROBE_TIMEOUT_S) + CLIENT_MARGIN_S),
        head(c),
    )
    .await
    {
        Ok(h) => h?,
        Err(_) => return Err(explain_timeout()),
    };
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
    // «Выполнить как запись»: ограда и реплика — до пробного плана, потому что он идёт уже вне READ ONLY.
    if req.force_write {
        ensure_write_allowed(&profile, req, h.in_recovery)?;
    }
    // 1. Пробный план в READ ONLY (с `force_write` — вне его, см. `probe_context`). CREATE … AS, отвергнутый 25006
    //    (на отдельных версиях), повторяется без READ ONLY — только ради яруса, выполнения всё равно нет (уточнение 12
    //    плана). Прочие 25006 в READ ONLY — `EXPLAIN_WRITES`: побочный эффект, вычисленный при планировании, в запросе
    //    «план» не выполняется; повтор с `force_write` строит план вне READ ONLY.
    let probe_opts = probe_options(&req.params);
    let probe_ctx = probe_context(req.force_write, &probe_opts, &req.sql, &sql);
    let probe = |ctx| {
        within(
            PROBE_TIMEOUT_S,
            explain_in_tx(c, ctx, PROBE_TIMEOUT_S, sp, &probe_opts, &sql, &req.params),
        )
    };
    let raw = match probe(probe_ctx).await {
        Ok(v) => v,
        Err(Step::Pg(e))
            if probe_ctx.read_only
                && is_read_only_violation(&e)
                && first_word(&sql).eq_ignore_ascii_case("create") =>
        {
            warnings.push("probeWithoutReadOnly".to_string());
            let ctx = Context {
                read_only: false,
                ..probe_ctx
            };
            match probe(ctx).await {
                Ok(v) => v,
                Err(Step::Pg(e)) => return Err(classify(&e, ctx)),
                Err(Step::App(e)) => return Err(e),
            }
        }
        Err(Step::Pg(e)) => return Err(classify(&e, probe_ctx)),
        Err(Step::App(e)) => return Err(e),
    };
    let tree = parse(&raw)?;
    // Вызов административной функции (`tier::ADMIN_FUNCTIONS`) переводит инструкцию в ярус записи: READ ONLY её не остановит.
    if !admin_calls(&raw).is_empty() {
        warnings.push("adminFunction".to_string());
    }
    let tier = if req.force_write {
        Tier::Write
    } else {
        tier_of(&tree, &sql, &raw)
    };
    if req.mode == Mode::Plan {
        return Ok(result(Mode::Plan, tier, tree, raw, started, warnings));
    }
    // 3–4. ANALYZE. С `force_write` ограда уже пройдена до пробного плана.
    if tier == Tier::Write && !req.force_write {
        ensure_write_allowed(&profile, req, h.in_recovery)?;
    }
    let analyze_ctx = Context {
        origin: Some(Origin::new(ANALYZE_OPTIONS, &req.sql)),
        write: is_write_statement(Some(tier), &sql),
        read_only: tier == Tier::Read,
    };
    let analyzed = within(
        req.timeout_s,
        explain_in_tx(
            c,
            analyze_ctx,
            req.timeout_s,
            sp,
            ANALYZE_OPTIONS,
            &sql,
            &req.params,
        ),
    )
    .await;
    let outcome = match &analyzed {
        Ok(_) => (journal::Outcome::Ok, None),
        Err(Step::Pg(e)) => {
            let err = classify(e, analyze_ctx);
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
        Err(Step::Pg(e)) => Err(classify(&e, analyze_ctx)),
        Err(Step::App(e)) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_statements_are_marked() {
        let begin = begin_sql(true, 30, Some("\"app\", public"));
        let parts: Vec<&str> = begin.split("; ").collect();
        assert_eq!(parts.len(), 4, "{begin}");
        assert!(
            parts.iter().all(|p| crate::product::is_marked_sql(p)),
            "{begin}"
        );
        assert!(
            parts[0].ends_with("BEGIN READ ONLY")
                && parts[3].ends_with("search_path = \"app\", public")
        );
        assert!(begin_sql(false, 5, None).starts_with("/* glafira */ BEGIN; "));
        assert!(crate::product::is_marked_sql(ROLLBACK_SQL));
    }

    #[test]
    fn explain_wrappers_of_glafira_are_own() {
        for options in [
            probe_options(&Params::None),
            probe_options(&Params::Generic),
            ANALYZE_OPTIONS.to_string(),
        ] {
            let text = format!("{}SELECT * FROM t WHERE id = $1", explain_prefix(&options));
            assert!(is_own_explain(&text), "{text}");
            assert!(crate::diag::texts::is_own_text(&text), "{text}");
        }
        assert!(is_own_explain(
            "EXPLAIN (FORMAT JSON, VERBOSE, SETTINGS, GENERIC_PLAN) SELECT $1"
        ));
        assert!(is_own_explain(
            "EXPLAIN (ANALYZE, BUFFERS, WAL, VERBOSE, SETTINGS, FORMAT JSON) SELECT 1"
        ));
        // EXPLAIN пользователя с другими опциями — не свой.
        assert!(!is_own_explain("EXPLAIN (FORMAT JSON) SELECT 1"));
        assert!(!is_own_explain("EXPLAIN ANALYZE SELECT 1"));
        assert!(!is_own_explain(
            "explain (format json, verbose, settings) SELECT 1"
        ));
        assert!(!is_own_explain("SELECT 1"));
    }

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
    fn write_statement_by_tier_or_first_word() {
        assert!(!is_write_statement(None, "SELECT * FROM t"));
        assert!(!is_write_statement(Some(Tier::Read), "select 1"));
        assert!(
            is_write_statement(Some(Tier::Write), "SELECT nextval('s')"),
            "ярус записи (forceWrite)"
        );
        for sql in [
            "UPDATE t SET a = 1",
            "/* c */ insert into t values (1)",
            "delete from t",
            "MERGE INTO t USING s ON true WHEN MATCHED THEN DELETE",
            "-- x\n create table t2 as select 1",
        ] {
            assert!(is_write_statement(None, sql), "{sql}");
        }
        assert!(
            !is_write_statement(None, "with d as (delete from t returning *) select 1"),
            "до плана — только по первому слову"
        );
    }

    #[test]
    fn force_write_probes_outside_read_only_as_a_write() {
        let plain = probe_context(
            false,
            PROBE_OPTIONS,
            "SELECT lo_creat(-1)",
            "SELECT lo_creat(-1)",
        );
        assert!(
            plain.read_only && !plain.write,
            "без force_write — READ ONLY"
        );
        assert!(
            probe_context(
                false,
                PROBE_OPTIONS,
                "update t set a = 1",
                "update t set a = 1"
            )
            .write,
            "подсказка 42501 по первому слову"
        );
        let forced = probe_context(
            true,
            PROBE_OPTIONS,
            " SELECT lo_creat(-1)",
            "SELECT lo_creat(-1)",
        );
        assert!(
            !forced.read_only && forced.write,
            "force_write — вне READ ONLY, подсказка записи"
        );
        assert_eq!(
            forced
                .origin
                .and_then(|o| o.position(explain_prefix(PROBE_OPTIONS).chars().count() as u32 + 1)),
            Some(2),
            "позиция — относительно текста пользователя"
        );
    }

    fn ctx(read_only: bool, write: bool) -> Context {
        Context {
            origin: Some(Origin::new(PROBE_OPTIONS, "  SELEC 1")),
            write,
            read_only,
        }
    }

    fn server<'a>(
        code: &'a SqlState,
        message: &'a str,
        position: Option<&'a ErrorPosition>,
        routine: Option<&'a str>,
    ) -> ServerError<'a> {
        ServerError {
            code,
            message,
            position,
            routine,
        }
    }

    #[test]
    fn server_errors_are_classified_by_sqlstate_and_context() {
        let prefix = explain_prefix(PROBE_OPTIONS).chars().count() as u32;
        let at = |n: u32| ErrorPosition::Original(prefix + n);
        // Вторая инструкция — по функции сервера, независимо от языка сообщения; английский текст — запасной признак.
        let ru = "нельзя вставить несколько команд в подготовленный оператор";
        let e = classify_server(
            server(
                &SqlState::SYNTAX_ERROR,
                ru,
                None,
                Some("exec_parse_message"),
            ),
            ctx(true, false),
        )
        .unwrap();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::InvalidInput, "errors.explain.oneStatement")
        );
        let en = "cannot insert multiple commands into a prepared statement";
        assert_eq!(
            classify_server(
                server(&SqlState::SYNTAX_ERROR, en, None, None),
                ctx(true, false)
            )
            .unwrap()
            .key,
            "errors.explain.oneStatement"
        );
        // Классы 42 и 22 — INVALID_INPUT с позицией в тексте пользователя, если сервер её дал.
        let p = at(1);
        let e = classify_server(
            server(
                &SqlState::SYNTAX_ERROR,
                "syntax error at or near \"SELEC\"",
                Some(&p),
                Some("scanner_yyerror"),
            ),
            ctx(true, false),
        )
        .unwrap();
        assert_eq!(
            (e.code, e.key.as_str(), e.details.as_deref()),
            (
                ErrorCode::InvalidInput,
                "errors.explain.server",
                Some("position 3")
            )
        );
        let p = at(15);
        let e = classify_server(
            server(
                &SqlState::UNDEFINED_TABLE,
                "relation \"shop.nope\" does not exist",
                Some(&p),
                None,
            ),
            ctx(true, false),
        )
        .unwrap();
        assert_eq!(
            (
                e.key.as_str(),
                e.params["message"].as_str(),
                e.details.as_deref()
            ),
            (
                "errors.explain.server",
                "relation \"shop.nope\" does not exist",
                Some("position 17")
            )
        );
        let e = classify_server(
            server(
                &SqlState::INVALID_TEXT_REPRESENTATION,
                "invalid input syntax for type bigint: \"abc\"",
                None,
                None,
            ),
            ctx(true, false),
        )
        .unwrap();
        assert_eq!(
            (e.code, e.key.as_str(), e.details),
            (ErrorCode::InvalidInput, "errors.explain.server", None)
        );
        let internal = ErrorPosition::Internal {
            position: 3,
            query: "SELECT x".into(),
        };
        let e = classify_server(
            server(
                &SqlState::UNDEFINED_COLUMN,
                "column \"x\" does not exist",
                Some(&internal),
                None,
            ),
            ctx(true, false),
        )
        .unwrap();
        assert_eq!(
            e.details, None,
            "позиция во внутреннем запросе не показывается"
        );
        // 42501: подсказка по типу инструкции, сообщение сервера — в details.
        let denied = "permission denied for table balances";
        let e = classify_server(
            server(&SqlState::INSUFFICIENT_PRIVILEGE, denied, None, None),
            ctx(true, true),
        )
        .unwrap();
        assert_eq!(
            (e.code, e.key.as_str(), e.details.as_deref()),
            (
                ErrorCode::NotAuthorized,
                "errors.explain.notAuthorizedWrite",
                Some(denied)
            )
        );
        let e = classify_server(
            server(&SqlState::INSUFFICIENT_PRIVILEGE, denied, None, None),
            ctx(true, false),
        )
        .unwrap();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::NotAuthorized, "errors.explain.notAuthorized")
        );
        // 25006: в READ ONLY — EXPLAIN_WRITES (предложить запись), вне его — сервер не принимает запись (без петли forceWrite).
        let ro = "cannot execute UPDATE in a read-only transaction";
        let e = classify_server(
            server(&SqlState::READ_ONLY_SQL_TRANSACTION, ro, None, None),
            ctx(true, true),
        )
        .unwrap();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::ExplainWrites, "errors.explain.writes")
        );
        let e = classify_server(
            server(&SqlState::READ_ONLY_SQL_TRANSACTION, ro, None, None),
            ctx(false, true),
        )
        .unwrap();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::ReadOnly, "errors.explain.serverReadOnly")
        );
        // 57014 — срок; прочее — `from_pg`.
        assert_eq!(
            classify_server(
                server(
                    &SqlState::QUERY_CANCELED,
                    "canceling statement due to statement timeout",
                    None,
                    None
                ),
                ctx(false, true)
            )
            .unwrap()
            .code,
            ErrorCode::Timeout
        );
        assert!(
            classify_server(
                server(&SqlState::TOO_MANY_CONNECTIONS, "too many", None, None),
                ctx(true, false)
            )
            .is_none()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn client_deadline_expires_after_the_server_timeout_and_a_margin() {
        let never = std::future::pending::<Result<(), Step>>();
        let started = tokio::time::Instant::now();
        match within(5, never).await {
            Err(Step::App(e)) => assert_eq!(
                (e.code, e.key.as_str()),
                (ErrorCode::Timeout, "errors.explain.timeout")
            ),
            _ => panic!("срок должен истечь"),
        }
        assert_eq!(started.elapsed(), Duration::from_secs(5 + CLIENT_MARGIN_S));
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
