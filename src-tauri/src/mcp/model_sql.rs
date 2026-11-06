//! SQL модели (спец. P4 §7.1) до соединения и источник текста запроса: длина и `sqlcheck` с позицией отказа в тексте,
//! как его прислала модель (§16.17), текст pgss по `queryId` (те же проверки, подсказка к синтаксической ошибке
//! нормализованного текста — E5), строки гипотетических индексов (имя снимается — E4), «ровно один из `sql` и
//! `queryId`», база вызова по умолчанию (§16.21). Общие для инструментов P4a (`mcp::read`), `run_select` и
//! предложений P4b; перенесены из `read.rs` без изменения поведения.

use glafira_mcp_core::error::ToolError;
use glafira_pg_mcp::params::{SQL_AND_QUERY_ID_BOTH, SQL_AND_QUERY_ID_NEITHER};
use pg_advisor::sqlcheck::{self, Checked, Purpose};
use pg_advisor::sqltext::cast_typed_literal_params;

use super::access::{err, invalid_argument, tool_error};
use super::guard;
use crate::conn::errors::from_pg;
use crate::diag::on_member;
use crate::diag::queries::{Detect, detect};
use crate::diag::query_detail;
use crate::explain::MAX_SQL_BYTES;
use crate::explain::hypopg::{self, MAX_HYPO_INDEXES};
use crate::explain::params::normalize;
use crate::product::SQL_MARKER;
use crate::sampler::statements::{pgss_missing, pgss_too_old};
use crate::state::AppState;
use crate::storage::connections::ConnectionProfile;

/// База вызова: заданная моделью, иначе база подключения по умолчанию (`database` в `list_connections`). Пустая строка
/// — как «не задана» (крейт шлюза уже обнуляет её в `validate`; здесь — страховка для прямых вызовов).
pub(crate) fn resolve_database(profile: &ConnectionProfile, asked: Option<&str>) -> String {
    match asked.map(str::trim) {
        Some(d) if !d.is_empty() => d.to_string(),
        _ => profile.pg.database.clone(),
    }
}

/// Текст pgss чужого запроса без `pg_read_all_stats`.
const HIDDEN_PGSS_TEXT: &str = "<insufficient privilege>";

/// Проверка SQL модели до соединения (§7.1): длина и `sqlcheck` нормализованного текста (без ведущих пробелов и
/// хвостовых `;` — его же получит сервер), позиция отказа — в тексте `original`, как его прислала модель (ведущие
/// пробельные символы прибавляются). `pg_query` разбирает на рабочем потоке tokio (стек 2 МиБ). Итог — разбор
/// `sqlcheck` (вид выражения, функции, таблицы): функции сверяет с `pg_proc` `run_select` (P4b).
pub(crate) fn check_model_sql(original: &str, purpose: Purpose) -> Result<Checked, ToolError> {
    let sql = normalize(original);
    if sql.len() > MAX_SQL_BYTES {
        return Err(invalid_argument(format!(
            "The SQL is longer than {MAX_SQL_BYTES} bytes"
        )));
    }
    sqlcheck::check(&sql, purpose).map_err(|r| {
        err(
            r.code.as_str(),
            guard::with_position(&r.message, r.position, guard::leading_chars(original)),
        )
    })
}

/// `check_model_sql`, когда разбор не нужен.
fn check_sql(original: &str, purpose: Purpose) -> Result<(), ToolError> {
    check_model_sql(original, purpose).map(|_| ())
}

/// Текст pgss по `queryId` — те же проверки, что у SQL модели; слишком длинный называется хранимым текстом запроса:
/// модель прислала только `queryId`, не SQL. Синтаксическая ошибка разбора — с подсказкой `pgss_syntax_hint` (E5).
pub(crate) fn check_pgss_text(query_id: i64, text: &str) -> Result<(), ToolError> {
    if normalize(text).len() > MAX_SQL_BYTES {
        return Err(invalid_argument(format!(
            "The text of query {query_id} stored in pg_stat_statements is longer than {MAX_SQL_BYTES} bytes: the gateway does not plan it"
        )));
    }
    check_sql(text, Purpose::Explain).map_err(|e| {
        // Ошибка грамматики `pg_query` — текст парсера PostgreSQL («syntax error at or near …», «… at end of input»),
        // как у SQLSTATE 42601 сервера; прочие отказы `sqlcheck` (вид выражения, функции) так не начинаются.
        if e.code == "invalid_sql" && e.message.starts_with("syntax error") {
            err(
                "invalid_sql",
                guard::with_hint(&e.message, &pgss_syntax_hint(query_id)),
            )
        } else {
            e
        }
    })
}

