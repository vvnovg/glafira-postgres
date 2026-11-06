//! Проверка SQL от модели по дереву `pg_query` (спец. P4 §7.1). Назначение (`Explain`, `HypoIndex`, `Select`) задаёт
//! допустимый вид выражения; функции из `functions::DENIED_FUNCTIONS` отвергаются на любой глубине дерева.
//!
//! Обход — по serde-сериализации protobuf-дерева, а не `ParseResult::nodes()`: тот обходит лишь часть узлов (нет
//! `VALUES`, `LIMIT`, `FILTER`, окон, именованных аргументов, `XMLTABLE`…), и вызов там остался бы невидимым. В JSON
//! видны все поля всех узлов. Вызов узнаётся по форме объекта (`funcname` + `agg_star` — `FuncCall`, в том числе без
//! обёртки узла, как у `CALL`), таблица — по `relname` + `relpersistence` (`RangeVar`, в том числе цель
//! `INSERT` / `UPDATE` / `DELETE` / `MERGE` / `CREATE INDEX`), `INTO` и блокировки строк — по полям `into_clause` и
//! `locking_clause` (ветви `UNION` лежат без обёртки узла). Глубину ограничивает prost (100 вложенных сообщений):
//! глубже — ошибка разбора, `invalid_sql`. Сообщения — для модели, по-английски; позиция — номер символа с 1, как у
//! ошибок сервера.

use std::collections::HashSet;

use pg_query::NodeEnum;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::functions::{is_denied, is_denied_qualified};

/// Для чего проверяется SQL: от назначения зависит, какое выражение допустимо.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// `explain_query`, запрос `hypothetical_index`, предложение (P4b): одно `SELECT` / `INSERT` / `UPDATE` / `DELETE` /
    /// `MERGE` — без ANALYZE оно не выполняется.
    Explain,
    /// Строка `indexes` у `hypothetical_index`: одно `CREATE INDEX ON …` без имени, `CONCURRENTLY` и `UNIQUE` (P3b);
    /// имя, присланное моделью, шлюз снимает до проверки (`without_index_name`).
    HypoIndex,
    /// `run_select` (P4b): одно `SELECT` без `INTO`, блокировок строк и изменяющих CTE на любой глубине.
    Select,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StatementKind {
    Select,
    Insert,
    Update,
    Delete,
    Merge,
    CreateIndex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FuncRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub name: String,
}

/// Принятое выражение: вид, вызванные функции и таблицы — в порядке текста, без повторов; имена CTE — не таблицы.
/// Функции P4b сверяет с `pg_proc` (`user_function`, спец. §7.3, шаг 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checked {
    pub kind: StatementKind,
    pub functions: Vec<FuncRef>,
    pub tables: Vec<TableRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectCode {
    InvalidSql,
    ForbiddenFunction,
}

impl RejectCode {
    /// Код ошибки инструмента (спец. §10.2).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidSql => "invalid_sql",
            Self::ForbiddenFunction => "forbidden_function",
        }
    }
}

/// Отказ: сообщение для модели (по-английски) и позиция в тексте (номер символа с 1), если она известна.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub code: RejectCode,
    pub message: String,
    pub position: Option<usize>,
}

fn invalid(message: impl Into<String>, position: Option<usize>) -> Rejection {
    Rejection {
        code: RejectCode::InvalidSql,
        message: message.into(),
        position,
    }
}

/// Число выражений в тексте по разбору `pg_query` (грамматика PostgreSQL 18); ошибка — текст `pg_query`.
///
/// Вызывает `pg_query::parse`: в отладочной сборке ему нужен стек потока не меньше 2 МиБ (рабочий поток tokio и
/// `spawn_blocking` подходят); из небольших собственных потоков не вызывать — возможен `stack overflow` с аварийным
/// завершением процесса.
pub fn parse_count(sql: &str) -> Result<usize, String> {
    pg_query::parse(sql, pg_query::ParserOptions::DEFAULT)
        .map(|r| r.protobuf.stmts.len())
        .map_err(|e| e.to_string())
}

