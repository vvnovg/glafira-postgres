//! Слои защиты SQL модели на сервере (спец. P4 §7.3, шаги 1–3), общие для `explain_query`, `hypothetical_index` и P4b
//! (`run_select`, проверка предложения): своё соединение на вызов (`for_mcp`: сессия EXPLAIN с
//! `default_transaction_read_only = on` — только чтение и вне транзакции защиты, `application_name = glafira-pg-mcp`:
//! нагрузку модели видно в `pg_stat_activity` отдельно от Glafira), `standard_conforming_strings = on` на всю сессию
//! (разбор `sqlcheck` и сервера сходятся, в том числе в транзакциях hypopg на том же соединении), транзакция
//! `READ ONLY` со сроками, `prepare` под точкой сохранения (сервер подтверждает, что выражение одно) и проба
//! `EXPLAIN (FORMAT JSON, VERBOSE, SETTINGS)` с поиском запрещённых функций в выражениях плана. В конце — `ROLLBACK`
//! (`finish`). Текст модели `prepare_one` и `probe` получают как есть и нормализуют сами: позиции ошибок сервера — в
//! исходном тексте (ведущие пробельные символы, снятые `normalize`, прибавляются, как у `Origin` P3).
//!
//! Сброс. Ядро шлюза бросает тело вызова на любом `await`, когда сессия MCP закрывается. Транзакция и виртуальные
//! индексы hypopg живут только в сессии сервера этого соединения, а соединение — только в `GuardTx`: сброс `GuardTx`
//! (и `finish`) закрывает его (`ActionClient` прерывает задачу соединения, сокет закрывается), сервер завершает
//! процесс сессии — транзакция откатывается, виртуальные индексы исчезают вместе с памятью сессии. Пулы приложения
//! для SQL модели не используются: брошенное тело вернуло бы в пул клиента с открытой транзакцией.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use glafira_mcp_core::error::ToolError;
use tokio_postgres::Client;
use tokio_postgres::error::{ErrorPosition, SqlState};

use super::access::{err, tool_error};
use crate::actions::conn::{ActionClient, open_with};
use crate::conn::errors::from_pg;
use crate::conn::pg::ConnectSpec;
use crate::diag::head;
use crate::error::AppError;
use crate::explain::Params;
use crate::explain::params::{first_word, normalize};
use crate::explain::run::{
    Context, Origin, ROLLBACK_SQL, Step, classify, explain_in_open_tx, probe_options, within,
};
use crate::product::SQL_MARKER;
use crate::state::AppState;

/// Настройка сессии SQL модели — своим запросом, до транзакции: в одном пакете с `BEGIN` сервер включает `SET` в
/// транзакцию (неявный блок пакета становится явным), и `ROLLBACK` вернул бы значение базы или роли — а на том же
/// соединении после отката идут транзакции hypopg.
pub(crate) const SESSION_SQL: &str = marked_sql!("SET standard_conforming_strings = on");
const SAVEPOINT_SQL: &str = marked_sql!("SAVEPOINT glafira_mcp_one");
const ROLLBACK_TO_SQL: &str = marked_sql!("ROLLBACK TO SAVEPOINT glafira_mcp_one");
/// Клиентский срок `ROLLBACK` в `finish`: занятый сервер (истёк клиентский срок шага) его не дождётся, транзакцию
/// откатит закрытие соединения.
const FINISH_ROLLBACK: Duration = Duration::from_secs(1);
/// Классы ошибок сервера при `prepare`, которые говорят о самом тексте запроса: данные (22), синтаксис и правила
/// доступа (42), неподдерживаемое (0A), пределы программы (54), кардинальность (21).
const SQL_ERROR_CLASSES: [&str; 5] = ["22", "42", "0A", "54", "21"];
const PLAN_WRITES: &str = "Planning the query tried to write (a function evaluated at planning time is not read-only); the gateway plans queries only in READ ONLY transactions";
const HYPOPG_MISSING: &str = "hypopg is not installed in this database: ask the user to run CREATE EXTENSION hypopg (the hypopg package must be installed on the server)";

