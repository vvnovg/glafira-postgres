//! `run_select` (спец. P4 §7.4): уровень `data` (§4.3 шаг 4), проверка SQL модели (`model_sql::check_model_sql`,
//! назначение `Select`), слои §7.3 (`GuardTx`: `READ ONLY`, `prepare` под точкой сохранения, проба плана с поиском
//! запрещённых функций), сверка функций с `pg_proc` (шаг 4 — по дереву разбора до пробы и по VERBOSE-плану после неё,
//! решение владельца P4b; модуль `functions`), курсор `NO SCROLL` над обёрткой, которая обрезает значения на сервере
//! (`cursor_query`, решение владельца Q5), и выборка в текстовом формате до пределов строк, байт и времени. Подготовка и
//! выборка — под общим сроком и отменой клиентом. Значения в журнал не пишутся — только число строк (`rows`, ставит
//! крейт шлюза).

mod functions;

use std::future::Future;
use std::time::{Duration, Instant, SystemTime};

use glafira_mcp_core::error::ToolError;
use glafira_mcp_core::limits::HARD_LIMIT_BYTES;
use glafira_pg_mcp::model::{ObserveCtx, PgAnswer, PgOutput, SelectColumn, SelectOut};
use glafira_pg_mcp::params::{RunSelectParams, SELECT_DEFAULT_ROWS};
use pg_advisor::readiness::{DANGEROUS_ROLES, dangerous_rights};
use pg_advisor::sqlcheck::{FuncRef, Purpose, statement_text};
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, SimpleQueryMessage, SimpleQueryRow};

use crate::conn::errors::from_pg;
use crate::conn::profile::{ConnectionKind, LlmAccess};
use crate::error::AppResult;
use crate::explain::Params;
use crate::explain::params::{count_params, normalize};
use crate::mcp::access::{check_connection, ensure_open, err, remember_role_warning, tool_error};
use crate::mcp::guard::{self, GuardTx};
use crate::mcp::model_sql::{check_model_sql, resolve_database};
use crate::product::{MCP_APPLICATION_NAME, SQL_MARKER};
use crate::state::AppState;
use crate::storage::connections::ConnectionProfile;
use crate::topology::model::Topology;
use crate::topology::observe::leader_member;

use functions::reject_functions;
pub use functions::{FuncHit, Lookups, function_rejection, plan_functions, user_functions};

/// `statement_timeout` и общий срок выборки, секунды (спец. §7.3, §10.1).
pub const SELECT_TIMEOUT_S: u32 = 10;
/// Строк в одном `FETCH` (при немногих колонках, см. `batch_rows`).
pub const FETCH_BATCH: usize = 100;
/// Значение длиннее — обрезается с пометкой `TRUNCATED_MARK`.
pub const MAX_VALUE_BYTES: usize = 2_048;
pub const TRUNCATED_MARK: &str = "…[truncated]";
/// Символов значения, которые отдаёт сервер (`left` в `cursor_query`): на один больше `MAX_VALUE_BYTES` — `cap_value`
/// видит «длиннее 2 КБ» и ставит пометку; `left` считает символы, `cap_value` режет по байтам.
pub const SERVER_CUT_CHARS: usize = MAX_VALUE_BYTES + 1;
/// Значений в одной пачке `FETCH` не больше стольких (кроме строки, в которой колонок больше): пачка в памяти
/// приложения — не больше `BATCH_VALUES` × `SERVER_CUT_CHARS` символов (≈ 3,3 млн, до 13 МБ в UTF-8), сколько бы
/// колонок ни было; до 16 колонок — `FETCH_BATCH` строк.
const BATCH_VALUES: usize = 1_600;
const CURSOR: &str = "glafira_c";
/// Запас клиентского срока над серверным: сервер успевает прервать инструкцию сам (как `CLIENT_MARGIN_S` EXPLAIN).
const CLIENT_MARGIN: Duration = Duration::from_secs(5);

/// Шаг 4 проверок §4.3: `run_select` — только на действующем уровне `data`. Уровень `data` под потолком политики
/// `diagnostics` — `data_disabled`, остальное — `data_not_allowed`.
pub fn data_check(configured: LlmAccess, effective: LlmAccess) -> Result<(), ToolError> {
    match (configured, effective) {
        (_, LlmAccess::Data) => Ok(()),
        (LlmAccess::Data, _) => Err(err(
            "data_disabled",
            "Reading data is limited by the organization's policy",
        )),
        _ => Err(err(
            "data_not_allowed",
            "LLM access to this connection is limited to diagnostics. Ask the user to switch it to \"Diagnostics and \
             data\" in the connection settings in Glafira",
        )),
    }
}

/// Ошибка сервера при выполнении SQL модели по коду SQLSTATE: права — `NOT_AUTHORIZED` с текстом сервера; запись в
/// `READ ONLY` — `invalid_sql`; прочие ошибки самого запроса (данные, разбор, деление на ноль…) — `invalid_sql` с текстом
/// сервера, чтобы модель исправила запрос. `None` — код приложения через `from_pg`: срок (57014) и блокировка (55P03) —
/// `TIMEOUT`, соединение, ресурсы, конфликт с восстановлением, остановка и внутренние ошибки сервера.
fn db_error(code: &SqlState, message: &str) -> Option<ToolError> {
    if *code == SqlState::INSUFFICIENT_PRIVILEGE {
        Some(err(
            "NOT_AUTHORIZED",
            format!(
                "Not enough privileges on the query's tables (pg_read_all_data or SELECT is needed): {message}"
            ),
        ))
    } else if *code == SqlState::READ_ONLY_SQL_TRANSACTION {
        Some(err(
            "invalid_sql",
            format!("The query tries to write, run_select only reads: {message}"),
        ))
    } else if *code == SqlState::LOCK_NOT_AVAILABLE
        || ["08", "40", "53", "57", "58", "XX"].contains(&&code.code()[..2])
    {
        None
    } else {
        Some(err("invalid_sql", message))
    }
}

fn server_error(e: &tokio_postgres::Error) -> ToolError {
    e.as_db_error()
        .and_then(|db| db_error(db.code(), db.message()))
        .unwrap_or_else(|| tool_error(from_pg(e)))
}

/// Значение не длиннее `MAX_VALUE_BYTES` (по границе символа) с пометкой; `true` — обрезано.
pub fn cap_value(v: &str) -> (String, bool) {
    if v.len() <= MAX_VALUE_BYTES {
        return (v.to_string(), false);
    }
    let mut end = MAX_VALUE_BYTES;
    while !v.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}{TRUNCATED_MARK}", &v[..end]), true)
}