/// Текст гипотетического индекса без имени (P4a, E4): модели пишут `CREATE INDEX idx_x ON …`, а hypopg даёт индексу
/// своё имя. Одно `CREATE INDEX [IF NOT EXISTS] имя ON …` без `UNIQUE` и `CONCURRENTLY` (по дереву разбора) → тот же
/// текст без `[IF NOT EXISTS] имя`: вырезается по токенам `pg_query::scan` от конца `INDEX` до конца имени (имя — один
/// токен перед `ON`, в кавычках или без; `IF NOT EXISTS` без имени PostgreSQL не принимает). Иначе `None` — текст
/// проверяется как есть, и отказы `check` прежние: имя со схемой не разбирается, `UNIQUE`, `CONCURRENTLY`, комментарий
/// между `INDEX` и `ON`, не `CREATE INDEX`. `check` имя по-прежнему отвергает: снимает его вызывающий (шлюз MCP).
///
/// Вызывает `pg_query::parse` — требования к стеку те же, что у [`check`].
pub fn without_index_name(ddl: &str) -> Option<String> {
    use pg_query::protobuf::Token;
    let parsed = pg_query::parse(ddl, pg_query::ParserOptions::DEFAULT).ok()?;
    let [stmt] = parsed.protobuf.stmts.as_slice() else {
        return None;
    };
    let Some(NodeEnum::IndexStmt(ix)) = stmt.stmt.as_ref().and_then(|s| s.node.as_ref()) else {
        return None;
    };
    if ix.idxname.is_empty() || ix.unique || ix.concurrent {
        return None;
    }
    let tokens = pg_query::scan(ddl).ok()?.tokens;
    let is = |i: usize, t: Token| tokens.get(i).is_some_and(|x| x.token == t as i32);
    let name = if ix.if_not_exists { 5 } else { 2 };
    let header = is(0, Token::Create)
        && is(1, Token::Index)
        && (!ix.if_not_exists || (is(2, Token::IfP) && is(3, Token::Not) && is(4, Token::Exists)))
        && !is(name, Token::SqlComment)
        && !is(name, Token::CComment)
        && is(name + 1, Token::On);
    if !header {
        return None;
    }
    let from = usize::try_from(tokens[1].end).ok()?;
    let to = usize::try_from(tokens[name].end).ok()?;
    Some(format!("{}{}", ddl.get(..from)?, ddl.get(to..)?))
}

/// Байтовое смещение `pg_query` (`location`; −1 — неизвестно) → номер символа с 1.
fn char_position(sql: &str, location: i64) -> Option<usize> {
    let at = usize::try_from(location).ok()?;
    sql.get(..at).map(|s| s.chars().count() + 1)
}

/// Начало выражения по `stmt_location`: пробелы и `;` перед ним пропускаются.
fn statement_start(sql: &str, location: i32) -> Option<usize> {
    let at = usize::try_from(location).ok()?;
    let rest = sql.get(at..)?;
    let skipped = rest.len()
        - rest
            .trim_start_matches(|c: char| c.is_whitespace() || c == ';')
            .len();
    char_position(sql, i64::try_from(at + skipped).ok()?)
}

fn parse_error(e: pg_query::Error) -> Rejection {
    match e {
        // Текст парсера PostgreSQL: «syntax error at or near …»; слишком глубокий текст — «stack depth limit exceeded»
        // (проверка стека libpg_query) или «memory exhausted at or near …» (стек bison).
        pg_query::Error::Parse(message) => invalid(message, None),
        // Предел prost (100 вложенных сообщений). Текст ошибки — путь из сотни полей; модели — только причина.
        pg_query::Error::Decode(e) => {
            let text = e.to_string();
            let reason = text.rsplit(": ").next().unwrap_or(&text);
            invalid(
                format!(
                    "The statement is nested too deeply to check ({reason}): simplify nested expressions or subqueries"
                ),
                None,
            )
        }
        other => invalid(format!("The SQL text cannot be parsed: {other}"), None),
    }
}