/// Транзакция: каждая инструкция — со своей меткой (pgss хранит их по отдельности).
fn begin_sql(timeout_s: u32) -> String {
    format!(
        "{SQL_MARKER}BEGIN READ ONLY; {SQL_MARKER}SET LOCAL statement_timeout = '{timeout_s}s'; \
         {SQL_MARKER}SET LOCAL lock_timeout = '1s'"
    )
}

/// Параметры соединения SQL модели: как у EXPLAIN P3 (схемы пользователя видны), только чтение на всю сессию, своё
/// `application_name`.
fn mcp_session(spec: ConnectSpec) -> ConnectSpec {
    spec.for_mcp()
}

/// Ведущих пробельных символов в тексте модели — их снимает `normalize` (`str::trim`), и сервер их не видит
/// (так же считает `Origin` P3).
pub(crate) fn leading_chars(original: &str) -> usize {
    original.chars().take_while(|c| c.is_whitespace()).count()
}

/// Сообщение с позицией в тексте модели: позиция по нормализованному тексту (символы с 1) плюс `leading`.
pub(crate) fn with_position(message: &str, position: Option<usize>, leading: usize) -> String {
    match position {
        Some(p) => format!("{message} (position {})", p + leading),
        None => message.to_string(),
    }
}

/// Сообщение об ошибке SQL и подсказка после него (текст и позиция — как были).
pub(crate) fn with_hint(message: &str, hint: &str) -> String {
    format!("{message}. {hint}")
}

/// Служебный шаг (сессия, версия, точка сохранения): его ошибка — не в тексте модели.
fn service_ctx() -> Context {
    Context {
        origin: None,
        write: false,
        read_only: true,
    }
}

/// Ошибка SQL модели (спец. §10.2): отказ сервера при разборе и планировании — `invalid_sql` с его текстом и позицией;
/// запись во время планирования — тоже `invalid_sql` (шлюз строит планы только в `READ ONLY`); без hypopg —
/// `FEATURE_UNAVAILABLE` с подсказкой; прочее — как у остальных инструментов (`tool_error`).
pub(crate) fn sql_error(e: AppError) -> ToolError {
    match e.key.as_str() {
        "errors.explain.server" | "errors.explain.serverMissing" | "errors.hypo.rejected" => {
            let message = match &e.details {
                Some(d) => format!("{} ({d})", e.message),
                None => e.message.clone(),
            };
            err("invalid_sql", message)
        }
        "errors.explain.oneStatement" => err("invalid_sql", "Pass exactly one SQL statement"),
        "errors.explain.writes" => err("invalid_sql", PLAN_WRITES),
        "errors.hypo.notInstalled" => err("FEATURE_UNAVAILABLE", HYPOPG_MISSING),
        _ => tool_error(e),
    }
}

fn step_error(s: Step, ctx: Context) -> ToolError {
    sql_error(match s {
        Step::Pg(e) => classify(&e, ctx),
        Step::App(e) => e,
    })
}

/// Отказ сервера при `prepare`: ошибка самого текста — `invalid_sql` с его текстом и позицией в тексте модели
/// (символы с 1; `leading` — ведущие пробельные символы, снятые `normalize`), у синтаксической (SQLSTATE 42601) —
/// ещё `syntax_hint` после них; срок и права — `TIMEOUT` и `NOT_AUTHORIZED`, как у пробы; прочее (соединение, сервер) —
/// как у остальных инструментов.
fn prepare_error(
    e: &tokio_postgres::Error,
    leading: usize,
    syntax_hint: Option<&str>,
) -> ToolError {
    let Some(db) = e.as_db_error() else {
        return tool_error(from_pg(e));
    };
    let code = db.code();
    if *code == SqlState::QUERY_CANCELED || *code == SqlState::INSUFFICIENT_PRIVILEGE {
        return sql_error(classify(e, service_ctx()));
    }
    if !SQL_ERROR_CLASSES.iter().any(|c| code.code().starts_with(c)) {
        return tool_error(from_pg(e));
    }
    let position = match db.position() {
        Some(ErrorPosition::Original(p)) => Some(*p as usize),
        _ => None,
    };
    let message = with_position(db.message(), position, leading);
    match syntax_hint {
        Some(hint) if *code == SqlState::SYNTAX_ERROR => {
            err("invalid_sql", with_hint(&message, hint))
        }
        _ => err("invalid_sql", message),
    }
}