/// Размер строки в JSON ответа: значения в кавычках с экранированием, `null`, запятые между значениями и строками, скобки.
fn row_json_bytes(values: &[Option<String>]) -> usize {
    let cells: usize = values
        .iter()
        .map(|v| match v {
            Some(s) => serde_json::to_string(s).map_or(s.len() + 2, |j| j.len()),
            None => 4,
        })
        .sum();
    cells + values.len().saturating_sub(1) + 2 + 1
}

/// Место в ответе под строки (спец. §7.4, «100 КБ значений»; уточнение 6 плана P4b): предел ответа шлюза минус всё
/// остальное — узел, колонки, счётчики. Так ответ из набранных строк не превышает 100 КБ, и модель не получает
/// `response_too_large`.
pub fn rows_budget(member: &str, columns: &[SelectColumn]) -> usize {
    let envelope = SelectOut {
        member: member.to_string(),
        columns: columns.to_vec(),
        rows: Vec::new(),
        row_count: u64::MAX,
        truncated: true,
        truncated_values: u64::MAX,
        ms: u64::MAX,
    };
    let used = serde_json::to_vec(&envelope).map_or(HARD_LIMIT_BYTES, |b| b.len());
    HARD_LIMIT_BYTES.saturating_sub(used)
}

/// Строки курсора и пределы §7.4: строк `max_rows`, строк в JSON — `budget` байт (`rows_budget`), значение —
/// `MAX_VALUE_BYTES`.
#[derive(Debug, Default)]
pub struct Collector {
    max_rows: usize,
    budget: usize,
    pub rows: Vec<Vec<Option<String>>>,
    bytes: usize,
    pub truncated: bool,
    pub truncated_values: u64,
}

impl Collector {
    pub fn new(max_rows: usize, budget: usize) -> Self {
        Self {
            max_rows,
            budget,
            ..Self::default()
        }
    }

    /// Сколько строк просить следующим `FETCH`: не больше `FETCH_BATCH` и на одну больше остатка — так видно, что строки
    /// ещё есть; `0` — выборка закончена.
    pub fn want(&self) -> usize {
        if self.truncated {
            0
        } else {
            FETCH_BATCH.min(self.max_rows - self.rows.len() + 1)
        }
    }

    /// Строка принята; `false` — предел (строк или байт), строка не взята, `truncated` выставлен.
    pub fn accept(&mut self, row: &[Option<&str>]) -> bool {
        if self.rows.len() >= self.max_rows {
            self.truncated = true;
            return false;
        }
        let mut cut = 0u64;
        let values: Vec<Option<String>> = row
            .iter()
            .map(|v| {
                v.map(|s| {
                    let (s, c) = cap_value(s);
                    cut += u64::from(c);
                    s
                })
            })
            .collect();
        let size = row_json_bytes(&values);
        if self.bytes + size > self.budget {
            self.truncated = true;
            return false;
        }
        self.bytes += size;
        self.truncated_values += cut;
        self.rows.push(values);
        true
    }
}

/// Узел по умолчанию (спец. §7.4): доступная реплика с наименьшим лагом (неизвестный лаг — последним, при равенстве — по
/// имени), иначе лидер / primary / сервер.
pub fn pick_member(kind: ConnectionKind, t: &Topology) -> Option<String> {
    t.members
        .iter()
        .filter(|m| m.reachable && m.role.is_standby())
        .min_by(|a, b| {
            a.lag_bytes
                .unwrap_or(i64::MAX)
                .cmp(&b.lag_bytes.unwrap_or(i64::MAX))
                .then_with(|| a.name.cmp(&b.name))
        })
        .or_else(|| leader_member(kind, t))
        .map(|m| m.name.clone())
}

/// Узел по умолчанию открытого подключения; `None` — подключение не открыто или доступного узла нет.
pub fn default_member(st: &AppState, connection_id: &str) -> Option<String> {
    let profile = st.profile(connection_id).ok()?;
    let topology = st.runtimes.view(connection_id).topology?;
    pick_member(profile.kind, &topology)
}

const ROLE_SQL: &str = marked_sql!(
    "SELECT current_user::text, r.rolsuper, ARRAY(SELECT b.rolname::text FROM pg_roles b \
     WHERE b.rolname::text = ANY ($1::text[]) AND pg_has_role(current_user, b.oid, 'USAGE')) \
     FROM pg_roles r WHERE r.rolname = current_user"
);
/// Точка сохранения факта роли: ошибка `ROLE_SQL` (срок блокировки каталога, срок инструкции) откатывается к ней и не
/// прерывает транзакцию вызова — иначе выборка упала бы с «current transaction is aborted».
const ROLE_SAVEPOINT_SQL: &str = marked_sql!("SAVEPOINT glafira_role");
const ROLE_ROLLBACK_SQL: &str = marked_sql!("ROLLBACK TO SAVEPOINT glafira_role");
const ROLE_RELEASE_SQL: &str = marked_sql!("RELEASE SAVEPOINT glafira_role");

/// Роль соединения, суперпользователь ли она и её опасные роли (`ROLE_SQL`) — под точкой сохранения.
async fn role_fact(c: &Client) -> Result<(String, bool, Vec<String>), tokio_postgres::Error> {
    c.batch_execute(ROLE_SAVEPOINT_SQL).await?;
    match c.query_one(ROLE_SQL, &[&&DANGEROUS_ROLES[..]]).await {
        Ok(row) => {
            c.batch_execute(ROLE_RELEASE_SQL).await?;
            Ok((row.try_get(0)?, row.try_get(1)?, row.try_get(2)?))
        }
        Err(e) => {
            c.batch_execute(ROLE_ROLLBACK_SQL).await?;
            Err(e)
        }
    }
}

/// Факт роли (спец. §4.5) обновляет `list_connections.roleWarning`: опасные роли — `pg_advisor::readiness::DANGEROUS_ROLES`.
/// Читается на каждом вызове, прошедшем проверки, — одна строка каталога в той же транзакции; так «при первом
/// `run_select` сессии» выполняется без id сессии в бэкенде. Ошибка — только запись в лог: шаг идёт под своей точкой
/// сохранения (`role_fact`), и предупреждение не ломает выборку.
async fn refresh_role_warning(st: &AppState, connection_id: &str, c: &Client) {
    match role_fact(c).await {
        Ok((user, superuser, member_of)) => {
            let rights = dangerous_rights(superuser, &member_of);
            let warning =
                (!rights.is_empty()).then(|| crate::mcp::texts::role_warning_text(&user, &rights));
            remember_role_warning(st, connection_id, warning);
        }
        Err(e) => {
            tracing::debug!(connection = %connection_id, "run_select role check failed: {e}");
        }
    }
}