/// Подсказка к синтаксической ошибке текста pgss (E5): pgss хранит нормализованный текст, где константы типизированных
/// литералов (`interval '1 day'`, `timestamptz '…'`) стали `$N`, и такой текст не разбирается ни шлюзом, ни сервером.
/// Частые типы шлюз приводит сам (`cast_typed_literal_params`, смоук P4b, G5); подсказка — для оставшихся.
pub(crate) fn pgss_syntax_hint(query_id: i64) -> String {
    format!(
        "pg_stat_statements stores query {query_id} in normalized form: its constants became $N placeholders, and some of them need a type that the text does not give (typed literals such as point '(1,2)'). Pass sql instead of queryId, written with $N::type casts (for example $1::date) or with real values in place of such $N."
    )
}

/// Строки `indexes` у `hypothetical_index` до соединения: число (1–`MAX_HYPO_INDEXES`), имя индекса снимается
/// (`sqlcheck::without_index_name`, E4: модели называют индексы, hypopg даёт свои имена), затем `sqlcheck`
/// (`HypoIndex`) и правило P3b `validate_ddl` (без `;`, комментариев, `$` и обратной косой вне кавычек). Итог —
/// проверенные строки: они и идут в hypopg и в ответ (`indexes[].ddl`). Диалог P3b имя не снимает: там правило прежнее.
pub(crate) fn check_index_ddls(ddls: &[String]) -> Result<Vec<String>, ToolError> {
    if ddls.is_empty() || ddls.len() > MAX_HYPO_INDEXES {
        return Err(invalid_argument(format!(
            "Pass from 1 to {MAX_HYPO_INDEXES} statements in indexes"
        )));
    }
    ddls.iter()
        .map(|ddl| {
            // Длинный текст не разбирается лишний раз: его отвергнет `check_sql`.
            let ddl = (ddl.len() <= MAX_SQL_BYTES)
                .then(|| sqlcheck::without_index_name(ddl))
                .flatten()
                .unwrap_or_else(|| ddl.clone());
            check_sql(&ddl, Purpose::HypoIndex)?;
            hypopg::validate_ddl(&ddl).map_err(|e| err("invalid_sql", e.message))?;
            Ok(ddl)
        })
        .collect()
}

/// Откуда SQL: прислала модель или текст pgss по `queryId` — план по нему не «SQL модели» и маскируется (§6.1);
/// синтаксическая ошибка сервера в тексте pgss получает подсказку (`syntax_hint`, E5).
pub(crate) struct SqlSource {
    pub(crate) text: String,
    pub(crate) from_pgss: bool,
    pub(crate) syntax_hint: Option<String>,
}

/// Что прислала модель: ровно одно из `sql` и `queryId`; `sql` проверен `sqlcheck`, `queryId` — число. До соединения.
pub(crate) enum Asked<'a> {
    Sql(&'a str),
    QueryId(i64),
}

/// Правило и сообщения — как у `validate` крейта (пустая строка — «не задана»): через шлюз сюда приходит уже
/// нормализованная пара, а при прямом вызове `read_op` (тесты, будущие вызовы приложения) проверка остаётся страховкой.
pub(crate) fn asked<'a>(
    sql: Option<&'a str>,
    query_id: Option<&str>,
) -> Result<Asked<'a>, ToolError> {
    let sql = sql.filter(|s| !s.trim().is_empty());
    let query_id = query_id.filter(|q| !q.trim().is_empty());
    match (sql, query_id) {
        (Some(s), None) => {
            check_sql(s, Purpose::Explain)?;
            Ok(Asked::Sql(s))
        }
        (None, Some(q)) => Ok(Asked::QueryId(
            query_detail::parse_query_id(q).map_err(tool_error)?,
        )),
        (Some(_), Some(_)) => Err(invalid_argument(SQL_AND_QUERY_ID_BOTH)),
        (None, None) => Err(invalid_argument(SQL_AND_QUERY_ID_NEITHER)),
    }
}

fn pgss_text_sql(ns: &str) -> String {
    format!(
        "{SQL_MARKER}SELECT s.query FROM {ns}.pg_stat_statements(true) s JOIN pg_database d ON d.oid = s.dbid \
         WHERE s.queryid = $1 AND d.datname = $2 AND s.query IS NOT NULL LIMIT 1"
    )
}

