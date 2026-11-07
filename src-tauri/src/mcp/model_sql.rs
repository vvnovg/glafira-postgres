//! SQL модели (спец. P4 §7.1) до соединения и источник текста запроса: длина и `sqlcheck` с позицией отказа в тексте,
//! как его прислала модель (§16.17), текст pgss по `queryId` (те же проверки, подсказка к синтаксической ошибке
//! нормализованного текста — E5), строки гипотетических индексов (имя снимается — E4), «ровно один из `sql` и
//! `queryId`», база вызова по умолчанию (§16.21). Общие для инструментов P4a (`mcp::read`), `run_select` и
//! предложений P4b; перенесены из `read.rs` без изменения поведения.

use glafira_mcp_core::error::ToolError;
use glafira_pg_mcp::params::{SQL_AND_QUERY_ID_BOTH, SQL_AND_QUERY_ID_NEITHER};
use pg_advisor::sqlcheck::{self, Checked, Purpose};
use pg_advisor::sqltext::{apply_typed_literal_casts, typed_literal_casts};

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

/// Длина текста pgss по `queryId`: слишком длинный называется хранимым текстом запроса — модель прислала только
/// `queryId`, не SQL.
fn check_pgss_len(query_id: i64, text: &str) -> Result<(), ToolError> {
    if normalize(text).len() > MAX_SQL_BYTES {
        return Err(invalid_argument(format!(
            "The text of query {query_id} stored in pg_stat_statements is longer than {MAX_SQL_BYTES} bytes: the gateway does not plan it"
        )));
    }
    Ok(())
}

/// Текст pgss по `queryId` — те же проверки, что у SQL модели (длина — `check_pgss_len`). Синтаксическая ошибка
/// разбора — с подсказкой `pgss_syntax_hint` (E5).
pub(crate) fn check_pgss_text(query_id: i64, text: &str) -> Result<(), ToolError> {
    check_pgss_len(query_id, text)?;
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
    pub(crate) plan: PlanText,
    pub(crate) from_pgss: bool,
    pub(crate) syntax_hint: Option<String>,
}

/// Текст для плана (смоук P4b, G5; ревью правки B, #2) — SQL модели, текст pgss по `queryId`, снимок запроса на
/// оптимизацию и предложенный SQL: типизированные литералы частых типов (`date $1`) становятся приведениями `$1::date`
/// (`pg_advisor::sqltext::typed_literal_casts`). `<тип> $N` — не SQL (сервер его не разбирает), так что меняются только
/// тексты, которые иначе не построили бы план. Позиции в ошибках разбора и плана — по тексту приведений; `restore`
/// пересчитывает их в исходный текст (§16.17). Текст длиннее `MAX_SQL_BYTES` на лексемы не разбирается (ревью правки
/// B, #1): его отвергнет проверка длины.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlanText {
    pub(crate) text: String,
    /// Приведения по порядку, в символах исходного текста: начало куска `<тип> $N` (с 0), его длина и длина приведения.
    casts: Vec<(usize, usize, usize)>,
}

impl PlanText {
    pub(crate) fn new(sql: &str) -> Self {
        let found = if normalize(sql).len() > MAX_SQL_BYTES {
            Vec::new()
        } else {
            typed_literal_casts(sql)
        };
        if found.is_empty() {
            return Self {
                text: sql.to_string(),
                casts: Vec::new(),
            };
        }
        let chars = |a: usize, b: usize| sql[a..b].chars().count();
        let (mut byte, mut char) = (0, 0);
        let casts = found
            .iter()
            .map(|c| {
                char += chars(byte, c.type_start);
                byte = c.type_start;
                let cast = chars(c.param_start, c.param_end) + 2 + chars(c.type_start, c.type_end);
                (char, chars(c.type_start, c.param_end), cast)
            })
            .collect();
        Self {
            text: apply_typed_literal_casts(sql, &found),
            casts,
        }
    }

    /// Позиция в тексте приведений (символы с 1) → позиция в исходном тексте; внутри приведения — начало его типа.
    fn original_position(&self, p: usize) -> usize {
        let q = p.saturating_sub(1);
        // Насколько текст приведений длиннее исходного до текущего куска.
        let mut shift = 0isize;
        for &(start, piece, cast) in &self.casts {
            let at = start.saturating_add_signed(shift);
            if q < at {
                break;
            }
            if q < at + cast {
                return start + 1;
            }
            shift += cast as isize - piece as isize;
        }
        q.saturating_add_signed(-shift) + 1
    }