/// Колонки результата: имя и тип по описанию подготовленного выражения (Parse/Describe, без выполнения).
async fn describe(c: &Client, sql: &str) -> Result<Vec<SelectColumn>, ToolError> {
    let stmt = c.prepare(sql).await.map_err(|e| server_error(&e))?;
    Ok(stmt
        .columns()
        .iter()
        .map(|col| SelectColumn {
            name: col.name().to_string(),
            type_name: col.type_().name().to_string(),
        })
        .collect())
}

/// Сеанс сервера соединения вызова: отменять инструкцию можно только его (pid переиспользуются).
const BACKEND_SQL: &str =
    marked_sql!("SELECT pid, backend_start FROM pg_stat_activity WHERE pid = pg_backend_pid()");
/// Отмена — только того же сеанса: pid, начало сеанса и имя приложения SQL модели.
const CANCEL_SQL: &str = marked_sql!(
    "SELECT pg_cancel_backend(pid) FROM pg_stat_activity \
     WHERE pid = $1 AND backend_start = $2 AND application_name = $3"
);

#[derive(Debug, Clone, Copy)]
struct Backend {
    pid: i32,
    started: SystemTime,
}

async fn backend_of(c: &Client) -> Result<Backend, ToolError> {
    let row = c
        .query_one(BACKEND_SQL, &[])
        .await
        .map_err(|e| server_error(&e))?;
    Ok(Backend {
        pid: row.get(0),
        started: row.get(1),
    })
}

/// Выражение колонки `i` обёртки `cursor_query`: значение текстом функции вывода своего типа (`format('%s', …)` — тот
/// же текст, что отдал бы `FETCH` без обёртки: `t`/`f`, `inet` без `/32`, `char(n)` с хвостовыми пробелами; `::text` дал
/// бы `true`, `10.0.0.1/32` и снял бы пробелы), обрезанное до `SERVER_CUT_CHARS` символов. NULL остаётся NULL:
/// `num_nulls` смотрит на само значение (у составного `ROW(NULL, NULL) IS NULL` истинно, а значение есть — `(,)`).
/// Функции и оператор — с `pg_catalog`: обёртку проба не видит, и `search_path` с пользовательской схемой перед
/// `pg_catalog` не подменит их своими.
fn cut_column(i: usize) -> String {
    format!(
        "CASE WHEN pg_catalog.num_nulls(s.c{i}) OPERATOR(pg_catalog.=) 0 \
         THEN pg_catalog.left(pg_catalog.format('%s', s.c{i}), {SERVER_CUT_CHARS}) END"
    )
}

/// Выражение курсора (решение владельца Q5 — обрезка на сервере):
/// `SELECT <cut_column 1…N> FROM (SELECT * FROM (\n<выражение модели>\n) q OFFSET 0) s(c1, …, cN)`. Значение длиннее
/// `SERVER_CUT_CHARS` символов не доходит до приложения целиком: пачка `FETCH` в его памяти — не больше строк × колонок ×
/// `SERVER_CUT_CHARS` символов (`batch_rows`). `body` — выражение модели без хвоста за ним (`;` и комментарии,
/// `sqlcheck::statement_text`); перевод строки перед `)` закрывает строчный комментарий в конце выражения. `OFFSET 0` —
/// барьер: без него планировщик подтянул бы подзапрос и подставил выражение колонки в обе ссылки `cut_column` — значение
/// (например, `repeat(…)` в сотни МБ) считалось бы дважды; изменчивые функции он и так не подтягивает. Имена `s(…)` — по
/// позиции: повторы и безымянные колонки (`?column?`) не мешают; колонок `columns` — по описанию выражения модели
/// (`describe`), оттуда же имена и типы ответа. `WITH …`, `VALUES`, `TABLE` и литералы без типа в подзапросе допустимы
/// (PostgreSQL 10+ даёт литералу тип `text`). Без колонок (`SELECT FROM t`) обёртки нет — нет и значений. Порядок строк:
/// внешние `SELECT` без соединений, группировки и сортировки сохраняют порядок подзапроса с `ORDER BY` — проекция идёт
/// поверх его отсортированного вывода (так PostgreSQL делает на практике; стандарт порядок подзапроса не обещает).
/// Проверки (`prepare_one`, проба, сверка функций) идут по тексту модели, не по обёртке.
pub fn cursor_query(body: &str, columns: usize) -> String {
    if columns == 0 {
        return body.to_string();
    }
    let values: Vec<String> = (1..=columns).map(cut_column).collect();
    let names: Vec<String> = (1..=columns).map(|i| format!("c{i}")).collect();
    format!(
        "SELECT {} FROM (SELECT * FROM (\n{body}\n) q OFFSET 0) s({})",
        values.join(", "),
        names.join(", ")
    )
}

/// Курсор без метки `/* glafira */` (текст модели внутри, как EXPLAIN пользователя в P3) и расширенным протоколом —
/// вторую инструкцию сервер отверг бы при разборе (её уже отверг `prepare_one`).
fn declare_sql(sql: &str) -> String {
    format!("DECLARE {CURSOR} NO SCROLL CURSOR FOR {sql}")
}

fn fetch_sql(rows: usize) -> String {
    format!("{SQL_MARKER}FETCH {rows} FROM {CURSOR}")
}

/// Строк в одном `FETCH`: `FETCH_BATCH`, а при многих колонках — столько, чтобы значений было не больше `BATCH_VALUES`
/// (но не меньше одной строки).
fn batch_rows(columns: usize) -> usize {
    (BATCH_VALUES / columns.max(1)).clamp(1, FETCH_BATCH)
}