/// Текст pgss запроса `query_id` в базе `database` на узле `member` (пул каталога узла: только чтение).
async fn pgss_text(
    st: &AppState,
    id: &str,
    member: &str,
    database: &str,
    query_id: i64,
) -> Result<String, ToolError> {
    let target = st
        .runtimes
        .diag_target(id, Some(member), false)
        .map_err(tool_error)?;
    let db = database.to_string();
    let text = on_member(st, id, &target, |c| {
        Box::pin(async move {
            let pgss = match detect(c, None).await? {
                Detect::Installed(p) => p,
                Detect::TooOld(v) => return Err(pgss_too_old(&v)),
                Detect::Missing => return Err(pgss_missing()),
            };
            let row = c
                .query_opt(&pgss_text_sql(&pgss.schema), &[&query_id, &db])
                .await
                .map_err(|e| from_pg(&e))?;
            Ok(row.and_then(|r| r.get::<_, Option<String>>(0)))
        })
    })
    .await
    .map_err(tool_error)?;
    match text {
        Some(t) if t == HIDDEN_PGSS_TEXT => Err(err(
            "NOT_AUTHORIZED",
            format!(
                "The text of query {query_id} is hidden from the connection role: it needs pg_read_all_stats"
            ),
        )),
        Some(t) => Ok(t),
        None => Err(err(
            "not_found",
            format!(
                "Query {query_id} is not in pg_stat_statements of database {database} on {member}"
            ),
        )),
    }
}