/// Сессия и транзакция §7.3 на открытом соединении: `SESSION_SQL` своим запросом, затем `BEGIN READ ONLY` и
/// `SET LOCAL` сроков (`timeout_s`).
pub async fn open_session(c: &Client, timeout_s: u32) -> Result<(), ToolError> {
    let sql = begin_sql(timeout_s);
    within(timeout_s, async {
        c.batch_execute(SESSION_SQL).await?;
        c.batch_execute(&sql).await?;
        Ok(())
    })
    .await
    .map_err(|s| step_error(s, service_ctx()))
}

/// Открытая транзакция защиты на своём соединении SQL модели; сброс закрывает соединение (см. описание модуля).
pub struct GuardTx {
    client: ActionClient,
    member: String,
    version_num: i32,
    timeout_s: u32,
    /// Транзакция ещё не откачена (`rollback` / `finish`).
    open: AtomicBool,
}

/// Новое соединение с узлом (`member` или «за лидером») и база `database`, сессия и транзакция §7.3 со сроком
/// `timeout_s` (5 с у EXPLAIN, 10 с у `run_select` P4b).
pub async fn begin(
    st: &AppState,
    connection_id: &str,
    member: Option<&str>,
    database: &str,
    timeout_s: u32,
) -> Result<GuardTx, ToolError> {
    let member = st
        .runtimes
        .diag_target(connection_id, member, false)
        .map_err(tool_error)?
        .member
        .map(|m| m.name)
        .ok_or_else(|| tool_error(AppError::internal_with("guard without a member")))?;
    let client = open_with(st, connection_id, &member, Some(database), mcp_session)
        .await
        .map_err(tool_error)?;
    let c = &client.client;
    let h = within(timeout_s, async { head(c).await.map_err(Step::App) })
        .await
        .map_err(|s| step_error(s, service_ctx()))?;
    open_session(c, timeout_s).await?;
    Ok(GuardTx {
        client,
        member,
        version_num: h.version_num,
        timeout_s,
        open: AtomicBool::new(true),
    })
}

impl GuardTx {
    pub fn client(&self) -> &Client {
        &self.client.client
    }

    pub fn member(&self) -> &str {
        &self.member
    }

    /// `server_version_num` узла (GENERIC_PLAN — 16+).
    pub fn version_num(&self) -> i32 {
        self.version_num
    }

    /// Сервер разбирает текст расширенным протоколом под точкой сохранения: второе выражение он отвергает (механизм P3),
    /// ошибка разбора и анализа — `invalid_sql` с его текстом и позицией в `sql`. Невыведенный тип параметра
    /// (`$1 IS NULL`) — не ошибка: анализ идёт уже после проверки числа выражений. Без выполнения: `prepare` — только
    /// Parse и Describe. `sql` — текст, как его прислала модель (нормализуется здесь).
    pub async fn prepare_one(&self, sql: &str) -> Result<(), ToolError> {
        self.prepare_one_hinted(sql, None).await
    }