/// Срок следующего `FETCH` — остаток общего срока (не меньше 1 мс: ноль отключил бы `statement_timeout`).
fn deadline_sql(left: Duration) -> String {
    format!(
        "{SQL_MARKER}SET LOCAL statement_timeout = '{}ms'",
        left.as_millis().max(1)
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// Пределы `Collector` или курсор кончился.
    Done,
    /// Вышел общий срок: сервер прервал `FETCH` (`statement_timeout`) или срок кончился до него; `running` — клиент
    /// перестал ждать, а сервер, возможно, ещё выполняет инструкцию.
    TimedOut { running: bool },
    /// Клиент отменил вызов; инструкция, возможно, ещё выполняется.
    Cancelled,
}

/// Шаг под общим сроком и отменой клиентом (подготовка курсора и каждый `FETCH`): `Err(Stop)` — клиент отменил вызов
/// (`Cancelled`) или вышел клиентский срок `until` (`TimedOut { running: true }`; это общий срок и `CLIENT_MARGIN` —
/// сервер успевает прервать инструкцию сам по `statement_timeout`).
async fn guarded<T>(
    ctx: &ObserveCtx,
    until: Instant,
    step: impl Future<Output = T>,
) -> Result<T, Stop> {
    tokio::select! {
        _ = ctx.cancel.cancelled() => Err(Stop::Cancelled),
        r = tokio::time::timeout_at(until.into(), step) => r.map_err(|_| Stop::TimedOut { running: true }),
    }
}

/// Значение не читается как текст (не UTF-8: база в SQL_ASCII с другими байтами) — отказ с подсказкой, а не паника
/// `SimpleQueryRow::get`. `column` — с нуля.
fn not_text(column: usize, columns: &[SelectColumn]) -> ToolError {
    let name = columns.get(column).map_or("?", |c| c.name.as_str());
    err(
        "invalid_sql",
        format!(
            "Column {} ({name}) has a value that is not valid UTF-8 text (the database encoding may be SQL_ASCII): leave the column out or select it as encode(convert_to(<expression>::text, 'SQL_ASCII'), 'hex')",
            column + 1
        ),
    )
}

/// Значения строки `FETCH`; не текст — `not_text`.
fn row_values<'a>(
    r: &'a SimpleQueryRow,
    columns: &[SelectColumn],
) -> Result<Vec<Option<&'a str>>, ToolError> {
    (0..r.len())
        .map(|i| r.try_get(i).map_err(|_| not_text(i, columns)))
        .collect()
}

/// `FETCH` пачками (`batch_rows`) в текстовом формате (простой протокол) до пределов `Collector`, срока `deadline` или
/// отмены. Перед каждой пачкой `statement_timeout` = остаток срока: 10 с — на всю выборку, а не на каждый `FETCH`.
async fn fetch(
    c: &Client,
    col: &mut Collector,
    columns: &[SelectColumn],
    deadline: Instant,
    ctx: &ObserveCtx,
) -> Result<Stop, ToolError> {
    let batch = batch_rows(columns.len());
    loop {
        let want = col.want().min(batch);
        if want == 0 {
            return Ok(Stop::Done);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(Stop::TimedOut { running: false });
        }
        let set = deadline_sql(left);
        let fetch = fetch_sql(want);
        let step = async {
            c.batch_execute(&set).await?;
            c.simple_query(&fetch).await
        };
        let messages = match guarded(ctx, deadline + CLIENT_MARGIN, step).await {
            Err(stop) => return Ok(stop),
            Ok(Err(e)) if e.code() == Some(&SqlState::QUERY_CANCELED) => {
                return Ok(Stop::TimedOut { running: false });
            }
            Ok(Err(e)) => return Err(server_error(&e)),
            Ok(Ok(m)) => m,
        };
        let mut got = 0;
        for m in &messages {
            if let SimpleQueryMessage::Row(r) = m {
                got += 1;
                if !col.accept(&row_values(r, columns)?) {
                    return Ok(Stop::Done);
                }
            }
        }
        if got < want {
            return Ok(Stop::Done);
        }
    }
}

/// Итог вызова по `Stop`: что делать с сеансом сервера и что ответить.
#[derive(Debug, PartialEq, Eq)]
struct Ending {
    /// Сервер, возможно, ещё выполняет инструкцию (отмена клиентом, истёк клиентский срок): соединение закрывается
    /// сразу (`ROLLBACK` ждал бы её), сеанс отменяется отдельно (`cancel_backend`); иначе — `finish`.
    cancel: bool,
    /// Строки с этим `truncated` или `TIMEOUT` — срок вышел, а строк нет.
    answer: Result<bool, ToolError>,
}

/// `Stop` → `Ending`: пределы или конец курсора — `truncated` сборщика; срок или отмена — уже собранные строки с
/// `truncated: true`; срок без строк — `TIMEOUT`, отмена без строк — пустой ответ с `truncated: true`.
fn ending(stop: Stop, rows: usize, truncated: bool) -> Ending {
    Ending {
        cancel: matches!(stop, Stop::Cancelled | Stop::TimedOut { running: true }),
        answer: match stop {
            Stop::Done => Ok(truncated),
            Stop::TimedOut { .. } if rows == 0 => Err(err(
                "TIMEOUT",
                format!(
                    "run_select did not return rows within {SELECT_TIMEOUT_S} s and was cancelled; narrow the query (WHERE, LIMIT, fewer joins)"
                ),
            )),
            Stop::TimedOut { .. } | Stop::Cancelled => Ok(true),
        },
    }
}

/// Отмена инструкции, оставшейся на сервере после отмены клиентом или истёкшего клиентского срока (уточнение 4 плана
/// P4b: у `ConnectSpec` нет пути отправить `CancelToken` через туннель): из соединения пула узла (та же роль может
/// отменять свои сеансы), только тот же сеанс (`CANCEL_SQL`). Соединение вызова закрывается сбросом `GuardTx`.
async fn cancel_backend(st: AppState, connection_id: String, member: String, b: Backend) {
    let r = async {
        let c = st.client(&connection_id, Some(&member), None).await?;
        c.query(CANCEL_SQL, &[&b.pid, &b.started, &MCP_APPLICATION_NAME])
            .await
            .map_err(|e| from_pg(&e))?;
        AppResult::Ok(())
    }
    .await;
    if let Err(e) = r {
        tracing::warn!(
            connection = %connection_id,
            member = %member,
            pid = b.pid,
            "run_select cancel failed: {}",
            e.message
        );
    }
}

/// Проверки на сервере до выборки (§7.3) и подготовка курсора: сеанс для отмены (первым — отменить можно уже
/// подготовку; в `backend`); `prepare` под точкой сохранения; функции дерева разбора — до пробы (планировщик может
/// вычислить функцию с константами уже при EXPLAIN); проба VERBOSE-плана с поиском запрещённых функций; функции плана;
/// колонки; факт роли; `DECLARE` над обёрткой `cursor_query`. Проверки — по тексту модели: `original` — как его прислала
/// модель (позиции ошибок — в нём), `sql` — нормализованный (колонки и выражение курсора).
async fn prepare_cursor(
    st: &AppState,
    connection_id: &str,
    tx: &GuardTx,
    original: &str,
    sql: &str,
    tree: &[FuncRef],
    backend: &mut Option<Backend>,
) -> Result<Vec<SelectColumn>, ToolError> {
    *backend = Some(backend_of(tx.client()).await?);
    tx.prepare_one(original).await?;
    reject_functions(tx.client(), tree, &[]).await?;
    let raw = tx.probe(original, &Params::None).await?;
    reject_functions(tx.client(), &[], &plan_functions(&raw)).await?;
    let columns = describe(tx.client(), sql).await?;
    refresh_role_warning(st, connection_id, tx.client()).await;
    let body = statement_text(sql).unwrap_or(sql);
    tx.client()
        .execute(
            declare_sql(&cursor_query(body, columns.len())).as_str(),
            &[],
        )
        .await
        .map_err(|e| server_error(&e))?;
    Ok(columns)
}