/// Тип узла верхнего уровня → SQL-команда для сообщения; неизвестный — тип узла без `Stmt`.
fn statement_name(tag: &str) -> String {
    const NAMES: &[(&str, &str)] = &[
        ("SelectStmt", "SELECT"),
        ("InsertStmt", "INSERT"),
        ("UpdateStmt", "UPDATE"),
        ("DeleteStmt", "DELETE"),
        ("MergeStmt", "MERGE"),
        ("IndexStmt", "CREATE INDEX"),
        ("ExplainStmt", "EXPLAIN"),
        ("CreateTableAsStmt", "CREATE TABLE AS"),
        ("DeclareCursorStmt", "DECLARE"),
        ("PrepareStmt", "PREPARE"),
        ("ExecuteStmt", "EXECUTE"),
        ("VariableSetStmt", "SET"),
        ("VariableShowStmt", "SHOW"),
        ("CallStmt", "CALL"),
        ("DoStmt", "DO"),
        ("CopyStmt", "COPY"),
        ("TransactionStmt", "A transaction command"),
        ("CreateStmt", "CREATE TABLE"),
        ("DropStmt", "DROP"),
        ("TruncateStmt", "TRUNCATE"),
        ("AlterTableStmt", "ALTER TABLE"),
        ("VacuumStmt", "VACUUM / ANALYZE"),
        ("LockStmt", "LOCK"),
        ("NotifyStmt", "NOTIFY"),
        ("AlterSystemStmt", "ALTER SYSTEM"),
    ];
    NAMES.iter().find(|(t, _)| *t == tag).map_or_else(
        || tag.trim_end_matches("Stmt").to_string(),
        |(_, n)| (*n).to_string(),
    )
}

/// Что допустимо для назначения — продолжение сообщения об отказе по виду выражения.
fn expected(purpose: Purpose) -> &'static str {
    match purpose {
        Purpose::Explain => {
            "send exactly one SELECT, INSERT, UPDATE, DELETE or MERGE statement (it is only planned, never executed)"
        }
        Purpose::HypoIndex => {
            "each index must be one CREATE INDEX ON <table> (...) statement without CONCURRENTLY or UNIQUE"
        }
        Purpose::Select => "send exactly one SELECT statement",
    }
}

/// Вид выражения верхнего уровня и его допустимость для назначения; у `CREATE INDEX` гипотезы — правило P3b.
fn statement_kind(
    purpose: Purpose,
    node: Option<&NodeEnum>,
    tag: &str,
) -> Result<StatementKind, Rejection> {
    let kind = match node {
        Some(NodeEnum::SelectStmt(_)) => Some(StatementKind::Select),
        Some(NodeEnum::InsertStmt(_)) => Some(StatementKind::Insert),
        Some(NodeEnum::UpdateStmt(_)) => Some(StatementKind::Update),
        Some(NodeEnum::DeleteStmt(_)) => Some(StatementKind::Delete),
        Some(NodeEnum::MergeStmt(_)) => Some(StatementKind::Merge),
        Some(NodeEnum::IndexStmt(ix)) => {
            if purpose == Purpose::HypoIndex {
                if ix.concurrent {
                    return Err(invalid(
                        "CONCURRENTLY is not supported for hypothetical indexes: write CREATE INDEX ON <table> (...)",
                        None,
                    ));
                }
                if ix.unique {
                    return Err(invalid(
                        "UNIQUE is not supported for hypothetical indexes: write CREATE INDEX ON <table> (...)",
                        None,
                    ));
                }
                if !ix.idxname.is_empty() {
                    return Err(invalid(
                        format!(
                            "Index names are not accepted (\"{}\"): write CREATE INDEX ON <table> (...)",
                            ix.idxname
                        ),
                        None,
                    ));
                }
            }
            Some(StatementKind::CreateIndex)
        }
        _ => None,
    };
    let allowed = match purpose {
        Purpose::Explain => matches!(
            kind,
            Some(
                StatementKind::Select
                    | StatementKind::Insert
                    | StatementKind::Update
                    | StatementKind::Delete
                    | StatementKind::Merge
            )
        ),
        Purpose::HypoIndex => kind == Some(StatementKind::CreateIndex),
        Purpose::Select => kind == Some(StatementKind::Select),
    };
    match kind {
        Some(k) if allowed => Ok(k),
        _ => Err(invalid(
            format!(
                "{} is not allowed: {}",
                statement_name(tag),
                expected(purpose)
            ),
            None,
        )),
    }
}