    /// Ошибка с позициями «(position N)» (так их пишут `guard::with_position` и `sql_error`) в тексте приведений — с
    /// позициями в исходном тексте. Без приведений — как есть.
    pub(crate) fn restore(&self, mut e: ToolError) -> ToolError {
        const KEY: &str = "(position ";
        if self.casts.is_empty() || !e.message.contains(KEY) {
            return e;
        }
        let mut out = String::with_capacity(e.message.len());
        let mut rest = e.message.as_str();
        while let Some(i) = rest.find(KEY) {
            let (head, tail) = rest.split_at(i + KEY.len());
            out.push_str(head);
            let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
            match tail[..digits].parse::<usize>() {
                Ok(p) if tail[digits..].starts_with(')') => {
                    out.push_str(&self.original_position(p).to_string())
                }
                _ => out.push_str(&tail[..digits]),
            }
            rest = &tail[digits..];
        }
        out.push_str(rest);
        e.message = out;
        e
    }
}

/// Текст pgss по `queryId` для плана: длина — до разбора на лексемы (ревью правки B, #1: текст pgss бывает в мегабайты,
/// например длинный список `IN`), затем приведения (`PlanText`) и проверки `check_pgss_text` по тексту приведений
/// (позиции — в тексте pgss).
fn pgss_plan_text(query_id: i64, raw: &str) -> Result<PlanText, ToolError> {
    check_pgss_len(query_id, raw)?;
    let plan = PlanText::new(raw);
    check_pgss_text(query_id, &plan.text).map_err(|e| plan.restore(e))?;
    Ok(plan)
}

/// Что прислала модель: ровно одно из `sql` и `queryId`; `sql` — текст для плана (`PlanText`), проверен `sqlcheck`,
/// `queryId` — число. До соединения.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Asked {
    Sql(PlanText),
    QueryId(i64),
}