fn select_profile(st: &AppState, p: &RunSelectParams) -> Result<ConnectionProfile, ToolError> {
    // §4.3 шаг 3 (доступ) и шаг 4 (уровень data).
    let (profile, level) = check_connection(st, &p.connection_id)?;
    data_check(profile.llm_access, level)?;
    Ok(profile)
}

/// `run_select` (спец. §7.4): доступ и уровень `data`, SQL модели до соединения (`$N` не принимаются), узел по
/// умолчанию — реплика с наименьшим лагом (`pick_member`), база по умолчанию — база подключения (решение владельца
/// Q2), проверки на сервере и курсор (`prepare_cursor`), выборка до пределов (`fetch`) — обе фазы под общим сроком и
/// отменой клиентом (`guarded`). Итог — `ending`: отмена клиентом и истёкший срок отдают уже собранные строки с
/// `truncated: true`; срок без строк — `TIMEOUT`.
pub async fn run_select(
    st: &AppState,
    p: RunSelectParams,
    ctx: ObserveCtx,
) -> Result<PgAnswer, ToolError> {
    let started = Instant::now();
    let profile = select_profile(st, &p)?;
    let database = resolve_database(&profile, p.database.as_deref());
    let checked = check_model_sql(&p.sql, Purpose::Select)?;
    let sql = normalize(&p.sql);
    if count_params(&sql) > 0 {
        return Err(err(
            "invalid_argument",
            "run_select does not take $N parameters: put the values into the query",
        ));
    }
    ensure_open(st, &p.connection_id).await?;
    let member = match p.member.as_deref().filter(|m| !m.trim().is_empty()) {
        Some(m) => m.to_string(),
        None => default_member(st, &p.connection_id)
            .ok_or_else(|| err("NODE_UNAVAILABLE", "No node of the connection is available"))?,
    };
    // Слои §7.3: READ ONLY, сроки, одна инструкция (prepare под точкой сохранения), проба плана с поиском запрещённого.
    let tx = guard::begin(
        st,
        &p.connection_id,
        Some(&member),
        &database,
        SELECT_TIMEOUT_S,
    )
    .await?;
    let deadline = Instant::now() + Duration::from_secs(u64::from(SELECT_TIMEOUT_S));
    let max_rows = p.max_rows.unwrap_or(SELECT_DEFAULT_ROWS) as usize;
    let mut backend = None;
    let prepared = guarded(
        &ctx,
        deadline + CLIENT_MARGIN,
        prepare_cursor(
            st,
            &p.connection_id,
            &tx,
            &p.sql,
            &sql,
            &checked.functions,
            &mut backend,
        ),
    )
    .await;
    let (columns, col, stop) = match prepared {
        Ok(Ok(columns)) => {
            let mut col = Collector::new(max_rows, rows_budget(&member, &columns));
            match fetch(tx.client(), &mut col, &columns, deadline, &ctx).await {
                Ok(stop) => (columns, col, stop),
                Err(e) => {
                    tx.finish().await;
                    return Err(e);
                }
            }
        }
        Ok(Err(e)) => {
            tx.finish().await;
            return Err(e);
        }
        // Подготовку прервали отмена клиентом или клиентский срок: колонок и строк нет.
        Err(stop) => (Vec::new(), Collector::default(), stop),
    };
    let end = ending(stop, col.rows.len(), col.truncated);
    if end.cancel {
        // Соединение закрывается сразу, сеанс отменяется отдельно (если он уже известен; иначе его инструкция кончится
        // по своему `statement_timeout`).
        if let Some(b) = backend {
            tokio::spawn(cancel_backend(
                st.clone(),
                p.connection_id.clone(),
                member.clone(),
                b,
            ));
        }
        drop(tx);
    } else {
        tx.finish().await;
    }
    let truncated = end.answer?;
    let row_count = col.rows.len() as u64;
    Ok(PgAnswer {
        body: PgOutput::Select(SelectOut {
            member,
            columns,
            rows: col.rows,
            row_count,
            truncated,
            truncated_values: col.truncated_values,
            ms: started.elapsed().as_millis() as u64,
        }),
        masked: None,
        rows: Some(row_count),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glafira_pg_mcp::model::ObserveCtx;

    use super::*;
    use crate::conn::profile::sample_profile;
    use crate::error::AppResult;
    use crate::policy::{McpMaxAccess, Policy};
    use crate::secrets::MemoryStore;
    use crate::topology::BoxFuture;
    use crate::topology::model::{Member, Role, member, topo};
    use crate::topology::runtime::{ConnState, Runtimes, TopologySource};

    #[test]
    fn values_are_cut_at_2_kb_on_a_char_boundary() {
        assert_eq!(cap_value("abc"), ("abc".to_string(), false));
        let (v, cut) = cap_value(&"a".repeat(3000));
        assert!(cut);
        assert_eq!(v.len(), MAX_VALUE_BYTES + TRUNCATED_MARK.len());
        assert!(v.ends_with(TRUNCATED_MARK));
        assert!(
            !cap_value(&"a".repeat(MAX_VALUE_BYTES)).1,
            "ровно 2 КБ — целиком"
        );
        // «€» — 3 байта: граница символа перед 2048-м байтом — 2046.
        let (v, _) = cap_value(&"€".repeat(1000));
        assert_eq!(v.strip_suffix(TRUNCATED_MARK).unwrap(), "€".repeat(682));
    }

    #[test]
    fn collector_stops_at_max_rows_and_asks_for_one_more() {
        let mut c = Collector::new(2, 100_000);
        assert_eq!(
            c.want(),
            3,
            "на одну строку больше — так видно, что строки ещё есть"
        );
        assert!(c.accept(&[Some("1"), None]));
        assert!(c.accept(&[Some("2"), None]));
        assert_eq!(c.want(), 1);
        assert!(!c.accept(&[Some("3"), None]));
        assert!(c.truncated);
        assert_eq!(c.want(), 0);
        assert_eq!(
            c.rows,
            vec![
                vec![Some("1".to_string()), None],
                vec![Some("2".to_string()), None]
            ]
        );
        assert_eq!(Collector::new(500, 100_000).want(), FETCH_BATCH);
    }

    #[test]
    fn collector_keeps_the_rows_within_the_budget() {
        let long = "z".repeat(3000);
        let row: Vec<Option<&str>> = vec![Some(long.as_str()); 10];
        let mut c = Collector::new(500, 100_000);
        // Значение — 2048 байт и пометка (2062) в кавычках, строка в JSON — 20 652 байта: помещаются 4.
        while c.accept(&row) {}
        assert_eq!(c.rows.len(), 4);
        assert!(c.truncated);
        assert_eq!(c.truncated_values, 40, "только у взятых строк");
        // Строки в JSON считаются с экранированием и `null`.
        assert_eq!(row_json_bytes(&[Some("a\"b".into()), None]), 6 + 4 + 1 + 3);
    }

    #[test]
    fn rows_budget_leaves_room_for_the_envelope() {
        let columns = vec![SelectColumn {
            name: "v".into(),
            type_name: "text".into(),
        }];
        let budget = rows_budget("pg-2", &columns);
        assert!(
            budget > 102_000 && budget < glafira_mcp_core::limits::HARD_LIMIT_BYTES,
            "{budget}"
        );
        // Строки по 1500 байт: ответ из стольких строк, сколько помещается, не превышает предел шлюза.
        let value = "x".repeat(1500);
        let mut c = Collector::new(500, budget);
        while c.accept(&[Some(value.as_str())]) {}
        let n = c.rows.len();
        assert!((60..70).contains(&n), "{n}");
        let out = SelectOut {
            member: "pg-2".into(),
            columns,
            row_count: n as u64,
            rows: c.rows,
            truncated: true,
            truncated_values: 0,
            ms: 12_345,
        };
        assert!(
            serde_json::to_vec(&out).unwrap().len() <= glafira_mcp_core::limits::HARD_LIMIT_BYTES
        );
    }

    #[test]
    fn data_level_is_required_and_the_ceiling_is_named() {
        assert!(data_check(LlmAccess::Data, LlmAccess::Data).is_ok());
        let e = data_check(LlmAccess::Data, LlmAccess::Diagnostics).unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "data_disabled",
                "Reading data is limited by the organization's policy"
            )
        );
        let e = data_check(LlmAccess::Diagnostics, LlmAccess::Diagnostics).unwrap_err();
        assert_eq!(e.code, "data_not_allowed");
        assert!(
            e.message.contains("\"Diagnostics and data\""),
            "{}",
            e.message
        );
    }

    #[test]
    fn default_member_is_the_replica_with_the_smallest_lag() {
        let mut r1 = member("r1", Role::Replica);
        r1.lag_bytes = Some(5_000);
        let mut r2 = member("r2", Role::SyncStandby);
        r2.lag_bytes = Some(10);
        let mut r3 = member("r3", Role::Replica);
        r3.lag_bytes = Some(0);
        r3.reachable = false;
        let t = topo(vec![member("p", Role::Primary), r1.clone(), r2, r3]);
        assert_eq!(
            pick_member(ConnectionKind::Patroni, &t).as_deref(),
            Some("r2")
        );
        // Лаг неизвестен — после известного.
        let mut unknown = member("a-unknown", Role::Replica);
        unknown.lag_bytes = None;
        let t = topo(vec![member("p", Role::Primary), unknown, r1.clone()]);
        assert_eq!(
            pick_member(ConnectionKind::Patroni, &t).as_deref(),
            Some("r1")
        );
        // Реплик нет или они недоступны — лидер.
        let mut down = r1;
        down.reachable = false;
        let t = topo(vec![member("p", Role::Primary), down]);
        assert_eq!(
            pick_member(ConnectionKind::Patroni, &t).as_deref(),
            Some("p")
        );
        let t = topo(vec![member("s", Role::Primary)]);
        assert_eq!(
            pick_member(ConnectionKind::Server, &t).as_deref(),
            Some("s")
        );
        let mut gone = member("p", Role::Primary);
        gone.reachable = false;
        assert_eq!(
            pick_member(ConnectionKind::Patroni, &topo(vec![gone])),
            None
        );
    }

    /// SQL `run_select` проверяется общей проверкой SQL модели (`model_sql::check_model_sql`): код отказа и позиция в
    /// тексте, как его прислала модель; разбор (функции со схемой) уходит в сверку с `pg_proc`.
    #[test]
    fn select_sql_is_checked_like_other_model_sql() {
        let e = check_model_sql("  SELECT pg_sleep(1)", Purpose::Select).unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "forbidden_function",
                "Function pg_sleep is not allowed (position 10)"
            )
        );
        for sql in [
            "SELECT 1; SELECT 2",
            "SELECT * FROM shop.orders FOR UPDATE",
            "WITH d AS (DELETE FROM shop.orders WHERE id = -1 RETURNING *) SELECT * FROM d",
            "SELECT * INTO t2 FROM t",
            "DELETE FROM t",
            "EXPLAIN SELECT 1",
        ] {
            assert_eq!(
                check_model_sql(sql, Purpose::Select).unwrap_err().code,
                "invalid_sql",
                "{sql}"
            );
        }
        for sql in [
            "SELECT crosstab('SELECT 1')",
            "SELECT * FROM pgr_dijkstra('SELECT 1', 1, 2)",
            "SELECT pg_background_launch('DELETE FROM t')",
        ] {
            assert_eq!(
                check_model_sql(sql, Purpose::Select).unwrap_err().code,
                "forbidden_function",
                "{sql}"
            );
        }
        let checked = check_model_sql(
            "SELECT shop.order_count(o.id), lower(o.note) FROM shop.orders o",
            Purpose::Select,
        )
        .unwrap();
        let names: Vec<(Option<&str>, &str)> = checked
            .functions
            .iter()
            .map(|f| (f.schema.as_deref(), f.name.as_str()))
            .collect();
        assert!(names.contains(&(Some("shop"), "order_count")), "{names:?}");
        assert!(names.contains(&(None, "lower")), "{names:?}");
    }

    #[test]
    fn server_errors_of_the_query_reach_the_model() {
        let e = db_error(
            &SqlState::INSUFFICIENT_PRIVILEGE,
            "permission denied for table orders",
        )
        .unwrap();
        assert_eq!(e.code, "NOT_AUTHORIZED");
        assert!(
            e.message.contains("permission denied for table orders"),
            "{}",
            e.message
        );
        let e = db_error(
            &SqlState::READ_ONLY_SQL_TRANSACTION,
            "cannot execute nextval() in a read-only transaction",
        )
        .unwrap();
        assert_eq!(e.code, "invalid_sql");
        assert!(e.message.contains("only reads"), "{}", e.message);
        let e = db_error(&SqlState::DIVISION_BY_ZERO, "division by zero").unwrap();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            ("invalid_sql", "division by zero")
        );
        // Срок, блокировка, соединение, ресурсы, конфликт с восстановлением, внутренние — коды приложения (`from_pg`).
        for code in [
            SqlState::QUERY_CANCELED,
            SqlState::LOCK_NOT_AVAILABLE,
            SqlState::T_R_SERIALIZATION_FAILURE,
            SqlState::ADMIN_SHUTDOWN,
            SqlState::OUT_OF_MEMORY,
            SqlState::CONNECTION_FAILURE,
            SqlState::INTERNAL_ERROR,
        ] {
            assert!(db_error(&code, "x").is_none(), "{}", code.code());
        }
    }

    #[test]
    fn service_sql_is_marked_and_the_model_sql_is_not() {
        for sql in [
            functions::USER_FUNCTIONS_SQL,
            ROLE_SQL,
            ROLE_SAVEPOINT_SQL,
            ROLE_ROLLBACK_SQL,
            ROLE_RELEASE_SQL,
            BACKEND_SQL,
            CANCEL_SQL,
        ] {
            assert!(crate::product::is_marked_sql(sql), "{sql}");
        }
        let fetch = fetch_sql(100);
        assert!(crate::product::is_marked_sql(&fetch), "{fetch}");
        assert!(fetch.ends_with("FETCH 100 FROM glafira_c"), "{fetch}");
        let set = deadline_sql(Duration::from_millis(9_250));
        assert!(crate::product::is_marked_sql(&set), "{set}");
        assert!(
            set.ends_with("SET LOCAL statement_timeout = '9250ms'"),
            "{set}"
        );
        assert!(
            deadline_sql(Duration::from_micros(10)).ends_with("'1ms'"),
            "ноль отключил бы срок"
        );
        let declare = declare_sql("SELECT 1");
        assert_eq!(declare, "DECLARE glafira_c NO SCROLL CURSOR FOR SELECT 1");
    }

    /// Обёртка курсора (решение владельца Q5): каждая колонка — текст функции вывода, обрезанный на сервере до 2049
    /// символов, NULL остаётся NULL; барьер `OFFSET 0` — выражение колонки считается один раз; имена по позиции;
    /// перевод строки перед `)` закрывает строчный комментарий; без колонок — выражение как есть.
    #[test]
    fn cursor_query_cuts_every_column_on_the_server() {
        let cut = |i: usize| {
            format!(
                "CASE WHEN pg_catalog.num_nulls(s.c{i}) OPERATOR(pg_catalog.=) 0 \
                 THEN pg_catalog.left(pg_catalog.format('%s', s.c{i}), 2049) END"
            )
        };
        assert_eq!(SERVER_CUT_CHARS, 2049);
        assert_eq!(
            cursor_query("SELECT 1, 1 -- два раза ?column?", 2),
            format!(
                "SELECT {}, {} FROM (SELECT * FROM (\nSELECT 1, 1 -- два раза ?column?\n) q OFFSET 0) s(c1, c2)",
                cut(1),
                cut(2)
            )
        );
        for body in [
            "WITH w AS (SELECT 'x') SELECT * FROM w",
            "VALUES (1, 'a'), (2, NULL) ORDER BY 1 DESC",
            "TABLE shop.orders",
            "SELECT 'literal without a type'",
        ] {
            let q = cursor_query(body, 1);
            assert_eq!(
                q,
                format!(
                    "SELECT {} FROM (SELECT * FROM (\n{body}\n) q OFFSET 0) s(c1)",
                    cut(1)
                ),
                "{body}"
            );
        }
        assert_eq!(
            cursor_query("SELECT FROM shop.orders", 0),
            "SELECT FROM shop.orders",
            "без колонок значений нет — обёртка не нужна"
        );
        let wide = cursor_query("SELECT * FROM t", 3);
        assert!(wide.ends_with("\n) q OFFSET 0) s(c1, c2, c3)"), "{wide}");
        assert_eq!(wide.matches("pg_catalog.left(").count(), 3, "{wide}");
        // Хвост за выражением модели (`;` и комментарий после неё) в подзапрос не попадает.
        let body = statement_text("SELECT 1; -- конец").unwrap();
        assert_eq!(
            cursor_query(body, 1),
            format!(
                "SELECT {} FROM (SELECT * FROM (\nSELECT 1\n) q OFFSET 0) s(c1)",
                cut(1)
            )
        );
    }

    /// Пачка `FETCH` — не больше `BATCH_VALUES` значений, но не меньше строки; до 16 колонок — `FETCH_BATCH` строк.
    #[test]
    fn batch_rows_bound_the_values_of_one_fetch() {
        for (columns, rows) in [
            (0, 100),
            (1, 100),
            (16, 100),
            (17, 94),
            (100, 16),
            (1_600, 1),
            (1_664, 1),
        ] {
            assert_eq!(batch_rows(columns), rows, "{columns}");
        }
        // Память пачки: строки × колонки × 2049 символов — не больше ≈ 3,3 млн символов при любом числе колонок.
        for columns in [1usize, 7, 16, 17, 100, 1_000, 1_600] {
            assert!(
                batch_rows(columns) * columns * SERVER_CUT_CHARS <= BATCH_VALUES * SERVER_CUT_CHARS,
                "{columns}"
            );
        }
    }

    /// `Stop` → сеанс сервера и ответ: отменять сеанс — когда инструкция, возможно, ещё идёт (отмена клиентом, истёк
    /// клиентский срок); срок без строк — `TIMEOUT`; срок или отмена со строками — строки с `truncated: true`.
    #[test]
    fn ending_maps_the_stop_to_the_session_and_the_answer() {
        assert_eq!(
            ending(Stop::Done, 3, false),
            Ending {
                cancel: false,
                answer: Ok(false)
            }
        );
        assert_eq!(
            ending(Stop::Done, 0, true),
            Ending {
                cancel: false,
                answer: Ok(true)
            }
        );
        for (stop, cancel) in [
            (Stop::TimedOut { running: false }, false),
            (Stop::TimedOut { running: true }, true),
        ] {
            let e = ending(stop, 0, false);
            assert_eq!(e.cancel, cancel, "{stop:?}");
            let timeout = e.answer.unwrap_err();
            assert_eq!(timeout.code, "TIMEOUT");
            assert!(timeout.message.contains("10 s"), "{}", timeout.message);
            assert_eq!(
                ending(stop, 2, false),
                Ending {
                    cancel,
                    answer: Ok(true)
                },
                "{stop:?}: собранные строки — с truncated"
            );
        }
        for rows in [0, 5] {
            assert_eq!(
                ending(Stop::Cancelled, rows, false),
                Ending {
                    cancel: true,
                    answer: Ok(true)
                },
                "отмена клиентом — собранное, даже пустое"
            );
        }
    }

    /// Подготовка курсора и `FETCH` — под общим сроком и отменой клиентом: шаг кончился — его итог; отмена —
    /// `Cancelled`; клиентский срок — `TimedOut { running: true }`.
    #[tokio::test]
    async fn guarded_steps_stop_on_cancel_and_on_the_client_deadline() {
        let ctx = ObserveCtx::new(Duration::from_secs(10));
        let far = Instant::now() + Duration::from_secs(30);
        assert_eq!(guarded(&ctx, far, async { 7 }).await, Ok(7));
        let started = Instant::now();
        let r = guarded(
            &ctx,
            Instant::now() + Duration::from_millis(50),
            std::future::pending::<()>(),
        )
        .await;
        assert_eq!(r, Err(Stop::TimedOut { running: true }));
        assert!(started.elapsed() < Duration::from_secs(5));
        let cancel = ctx.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel.cancel();
        });
        let r = guarded(&ctx, far, std::future::pending::<()>()).await;
        assert_eq!(r, Err(Stop::Cancelled));
    }

    /// Значение не UTF-8 (база в SQL_ASCII) — ошибка инструмента с номером и именем колонки и подсказкой, не паника.
    #[test]
    fn undecodable_values_are_a_tool_error() {
        let columns = vec![
            SelectColumn {
                name: "id".into(),
                type_name: "int4".into(),
            },
            SelectColumn {
                name: "note".into(),
                type_name: "text".into(),
            },
        ];
        let e = not_text(1, &columns);
        assert_eq!(e.code, "invalid_sql");
        assert!(
            e.message
                .starts_with("Column 2 (note) has a value that is not valid UTF-8"),
            "{}",
            e.message
        );
        assert!(e.message.contains("SQL_ASCII"), "{}", e.message);
        assert!(not_text(5, &columns).message.starts_with("Column 6 (?)"));
    }

    // ---- Порядок проверок без сервера ----

    /// Источник топологии, отдающий одну и ту же топологию (подключение открывается без стенда).
    struct Fixed(crate::topology::model::Topology);

    impl TopologySource for Fixed {
        fn observe<'a>(
            &'a self,
            _profile: &'a ConnectionProfile,
            _previous: Option<&'a crate::topology::model::Topology>,
        ) -> BoxFuture<'a, AppResult<crate::topology::model::Topology>> {
            Box::pin(async move { Ok(self.0.clone()) })
        }
    }

    /// Подключения `off`, `diag` и `data` с этими уровнями; политика — с потолком `diagnostics` или без него.
    fn state(capped: bool, members: Vec<Member>) -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let mut policy = Policy::default();
        if capped {
            policy.status.mcp_max_access = McpMaxAccess::Diagnostics;
        }
        let mut st = AppState::new(dir.path(), Arc::new(MemoryStore::default()))
            .unwrap()
            .with_policy(policy);
        st.runtimes = Runtimes::new(
            Arc::new(Fixed(topo(members))),
            st.pools.clone(),
            st.tunnels.clone(),
        );
        for (id, level) in [
            ("off", LlmAccess::Off),
            ("diag", LlmAccess::Diagnostics),
            ("data", LlmAccess::Data),
        ] {
            let mut p = sample_profile(id);
            p.llm_access = level;
            st.connections.upsert(p).unwrap();
        }
        (dir, st)
    }

    fn select(id: &str, sql: &str) -> RunSelectParams {
        RunSelectParams {
            connection_id: id.into(),
            member: None,
            database: None,
            sql: sql.into(),
            max_rows: Some(100),
        }
    }

    async fn code(st: &AppState, id: &str, sql: &str) -> ToolError {
        match run_select(
            st,
            select(id, sql),
            ObserveCtx::new(Duration::from_secs(10)),
        )
        .await
        {
            Ok(_) => panic!("{id}: {sql}: ожидался отказ"),
            Err(e) => e,
        }
    }

    /// §4.3: доступ (шаг 3), затем уровень `data` (шаг 4), затем SQL — всё до открытия подключения.
    #[tokio::test]
    async fn access_level_and_sql_are_checked_before_opening() {
        let (_dir, st) = state(false, vec![member("pg-1", Role::Primary)]);
        assert_eq!(code(&st, "off", "SELECT 1").await.code, "not_allowed");
        assert_eq!(code(&st, "nope", "SELECT 1").await.code, "not_allowed");
        let e = code(&st, "diag", "SELECT pg_sleep(1)").await;
        assert_eq!(e.code, "data_not_allowed", "уровень — раньше SQL");
        for (sql, want) in [
            ("SELECT pg_sleep(1)", "forbidden_function"),
            ("SELECT 1; SELECT 2", "invalid_sql"),
            ("UPDATE t SET a = 1", "invalid_sql"),
            ("SELECT * FROM t WHERE id = $1", "invalid_argument"),
            ("SELECT net.http_get('http://x')", "forbidden_function"),
            (
                "SELECT * FROM aws_s3.query_export_to_s3('SELECT 1', NULL)",
                "forbidden_function",
            ),
            ("SELECT aws_lambda.invoke('f', '{}')", "forbidden_function"),
        ] {
            assert_eq!(code(&st, "data", sql).await.code, want, "{sql}");
        }
        let e = code(&st, "data", "SELECT * FROM t WHERE id = $1").await;
        assert!(e.message.contains("$N"), "{}", e.message);
        for id in ["off", "diag", "data"] {
            assert_eq!(
                st.runtimes.view(id).state,
                ConnState::Disconnected,
                "{id}: отказ не открывает подключение"
            );
        }
        let (_dir, capped) = state(true, vec![member("pg-1", Role::Primary)]);
        assert_eq!(
            code(&capped, "data", "SELECT 1").await.code,
            "data_disabled"
        );
        assert_eq!(
            code(&capped, "diag", "SELECT 1").await.code,
            "data_not_allowed"
        );
    }

    #[tokio::test]
    async fn default_member_of_an_open_connection() {
        let mut replica = member("pg-2", Role::Replica);
        replica.lag_bytes = Some(0);
        let (_dir, st) = state(false, vec![member("pg-1", Role::Primary), replica]);
        assert_eq!(default_member(&st, "data"), None, "подключение не открыто");
        st.runtimes
            .connect(st.profile("data").unwrap())
            .await
            .unwrap();
        assert_eq!(default_member(&st, "data").as_deref(), Some("pg-2"));
        assert_eq!(default_member(&st, "nope"), None);
    }
}