/// Текст для плана: SQL модели как есть или текст pgss (он тоже проходит `sqlcheck`: в pgss бывают и служебные
/// команды). В тексте pgss типизированные литералы частых типов (`date $1`) сначала становятся приведениями (`$1::date`,
/// `cast_typed_literal_params`, смоук P4b, G5): иначе текст не разбирается.
pub(crate) async fn sql_source(
    st: &AppState,
    id: &str,
    member: &str,
    database: &str,
    asked: Asked<'_>,
) -> Result<SqlSource, ToolError> {
    match asked {
        Asked::Sql(s) => Ok(SqlSource {
            text: s.to_string(),
            from_pgss: false,
            syntax_hint: None,
        }),
        Asked::QueryId(q) => {
            let text = cast_typed_literal_params(&pgss_text(st, id, member, database, q).await?);
            check_pgss_text(q, &text)?;
            Ok(SqlSource {
                text,
                from_pgss: true,
                syntax_hint: Some(pgss_syntax_hint(q)),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::sample_profile;

    #[test]
    fn omitted_or_blank_database_falls_back_to_the_profile_database() {
        let mut p = sample_profile("on");
        p.pg.database = "shop".into();
        assert_eq!(resolve_database(&p, None), "shop");
        assert_eq!(resolve_database(&p, Some("")), "shop");
        assert_eq!(resolve_database(&p, Some("  ")), "shop");
        assert_eq!(resolve_database(&p, Some("other")), "other");
        assert_eq!(resolve_database(&p, Some(" other ")), "other");
    }

    #[test]
    fn model_sql_is_checked_before_the_server() {
        assert_eq!(
            check_sql("SELECT pg_sleep(1)", Purpose::Explain)
                .err()
                .unwrap()
                .code,
            "forbidden_function"
        );
        assert_eq!(
            check_sql("SELECT 1; DELETE FROM t", Purpose::Explain)
                .err()
                .unwrap()
                .code,
            "invalid_sql"
        );
        assert!(check_sql("DELETE FROM t WHERE id = 1", Purpose::Explain).is_ok());
        assert!(check_sql("CREATE INDEX ON t (a)", Purpose::HypoIndex).is_ok());
        let long = format!("SELECT {}", "1,".repeat(MAX_SQL_BYTES));
        assert_eq!(
            check_sql(&long, Purpose::Explain).err().unwrap().code,
            "invalid_argument"
        );
        assert!(crate::product::is_marked_sql(&pgss_text_sql("\"public\"")));
    }

    #[test]
    fn hypothesis_ddls_are_checked_before_the_server() {
        let ddls = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            check_index_ddls(&ddls(&["CREATE INDEX ON t (a)", "CREATE INDEX ON t (b);"])).ok(),
            Some(ddls(&["CREATE INDEX ON t (a)", "CREATE INDEX ON t (b);"]))
        );
        assert_eq!(check_index_ddls(&[]).unwrap_err().code, "invalid_argument");
        assert_eq!(
            check_index_ddls(&ddls(&["CREATE INDEX ON t (a)"; 6]))
                .unwrap_err()
                .code,
            "invalid_argument"
        );
        for (ddl, code) in [
            ("CREATE UNIQUE INDEX ON t (a)", "invalid_sql"),
            ("CREATE INDEX CONCURRENTLY ON t (a)", "invalid_sql"),
            ("CREATE INDEX ON t (a); DROP TABLE t", "invalid_sql"),
            ("CREATE INDEX ON t ((pg_sleep(1)))", "forbidden_function"),
            // `sqlcheck` пропускает комментарий, правило P3b — нет.
            ("CREATE INDEX ON t (a) /* x */", "invalid_sql"),
            // E4: имя снимается, но прочие отказы — прежние.
            ("CREATE INDEX idx ON t (a) /* x */", "invalid_sql"),
            ("CREATE INDEX s.idx ON t (a)", "invalid_sql"),
            ("CREATE UNIQUE INDEX idx ON t (a)", "invalid_sql"),
            ("CREATE INDEX CONCURRENTLY idx ON t (a)", "invalid_sql"),
            (
                "CREATE INDEX idx ON t ((a), (b)); CREATE INDEX ON t (c)",
                "invalid_sql",
            ),
            ("CREATE INDEX idx ON t (a) WHERE b = $1", "invalid_sql"),
        ] {
            assert_eq!(
                check_index_ddls(&ddls(&[ddl])).unwrap_err().code,
                code,
                "{ddl}"
            );
        }
    }

    /// E4: модель называет индекс гипотезы — имя (и `IF NOT EXISTS`) снимается до проверок; в hypopg и в ответ идёт
    /// проверенный текст без имени.
    #[test]
    fn hypothesis_index_names_are_dropped() {
        let one = |ddl: &str| check_index_ddls(&[ddl.to_string()]).map(|v| v[0].clone());
        for (ddl, want) in [
            (
                "CREATE INDEX idx_orders_status ON shop.orders (status)",
                "CREATE INDEX ON shop.orders (status)",
            ),
            (
                "CREATE INDEX \"Orders By Status\" ON shop.orders (status);",
                "CREATE INDEX ON shop.orders (status);",
            ),
            (
                "create index if not exists idx on shop.orders using btree (status) where status <> 'done'",
                "create index on shop.orders using btree (status) where status <> 'done'",
            ),
        ] {
            match one(ddl) {
                Ok(got) => assert_eq!(got, want, "{ddl}"),
                Err(e) => panic!("{ddl}: {}: {}", e.code, e.message),
            }
        }
        let e = one("CREATE UNIQUE INDEX idx ON t (a)").unwrap_err();
        assert!(
            e.message.starts_with("UNIQUE is not supported"),
            "{}",
            e.message
        );
        let e = one("CREATE INDEX CONCURRENTLY idx ON t (a)").unwrap_err();
        assert!(
            e.message.starts_with("CONCURRENTLY is not supported"),
            "{}",
            e.message
        );
        let e = one("CREATE INDEX s.idx ON t (a)").unwrap_err();
        assert!(e.message.starts_with("syntax error"), "{}", e.message);
        // Правило P3b (`$`, комментарии) — к тексту без имени: `$` в имени вырезается вместе с ним.
        assert_eq!(
            one("CREATE INDEX idx$1 ON t (a)").ok().as_deref(),
            Some("CREATE INDEX ON t (a)")
        );
        assert_eq!(
            one("CREATE INDEX \"x\"ON t (a)").ok().as_deref(),
            Some("CREATE INDEX ON t (a)")
        );
        assert_eq!(
            one("/* c */ CREATE INDEX idx ON t (a)").unwrap_err().code,
            "invalid_sql"
        );
    }

    /// E5: синтаксическая ошибка текста pgss (`queryId`) — с подсказкой после текста и позиции; у SQL модели и у прочих
    /// отказов — без неё.
    #[test]
    fn syntax_errors_of_a_pgss_text_carry_the_normalized_text_hint() {
        let hint = pgss_syntax_hint(42);
        assert_eq!(
            hint,
            "pg_stat_statements stores query 42 in normalized form: its constants became $N placeholders, and some of them need a type that the text does not give (typed literals such as point '(1,2)'). Pass sql instead of queryId, written with $N::type casts (for example $1::date) or with real values in place of such $N."
        );
        assert_eq!(
            guard::with_hint("syntax error at or near \"$1\" (position 118)", &hint),
            format!("syntax error at or near \"$1\" (position 118). {hint}")
        );
        let e = check_pgss_text(42, "SELECT * FROM t WHERE a = ").unwrap_err();
        assert_eq!(e.code, "invalid_sql");
        assert_eq!(
            e.message,
            format!("syntax error at end of input. {hint}"),
            "текст разбора — как сейчас, подсказка — после"
        );
        let e = check_sql("SELECT * FROM t WHERE a = ", Purpose::Explain).unwrap_err();
        assert_eq!(
            e.message, "syntax error at end of input",
            "SQL модели — без подсказки"
        );
        for text in ["SELECT pg_sleep($1)", "EXPLAIN SELECT 1"] {
            let e = check_pgss_text(42, text).unwrap_err();
            assert!(
                !e.message.contains("normalized form"),
                "{text}: {}",
                e.message
            );
        }
    }

    #[test]
    fn sql_or_query_id_is_checked_before_the_server() {
        assert!(matches!(
            asked(Some("SELECT 1"), None),
            Ok(Asked::Sql("SELECT 1"))
        ));
        assert!(matches!(asked(None, Some(" 42 ")), Ok(Asked::QueryId(42))));
        for (sql, qid, code) in [
            (Some("SELECT 1"), Some("42"), "invalid_argument"),
            (None, None, "invalid_argument"),
            (None, Some("x"), "invalid_argument"),
            (
                Some("SELECT pg_read_file('/etc/passwd')"),
                None,
                "forbidden_function",
            ),
            (Some("EXPLAIN SELECT 1"), None, "invalid_sql"),
            (Some("SELECT 1 INTO t"), None, "invalid_sql"),
        ] {
            let e = asked(sql, qid).err().unwrap();
            assert_eq!(e.code, code, "{sql:?} {qid:?}");
        }
        // Те же правило и сообщения, что у `validate` крейта (E2): пустая строка — «не задана».
        assert!(matches!(
            asked(Some("SELECT 1"), Some("")),
            Ok(Asked::Sql("SELECT 1"))
        ));
        assert!(matches!(asked(Some(" "), Some("7")), Ok(Asked::QueryId(7))));
        for (sql, qid, want) in [
            (Some("SELECT 1"), Some("42"), SQL_AND_QUERY_ID_BOTH),
            (None, None, SQL_AND_QUERY_ID_NEITHER),
            (Some(""), Some(" "), SQL_AND_QUERY_ID_NEITHER),
        ] {
            let e = asked(sql, qid).err().unwrap();
            assert_eq!(e.message, want, "{sql:?} {qid:?}");
        }
    }

    /// Позиция отказа `sqlcheck` — в тексте, как его прислала модель: ведущие пробелы и переводы строк, которые снимает
    /// `normalize`, в счёте есть (сервер и `sqlcheck` видят нормализованный текст).
    #[test]
    fn rejection_positions_refer_to_the_text_the_model_sent() {
        // « \n  » — 4 символа, «SELECT » — 7: имя функции — 12-й символ.
        let e = asked(Some(" \n  SELECT pg_terminate_backend(1);"), None)
            .err()
            .unwrap();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "forbidden_function",
                "Function pg_terminate_backend is not allowed (position 12)"
            )
        );
        let e = check_sql("  SELECT 1; SELECT 2", Purpose::Explain).unwrap_err();
        assert!(e.message.ends_with("(position 13)"), "{}", e.message);
        // Без ведущих пробелов позиция та же, что у `sqlcheck`.
        let e = check_sql("SELECT 1; SELECT 2", Purpose::Explain).unwrap_err();
        assert!(e.message.ends_with("(position 11)"), "{}", e.message);
        // DDL гипотезы: «  CREATE INDEX ON t ((» — 22 символа.
        let e = check_index_ddls(&["  CREATE INDEX ON t ((pg_sleep(1)))".to_string()]).unwrap_err();
        assert!(e.message.ends_with("(position 23)"), "{}", e.message);
    }

    /// Слишком длинный текст pgss по `queryId`: модель прислала только id — сообщение называет хранимый текст запроса.
    #[test]
    fn too_long_pgss_text_is_named_as_the_stored_text() {
        let long = format!("SELECT {}", "1,".repeat(MAX_SQL_BYTES));
        let e = check_pgss_text(42, &long).unwrap_err();
        assert_eq!(e.code, "invalid_argument");
        assert!(
            e.message.contains("query 42")
                && e.message.contains("pg_stat_statements")
                && !e.message.starts_with("The SQL is longer"),
            "{}",
            e.message
        );
        assert!(check_pgss_text(42, "SELECT * FROM t WHERE a = $1").is_ok());
        assert_eq!(
            check_pgss_text(42, "SELECT pg_sleep($1)").unwrap_err().code,
            "forbidden_function"
        );
    }
}