/// Правило и сообщения — как у `validate` крейта (пустая строка — «не задана»): через шлюз сюда приходит уже
/// нормализованная пара, а при прямом вызове `read_op` (тесты, будущие вызовы приложения) проверка остаётся страховкой.
pub(crate) fn asked(sql: Option<&str>, query_id: Option<&str>) -> Result<Asked, ToolError> {
    let sql = sql.filter(|s| !s.trim().is_empty());
    let query_id = query_id.filter(|q| !q.trim().is_empty());
    match (sql, query_id) {
        (Some(s), None) => {
            let plan = PlanText::new(s);
            check_sql(&plan.text, Purpose::Explain).map_err(|e| plan.restore(e))?;
            Ok(Asked::Sql(plan))
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

/// Текст для плана: SQL модели (уже `PlanText`) или текст pgss (он тоже проходит `sqlcheck`: в pgss бывают и служебные
/// команды) — в обоих типизированные литералы частых типов (`date $1`) становятся приведениями (`$1::date`, `PlanText`):
/// иначе сервер текст не разбирает.
pub(crate) async fn sql_source(
    st: &AppState,
    id: &str,
    member: &str,
    database: &str,
    asked: Asked,
) -> Result<SqlSource, ToolError> {
    match asked {
        Asked::Sql(plan) => Ok(SqlSource {
            plan,
            from_pgss: false,
            syntax_hint: None,
        }),
        Asked::QueryId(q) => Ok(SqlSource {
            plan: pgss_plan_text(q, &pgss_text(st, id, member, database, q).await?)?,
            from_pgss: true,
            syntax_hint: Some(pgss_syntax_hint(q)),
        }),
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
        assert_eq!(
            asked(Some("SELECT 1"), None),
            Ok(Asked::Sql(PlanText::new("SELECT 1")))
        );
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
        assert_eq!(
            asked(Some("SELECT 1"), Some("")),
            Ok(Asked::Sql(PlanText::new("SELECT 1")))
        );
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

    /// Ревью правки B, #2: `<тип> $N` — не SQL, и в SQL модели он, как в тексте pgss, становится приведением `$N::тип`;
    /// текст без таких литералов не меняется. Позиции в тексте приведений (символы с 1) пересчитываются в текст модели:
    /// до приведения — те же, внутри — начало типа, после — со сдвигом (приведение длиннее куска на 2 символа минус
    /// пробелы между типом и параметром).
    #[test]
    fn typed_literals_of_any_sql_become_casts_and_positions_come_back() {
        let original =
            "SELECT * FROM t WHERE e < timestamptz  $2 AND d = date $1 AND x = int\n $3 AND nope";
        let p = PlanText::new(original);
        assert_eq!(
            p.text,
            "SELECT * FROM t WHERE e < $2::timestamptz AND d = $1::date AND x = $3::int AND nope"
        );
        let at = |text: &str, part: &str| text.find(part).unwrap() + 1;
        for (cast, want) in [
            (1, 1),
            (at(&p.text, "$2::"), at(original, "timestamptz")),
            (at(&p.text, " AND d"), at(original, " AND d")),
            (at(&p.text, "$1::"), at(original, "date $1")),
            (at(&p.text, "date AND"), at(original, "date $1")),
            (at(&p.text, " AND x"), at(original, " AND x")),
            (at(&p.text, "$3::"), at(original, "int\n")),
            (at(&p.text, "nope"), at(original, "nope")),
        ] {
            assert_eq!(p.original_position(cast), want, "{cast}");
        }
        let e = p.restore(err(
            "invalid_sql",
            format!(
                "column \"nope\" does not exist (position {}) (positions) (position 1)",
                at(&p.text, "nope")
            ),
        ));
        assert_eq!(
            e.message,
            format!(
                "column \"nope\" does not exist (position {}) (positions) (position 1)",
                at(original, "nope")
            )
        );
        let plain = PlanText::new("SELECT $1::date FROM t");
        assert_eq!(plain.text, "SELECT $1::date FROM t");
        let e = err("invalid_sql", "x (position 9)");
        assert_eq!(plain.restore(e.clone()), e);
    }

    /// SQL модели с `date $1` уходит серверу приведённым (`pg_query` разбирает и `date $1`, сервер — нет); отказ
    /// `sqlcheck` по тексту приведений — с позицией в тексте модели.
    #[test]
    fn model_sql_with_typed_literals_is_checked_as_casts() {
        let Ok(Asked::Sql(p)) = asked(Some(" SELECT * FROM t WHERE d > date $1"), None) else {
            panic!("date $1 не принят");
        };
        assert_eq!(p.text, " SELECT * FROM t WHERE d > $1::date");
        let sql = " SELECT * FROM t WHERE d > date $1 AND pg_sleep(1) IS NULL";
        let e = asked(Some(sql), None).unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message),
            (
                "forbidden_function",
                format!(
                    "Function pg_sleep is not allowed (position {})",
                    sql.find("pg_sleep").unwrap() + 1
                )
            )
        );
    }

    /// Ревью правки B, #1: длина текста — до разбора на лексемы: длинный текст pgss отвергается сразу, длинный SQL модели
    /// не приводится (его отвергнет проверка длины); короткий текст pgss приводится и проверяется `check_pgss_text`.
    #[test]
    fn long_texts_are_not_tokenized_and_pgss_positions_come_back() {
        let long = format!("SELECT date $1, {}1", "1,".repeat(MAX_SQL_BYTES / 2));
        assert_eq!(PlanText::new(&long).text, long, "длинный текст — как есть");
        let e = pgss_plan_text(7, &long).unwrap_err();
        assert_eq!(e.code, "invalid_argument");
        assert!(
            e.message
                .contains("stored in pg_stat_statements is longer than"),
            "{}",
            e.message
        );
        let p = pgss_plan_text(7, "SELECT * FROM t WHERE d > date $1").unwrap();
        assert_eq!(p.text, "SELECT * FROM t WHERE d > $1::date");
        // `pg_query` разбирает `date $1` (грамматика libpg_query для нормализованных текстов) — отвергает сервер; здесь
        // ошибка грамматики после приведения: подсказка E5 остаётся.
        let e = pgss_plan_text(7, "SELECT * FROM t WHERE d > date $1 AND AND x").unwrap_err();
        assert_eq!(e.code, "invalid_sql");
        assert!(
            e.message
                .starts_with("syntax error at or near \"AND\". pg_stat_statements stores query 7"),
            "{}",
            e.message
        );
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