    /// `prepare_one` с подсказкой `syntax_hint` после текста и позиции синтаксической ошибки сервера (SQLSTATE 42601):
    /// у текста pgss по `queryId` (E5) — он нормализован и может не разбираться как есть.
    pub async fn prepare_one_hinted(
        &self,
        sql: &str,
        syntax_hint: Option<&str>,
    ) -> Result<(), ToolError> {
        let c = self.client();
        let text = normalize(sql);
        let rejected = within(self.timeout_s, async {
            c.batch_execute(SAVEPOINT_SQL).await?;
            match c.prepare(&text).await {
                Ok(_) => Ok(None),
                Err(e) if e.code() == Some(&SqlState::INDETERMINATE_DATATYPE) => {
                    c.batch_execute(ROLLBACK_TO_SQL).await?;
                    Ok(None)
                }
                Err(e) if e.as_db_error().is_some() => {
                    c.batch_execute(ROLLBACK_TO_SQL).await?;
                    Ok(Some(e))
                }
                Err(e) => Err(Step::Pg(e)),
            }
        })
        .await
        .map_err(|s| step_error(s, service_ctx()))?;
        match rejected {
            None => Ok(()),
            Some(e) => Err(prepare_error(&e, leading_chars(sql), syntax_hint)),
        }
    }

    /// Проба `EXPLAIN (FORMAT JSON, VERBOSE, SETTINGS[, GENERIC_PLAN])` в открытой транзакции и поиск запрещённых функций
    /// в выражениях плана (`functions::denied_in_plan`): ловит расхождение нашего разбора с сервером, представления и
    /// встроенные SQL-функции. План — итог пробы. `sql` — текст, как его прислала модель: EXPLAIN идёт по
    /// нормализованному, позиция ошибки сервера пересчитывается в исходный (`Origin`).
    pub async fn probe(&self, sql: &str, params: &Params) -> Result<serde_json::Value, ToolError> {
        let options = probe_options(params);
        let text = normalize(sql);
        let write = matches!(
            first_word(&text).to_ascii_lowercase().as_str(),
            "insert" | "update" | "delete" | "merge"
        );
        let ctx = Context {
            origin: Some(Origin::new(&options, sql)),
            write,
            read_only: true,
        };
        let raw = within(
            self.timeout_s,
            explain_in_open_tx(self.client(), &options, &text, params),
        )
        .await
        .map_err(|s| step_error(s, ctx))?;
        if let Some(name) = pg_advisor::functions::denied_in_plan(&raw) {
            return Err(err(
                "forbidden_function",
                format!(
                    "Function {name} is not allowed: the plan of the query calls it (possibly inside a view or an inlined SQL function)"
                ),
            ));
        }
        Ok(raw)
    }

    /// `ROLLBACK` транзакции защиты; соединение остаётся (на нём идут транзакции hypopg `check_many`, сессия — с той
    /// же `standard_conforming_strings = on`).
    pub async fn rollback(&self) -> Result<(), ToolError> {
        if self.open.swap(false, Ordering::SeqCst) {
            within(self.timeout_s, async {
                Ok(self.client().batch_execute(ROLLBACK_SQL).await?)
            })
            .await
            .map_err(|s| step_error(s, service_ctx()))?;
        }
        Ok(())
    }