/// Узлы изменения данных: внутри `SELECT` они возможны только в CTE.
const DML: [&str; 4] = ["InsertStmt", "UpdateStmt", "DeleteStmt", "MergeStmt"];

/// Что нашёл обход дерева. Позиции — байтовые смещения `pg_query` (−1 — неизвестно).
#[derive(Default)]
struct Found {
    calls: Vec<(i64, FuncRef)>,
    tables: Vec<(i64, TableRef)>,
    ctes: Vec<String>,
    /// Первое `SELECT … INTO`: смещение имени новой таблицы.
    into: Option<i64>,
    locking: bool,
    /// Первая CTE не на `SELECT`: имя и смещение.
    modifying_cte: Option<(String, i64)>,
    dml: usize,
}

fn location(o: &Map<String, Value>) -> i64 {
    o.get("location").and_then(Value::as_i64).unwrap_or(-1)
}

/// Строки списка узлов `String` (`funcname`, `indirection`): `[{"node": {"String": {"sval": …}}}, …]`.
fn string_parts(list: Option<&Value>) -> Vec<&str> {
    list.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|n| n.pointer("/node/String/sval").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default()
}

impl Found {
    fn visit(&mut self, o: &Map<String, Value>) {
        // FuncCall — с обёрткой узла и без неё (`CALL`).
        if o.contains_key("funcname") && o.contains_key("agg_star") {
            let parts = string_parts(o.get("funcname"));
            if let Some((name, path)) = parts.split_last() {
                let schema = path.last().map(|s| s.to_string());
                self.calls.push((
                    location(o),
                    FuncRef {
                        schema,
                        name: name.to_string(),
                    },
                ));
            }
        }
        // RangeVar — в том числе цель INSERT / UPDATE / DELETE / MERGE / CREATE INDEX (поле без обёртки узла).
        if o.contains_key("relpersistence")
            && let Some(name) = o.get("relname").and_then(Value::as_str)
        {
            let schema = o
                .get("schemaname")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            self.tables.push((
                location(o),
                TableRef {
                    schema,
                    name: name.to_string(),
                },
            ));
        }
        // Функциональная запись `(выражение).f` вызывает f: запрещённое имя в `A_Indirection` — отказ.
        if let Some(Value::Object(ind)) = o.get("AIndirection") {
            for name in string_parts(ind.get("indirection")) {
                if is_denied(name) {
                    self.calls.push((
                        -1,
                        FuncRef {
                            schema: None,
                            name: name.to_string(),
                        },
                    ));
                }
            }
        }
        if let Some(Value::Object(cte)) = o.get("CommonTableExpr") {
            let name = cte
                .get("ctename")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let select = cte
                .get("ctequery")
                .and_then(|q| q.pointer("/node/SelectStmt"))
                .is_some();
            if !select && self.modifying_cte.is_none() {
                self.modifying_cte = Some((name.clone(), location(cte)));
            }
            self.ctes.push(name);
        }
        if self.into.is_none()
            && let Some(into) = o.get("into_clause").filter(|x| !x.is_null())
        {
            self.into = Some(
                into.pointer("/rel/location")
                    .and_then(Value::as_i64)
                    .unwrap_or(-1),
            );
        }
        if o.get("locking_clause")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
            || o.contains_key("LockingClause")
        {
            self.locking = true;
        }
        self.dml += DML.iter().filter(|tag| o.contains_key(**tag)).count();
    }
}