    /// Конец: `ROLLBACK`, если транзакция открыта (не дольше `FINISH_ROLLBACK`), и закрытие соединения (сброс
    /// `ActionClient`).
    pub async fn finish(self) {
        if self.open.swap(false, Ordering::SeqCst) {
            let _ =
                tokio::time::timeout(FINISH_ROLLBACK, self.client().batch_execute(ROLLBACK_SQL))
                    .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    /// Транзакция `READ ONLY` со сроками; `standard_conforming_strings = on` — отдельной инструкцией сессии ДО пакета
    /// транзакции: в одном пакете с `BEGIN` сервер включает её в транзакцию, и `ROLLBACK` возвращал бы значение
    /// базы или роли (проверено на PG 17: `SET …; BEGIN` одним запросом, затем `ROLLBACK` — настройка откатилась).
    #[test]
    fn transaction_statements_are_marked_and_read_only() {
        let sql = begin_sql(5);
        let parts: Vec<&str> = sql.split("; ").collect();
        assert_eq!(parts.len(), 3, "{sql}");
        assert!(
            parts.iter().all(|p| crate::product::is_marked_sql(p)),
            "{sql}"
        );
        assert!(parts[0].ends_with("BEGIN READ ONLY"), "{sql}");
        assert!(
            parts[1].ends_with("SET LOCAL statement_timeout = '5s'"),
            "{sql}"
        );
        assert!(parts[2].ends_with("SET LOCAL lock_timeout = '1s'"), "{sql}");
        assert!(
            !sql.contains("standard_conforming_strings"),
            "настройка сессии — вне пакета транзакции: {sql}"
        );
        assert!(SESSION_SQL.ends_with("SET standard_conforming_strings = on"));
        for s in [SESSION_SQL, SAVEPOINT_SQL, ROLLBACK_TO_SQL] {
            assert!(crate::product::is_marked_sql(s), "{s}");
        }
    }

    #[test]
    fn model_connection_has_its_own_application_name() {
        let spec = ConnectSpec::build(
            &crate::conn::profile::sample_profile("a"),
            "h",
            5432,
            "postgres",
            &crate::secrets::MemoryStore::default(),
        )
        .unwrap();
        let cfg = mcp_session(spec).pg_config();
        assert_eq!(cfg.get_application_name(), Some("glafira-pg-mcp"));
        // Вся сессия — только чтение (защита вглубь: и вне транзакции `GuardTx`).
        assert_eq!(
            cfg.get_options(),
            Some(crate::conn::pg::MCP_SESSION_OPTIONS)
        );
        assert!(
            cfg.get_options()
                .is_some_and(|o| o.contains("default_transaction_read_only=on"))
        );
    }

    /// Позиция отказа сервера при `prepare` — в тексте модели: сервер считает от начала нормализованного текста,
    /// ведущие пробельные символы (`normalize`) прибавляются.
    #[test]
    fn prepare_positions_count_the_leading_whitespace() {
        assert_eq!(
            with_position("column \"nope\" does not exist", Some(8), 0),
            "column \"nope\" does not exist (position 8)"
        );
        assert_eq!(
            with_position(
                "column \"nope\" does not exist",
                Some(8),
                leading_chars(" \n SELECT nope")
            ),
            "column \"nope\" does not exist (position 11)"
        );
        assert_eq!(with_position("x", None, 3), "x");
        assert_eq!(
            leading_chars("\u{3000}\tSELECT 1"),
            2,
            "символы, а не байты"
        );
    }

    #[test]
    fn server_rejections_become_invalid_sql_with_the_position() {
        let e = sql_error(
            app_err!(
                ErrorCode::InvalidInput,
                "errors.explain.server",
                "The server rejected the query: {{message}}",
                message = "syntax error at or near \"FORM\""
            )
            .with_details("position 10"),
        );
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "invalid_sql",
                "The server rejected the query: syntax error at or near \"FORM\" (position 10)"
            )
        );
        let e = sql_error(app_err!(
            ErrorCode::ExplainWrites,
            "errors.explain.writes",
            "The query writes data (a volatile function or trigger): run it as a write with confirmation"
        ));
        assert_eq!(e.code, "invalid_sql");
        assert!(!e.message.contains("confirmation"), "{}", e.message);
        let e = sql_error(app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.hypo.notInstalled",
            "hypopg is not installed in this database"
        ));
        assert_eq!(e.code, "FEATURE_UNAVAILABLE");
        assert!(
            e.message.contains("CREATE EXTENSION hypopg"),
            "{}",
            e.message
        );
        let e = sql_error(app_err!(
            ErrorCode::Timeout,
            "errors.explain.timeout",
            "The plan was not produced in time; execution was cancelled, nothing changed"
        ));
        assert_eq!(e.code, "TIMEOUT");
    }
}