/// Обход всего дерева (итеративный); вызовы и таблицы — в порядке текста, неизвестная позиция — в конце.
fn scan(tree: &Value) -> Found {
    let mut found = Found::default();
    let mut stack = vec![tree];
    while let Some(v) = stack.pop() {
        match v {
            Value::Array(items) => stack.extend(items),
            Value::Object(o) => {
                found.visit(o);
                stack.extend(o.values());
            }
            _ => {}
        }
    }
    let order = |at: &i64| if *at < 0 { i64::MAX } else { *at };
    found.calls.sort_by_key(|(at, _)| order(at));
    found.tables.sort_by_key(|(at, _)| order(at));
    found
}

/// Проверка SQL от модели (спец. §7.1): разбор, ровно одно выражение, вид по назначению, для `Explain` и `Select` — без
/// `SELECT … INTO`, для `Select` — без блокировок строк и изменяющих CTE на любой глубине, во всех назначениях — без
/// запрещённых функций.
///
/// Вызывает `pg_query::parse`: в отладочной сборке ему нужен стек потока не меньше 2 МиБ (рабочий поток tokio и
/// `spawn_blocking` подходят); из небольших собственных потоков не вызывать — возможен `stack overflow` с аварийным
/// завершением процесса.
pub fn check(sql: &str, purpose: Purpose) -> Result<Checked, Rejection> {
    let parsed = pg_query::parse(sql, pg_query::ParserOptions::DEFAULT).map_err(parse_error)?;
    let stmts = &parsed.protobuf.stmts;
    if stmts.is_empty() {
        return Err(invalid("The SQL text contains no statement", None));
    }
    if stmts.len() > 1 {
        return Err(invalid(
            format!("Exactly one statement is allowed, found {}", stmts.len()),
            statement_start(sql, stmts[1].stmt_location),
        ));
    }
    let tree = serde_json::to_value(&parsed.protobuf)
        .map_err(|e| invalid(format!("The parse tree cannot be read: {e}"), None))?;
    let tag = tree
        .pointer("/stmts/0/stmt/node")
        .and_then(Value::as_object)
        .and_then(|o| o.keys().next())
        .map_or("UnknownStmt", String::as_str);
    let node = stmts[0].stmt.as_ref().and_then(|s| s.node.as_ref());
    let kind = statement_kind(purpose, node, tag)?;
    let found = scan(&tree);
    if let Some(at) = found.into {
        return Err(invalid(
            "SELECT INTO creates a table and is not allowed: use a plain SELECT",
            char_position(sql, at),
        ));
    }
    if purpose == Purpose::Select {
        if found.locking {
            return Err(invalid(
                "Row locks (FOR UPDATE, FOR NO KEY UPDATE, FOR SHARE, FOR KEY SHARE) are not allowed",
                None,
            ));
        }
        if let Some((name, at)) = &found.modifying_cte {
            return Err(invalid(
                format!(
                    "WITH query \"{name}\" is not a SELECT: data-modifying WITH is not allowed"
                ),
                char_position(sql, *at),
            ));
        }
        if found.dml > 0 {
            return Err(invalid(
                "INSERT, UPDATE, DELETE and MERGE are not allowed inside SELECT",
                None,
            ));
        }
    }
    if let Some((at, f)) = found
        .calls
        .iter()
        .find(|(_, f)| is_denied_qualified(f.schema.as_deref(), &f.name))
    {
        let name = match &f.schema {
            Some(s) => format!("{s}.{}", f.name),
            None => f.name.clone(),
        };
        return Err(Rejection {
            code: RejectCode::ForbiddenFunction,
            message: format!("Function {name} is not allowed"),
            position: char_position(sql, *at),
        });
    }
    // Повторы — по множествам: длинный текст с тысячами вызовов не должен стоить O(n²).
    let mut seen = HashSet::new();
    let functions: Vec<FuncRef> = found
        .calls
        .into_iter()
        .map(|(_, f)| f)
        .filter(|f| seen.insert((f.schema.clone(), f.name.clone())))
        .collect();
    let ctes: HashSet<String> = found.ctes.into_iter().collect();
    let mut seen = HashSet::new();
    let tables: Vec<TableRef> = found
        .tables
        .into_iter()
        .map(|(_, t)| t)
        .filter(|t| !(t.schema.is_none() && ctes.contains(&t.name)))
        .filter(|t| seen.insert((t.schema.clone(), t.name.clone())))
        .collect();
    Ok(Checked {
        kind,
        functions,
        tables,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Проба сборки `pg_query`: грамматика PostgreSQL 18 считает выражения; `;` внутри строк, комментариев и долларовых
    /// кавычек выражений не делит; пустой текст и одни комментарии — ноль выражений.
    #[test]
    fn parse_count_counts_statements() {
        assert_eq!(parse_count("SELECT 1"), Ok(1));
        assert_eq!(parse_count("SELECT 1; SELECT 2;"), Ok(2));
        assert_eq!(parse_count("SELECT ';' -- ;\n/* ; */"), Ok(1));
        assert_eq!(parse_count("SELECT $$;$$, $q$ ; $q$"), Ok(1));
        assert_eq!(parse_count(""), Ok(0));
        assert_eq!(parse_count("-- только комментарий"), Ok(0));
    }

    /// Грамматика — PostgreSQL 18: числа PG 16 (основание, `_`), JSON_TABLE и MERGE … RETURNING из PG 17,
    /// RETURNING WITH (OLD AS …, NEW AS …) из PG 18.
    #[test]
    fn parse_count_knows_postgresql_18_syntax() {
        assert_eq!(parse_count("SELECT 0x1F, 0o17, 0b101, 1_000_000"), Ok(1));
        assert_eq!(
            parse_count(
                "SELECT * FROM JSON_TABLE('[{\"a\":1}]', '$[*]' COLUMNS (a int PATH '$.a')) jt"
            ),
            Ok(1)
        );
        assert_eq!(
            parse_count(
                "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN DELETE RETURNING t.*"
            ),
            Ok(1)
        );
        assert_eq!(
            parse_count("UPDATE t SET a = a + 1 RETURNING WITH (OLD AS o, NEW AS n) o.a, n.a"),
            Ok(1)
        );
    }

    #[test]
    fn parse_count_reports_errors() {
        let e = parse_count("SELEC 1").unwrap_err();
        assert!(e.contains("syntax error"), "{e}");
        assert!(parse_count("SELECT 1\0").is_err(), "NUL в тексте");
    }

    #[test]
    fn rejection_position_is_a_character_number() {
        let r = check("SELECT 'й', pg_sleep(1)", Purpose::Select).unwrap_err();
        assert_eq!(r.code, RejectCode::ForbiddenFunction);
        assert_eq!(r.position, Some(13), "символы, а не байты: «й» — два байта");
        let r = check("SELECT 1; DELETE FROM t", Purpose::Explain).unwrap_err();
        assert_eq!(
            (r.code, r.position),
            (RejectCode::InvalidSql, Some(11)),
            "начало второго выражения"
        );
        let r = check("SELECT 1 FROM t FOR UPDATE", Purpose::Select).unwrap_err();
        assert_eq!(r.position, None, "у блокировки строк позиции в дереве нет");
        assert_eq!(RejectCode::InvalidSql.as_str(), "invalid_sql");
        assert_eq!(RejectCode::ForbiddenFunction.as_str(), "forbidden_function");
    }

    #[test]
    fn checked_serializes_in_camel_case() {
        let c = check(
            "CREATE INDEX ON shop.orders (lower(status))",
            Purpose::HypoIndex,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({
                "kind": "createIndex",
                "functions": [{"name": "lower"}],
                "tables": [{"schema": "shop", "name": "orders"}]
            })
        );
    }

    /// Имя гипотетического индекса снимается (P4a, E4): по дереву разбора и токенам — в кавычках, с `IF NOT EXISTS`,
    /// с `USING` и `WHERE`, с переводами строк; прочие формы — `None`, текст проверяется как есть, и отказы `check`
    /// прежние.
    #[test]
    fn index_name_is_dropped_by_tokens() {
        for (ddl, want) in [
            (
                "CREATE INDEX idx_orders_status ON shop.orders (status)",
                "CREATE INDEX ON shop.orders (status)",
            ),
            ("create index \"My Idx\" on t (a)", "create index on t (a)"),
            ("CREATE INDEX \"й\" ON t (a)", "CREATE INDEX ON t (a)"),
            (
                "CREATE INDEX IF NOT EXISTS idx ON t (a);",
                "CREATE INDEX ON t (a);",
            ),
            (
                "  CREATE INDEX\n  idx\n  ON t USING btree (a) WHERE a > 0",
                "  CREATE INDEX\n  ON t USING btree (a) WHERE a > 0",
            ),
            ("CREATE INDEX if ON t (a)", "CREATE INDEX ON t (a)"),
            // Комментарий вне заголовка остаётся: его отвергает правило P3b (`validate_ddl`) приложения.
            (
                "CREATE INDEX idx ON t (a) /* x */",
                "CREATE INDEX ON t (a) /* x */",
            ),
        ] {
            assert_eq!(without_index_name(ddl).as_deref(), Some(want), "{ddl}");
            assert!(
                check(want, Purpose::HypoIndex).is_ok(),
                "{want}: принимается проверкой"
            );
        }
        for ddl in [
            "CREATE INDEX ON t (a)",
            "CREATE INDEX s.idx ON t (a)",
            "CREATE UNIQUE INDEX idx ON t (a)",
            "CREATE INDEX CONCURRENTLY idx ON t (a)",
            "CREATE INDEX IF NOT EXISTS ON t (a)",
            "CREATE INDEX idx ON t (a); CREATE INDEX idx2 ON t (b)",
            "CREATE INDEX /* c */ idx ON t (a)",
            "CREATE INDEX idx -- c\n ON t (a)",
            "SELECT 1",
            "",
        ] {
            assert_eq!(without_index_name(ddl), None, "{ddl}");
        }
        // Отказ имени у самой проверки — прежний (её назначение `HypoIndex` общее с P3b-правилом).
        let r = check("CREATE INDEX idx ON t (a)", Purpose::HypoIndex).unwrap_err();
        assert!(
            r.message.starts_with("Index names are not accepted"),
            "{}",
            r.message
        );
    }

    /// Отказ по глубине — короткое сообщение, без пути prost из сотни полей; имя в `U&"…"` раскрывает разбор.
    #[test]
    fn deep_tree_message_is_short_and_unicode_names_are_decoded() {
        let r = check(&format!("SELECT 1{}", " + 1".repeat(60)), Purpose::Select).unwrap_err();
        assert_eq!(r.code, RejectCode::InvalidSql);
        assert!(
            r.message.len() < 200
                && (r.message.contains("too deeply") || r.message.contains("stack depth")),
            "{}",
            r.message
        );
        let r = check(r#"SELECT U&"\0070g_sleep"(1)"#, Purpose::Select).unwrap_err();
        assert_eq!(
            (r.code, r.message.as_str()),
            (
                RejectCode::ForbiddenFunction,
                "Function pg_sleep is not allowed"
            )
        );
    }
}
