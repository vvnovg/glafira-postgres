//! `run_select` (спец. P4 §7.4): уровень `data` (§4.3 шаг 4), проверка SQL модели (`model_sql::check_model_sql`,
//! назначение `Select`), слои §7.3 (`GuardTx`: `READ ONLY`, `prepare` под точкой сохранения, проба плана с поиском
//! запрещённых функций), сверка функций с `pg_proc` (шаг 4 — по дереву разбора до пробы и по VERBOSE-плану после неё,
//! решение владельца P4b), курсор `NO SCROLL` и выборка в текстовом формате до пределов строк, байт и времени. Значения
//! в журнал не пишутся — только число строк (`rows`, ставит крейт шлюза).

use std::time::{Duration, Instant, SystemTime};

use glafira_mcp_core::error::ToolError;
use glafira_mcp_core::limits::HARD_LIMIT_BYTES;
use glafira_pg_mcp::model::{ObserveCtx, PgAnswer, PgOutput, SelectColumn, SelectOut};
use glafira_pg_mcp::params::{RunSelectParams, SELECT_DEFAULT_ROWS};
use pg_advisor::functions::{DENIED_SCHEMAS, calls_in_plan};
use pg_advisor::readiness::{DANGEROUS_ROLES, dangerous_rights};
use pg_advisor::sqlcheck::{FuncRef, Purpose};
use serde_json::Value;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, SimpleQueryMessage};

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

/// `statement_timeout` и общий срок выборки, секунды (спец. §7.3, §10.1).
pub const SELECT_TIMEOUT_S: u32 = 10;
/// Строк в одном `FETCH`.
pub const FETCH_BATCH: usize = 100;
/// Значение длиннее — обрезается с пометкой `TRUNCATED_MARK`.
pub const MAX_VALUE_BYTES: usize = 2_048;
pub const TRUNCATED_MARK: &str = "…[truncated]";
const CURSOR: &str = "glafira_c";
/// Запас клиентского срока над серверным: сервер успевает прервать `FETCH` сам (как `CLIENT_MARGIN_S` EXPLAIN).
const CLIENT_MARGIN: Duration = Duration::from_secs(5);
/// Шаблон «любое имя» для `calls_in_plan`: все вызовы VERBOSE-плана.
const ANY_FUNCTION: &str = "*";

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

/// Все имена вызовов VERBOSE-плана пробы (без схемы: план печатает её, только если схема не видна), в порядке плана, без
/// повторов. Функции из представлений и запись через атрибут (`r.f` = `f(r)`) план печатает обычными вызовами; ключевые
/// слова перед скобкой (`ANY (`, `numeric(12,2)`) тоже попадают сюда — в `pg_proc` вне `pg_catalog` их обычно нет.
pub fn plan_functions(raw: &Value) -> Vec<String> {
    calls_in_plan(raw, &[ANY_FUNCTION])
}

/// Имена для сверки с `pg_proc` одним запросом (массивы `unnest`): из дерева разбора — в своей схеме (`schemas`) или в
/// `search_path` (`""`), из плана — в любой схеме (`anywhere`). Повторы не уходят на сервер.
#[derive(Debug, Default, PartialEq)]
pub struct Lookups {
    pub schemas: Vec<String>,
    pub names: Vec<String>,
    pub anywhere: Vec<bool>,
}

impl Lookups {
    pub fn new(tree: &[FuncRef], plan: &[String]) -> Self {
        let mut l = Self::default();
        let refs = tree
            .iter()
            .map(|f| (f.schema.clone().unwrap_or_default(), f.name.clone(), false))
            .chain(plan.iter().map(|n| (String::new(), n.clone(), true)));
        for (schema, name, anywhere) in refs {
            let seen = (0..l.names.len())
                .any(|i| l.names[i] == name && l.schemas[i] == schema && l.anywhere[i] == anywhere);
            if !seen {
                l.schemas.push(schema);
                l.names.push(name);
                l.anywhere.push(anywhere);
            }
        }
        l
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// Функция запроса, которую `run_select` отвергает: найдена в `pg_proc` вне `pg_catalog` и `information_schema` и либо
/// не принадлежит расширению (`pg_depend.deptype = 'e'`), либо лежит в схеме из `DENIED_SCHEMAS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncHit {
    pub schema: String,
    pub name: String,
    /// Схема из `DENIED_SCHEMAS` (`net`, pg_net: внешние вызовы) — кто бы ни владел функцией.
    pub denied_schema: bool,
}

const USER_FUNCTIONS_SQL: &str = marked_sql!(
    "SELECT DISTINCT n.nspname::text, p.proname::text, lower(n.nspname::text) = ANY ($4::text[]) \
     FROM unnest($1::text[], $2::text[], $3::bool[]) AS f(schema, name, anywhere) \
     JOIN pg_proc p ON p.proname = f.name \
     JOIN pg_namespace n ON n.oid = p.pronamespace \
     WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') \
       AND (f.anywhere OR CASE WHEN f.schema = '' THEN n.nspname = ANY (current_schemas(false)) \
            ELSE n.nspname = f.schema END) \
       AND (lower(n.nspname::text) = ANY ($4::text[]) OR NOT EXISTS (SELECT 1 FROM pg_depend d \
         WHERE d.classid = 'pg_proc'::regclass AND d.objid = p.oid \
           AND d.refclassid = 'pg_extension'::regclass AND d.deptype = 'e')) \
     ORDER BY 3 DESC, 1, 2"
);

/// Шаг 4 §7.3 (с решением владельца P4b): функции запроса, которые `run_select` отвергает (`FuncHit`). Тело
/// пользовательской функции наш разбор не видит. Сверка осторожная: функция дерева разбора без схемы ищется во всех
/// схемах `search_path`, имя из плана — во всех схемах; одноимённая пользовательская перегрузка тоже даёт отказ.
pub async fn user_functions(
    c: &Client,
    tree: &[FuncRef],
    plan: &[String],
) -> AppResult<Vec<FuncHit>> {
    let l = Lookups::new(tree, plan);
    if l.is_empty() {
        return Ok(Vec::new());
    }
    let rows = c
        .query(
            USER_FUNCTIONS_SQL,
            &[&l.schemas, &l.names, &l.anywhere, &DENIED_SCHEMAS],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(rows
        .iter()
        .map(|r| FuncHit {
            schema: r.get(0),
            name: r.get(1),
            denied_schema: r.get(2),
        })
        .collect())
}

/// Отказ по найденным функциям (первая — из запрещённой схемы, если есть): схема `net` — `forbidden_function`, иначе —
/// `user_function` (спец. §7.3, шаг 4). `from_plan` — имя из плана: вызов может быть внутри представления.
pub fn function_rejection(hits: &[FuncHit], from_plan: bool) -> Option<ToolError> {
    let hit = hits
        .iter()
        .find(|h| h.denied_schema)
        .or_else(|| hits.first())?;
    let name = format!("{}.{}", hit.schema, hit.name);
    Some(if hit.denied_schema {
        err(
            "forbidden_function",
            format!(
                "Function {name} is not allowed: functions of schema {} call outside the database",
                hit.schema
            ),
        )
    } else if from_plan {
        err(
            "user_function",
            format!(
                "User-defined function {name} is not allowed in run_select: its body cannot be checked (the query calls it, possibly inside a view)"
            ),
        )
    } else {
        err(
            "user_function",
            format!(
                "User-defined function {name} is not allowed in run_select: its body cannot be checked"
            ),
        )
    })
}

/// Сверка с `pg_proc` и отказ, если есть что отвергнуть.
async fn reject_functions(c: &Client, tree: &[FuncRef], plan: &[String]) -> Result<(), ToolError> {
    let hits = user_functions(c, tree, plan).await.map_err(tool_error)?;
    match function_rejection(&hits, !plan.is_empty()) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

const ROLE_SQL: &str = marked_sql!(
    "SELECT current_user::text, r.rolsuper, ARRAY(SELECT b.rolname::text FROM pg_roles b \
     WHERE b.rolname::text = ANY ($1::text[]) AND pg_has_role(current_user, b.oid, 'USAGE')) \
     FROM pg_roles r WHERE r.rolname = current_user"
);

/// Факт роли (спец. §4.5) обновляет `list_connections.roleWarning`: опасные роли — `pg_advisor::readiness::DANGEROUS_ROLES`.
/// Читается на каждом вызове, прошедшем проверки, — одна строка каталога в той же транзакции; так «при первом
/// `run_select` сессии» выполняется без id сессии в бэкенде. Ошибка — только запись в лог: предупреждение не должно
/// ломать выборку.
async fn refresh_role_warning(st: &AppState, connection_id: &str, c: &Client) {
    let row = match c.query_one(ROLE_SQL, &[&&DANGEROUS_ROLES[..]]).await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(connection = %connection_id, "run_select role check failed: {e}");
            return;
        }
    };
    let (user, superuser, member_of): (String, bool, Vec<String>) =
        (row.get(0), row.get(1), row.get(2));
    let rights = dangerous_rights(superuser, &member_of);
    let warning =
        (!rights.is_empty()).then(|| crate::mcp::texts::role_warning_text(&user, &rights));
    remember_role_warning(st, connection_id, warning);
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

/// Сеанс сервера соединения вызова: отменять `FETCH` можно только его (pid переиспользуются).
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

/// Курсор по тексту модели: без метки `/* glafira */` (как EXPLAIN пользователя в P3) и расширенным протоколом — вторую
/// инструкцию сервер отверг бы при разборе (её уже отверг `prepare_one`).
fn declare_sql(sql: &str) -> String {
    format!("DECLARE {CURSOR} NO SCROLL CURSOR FOR {sql}")
}

fn fetch_sql(rows: usize) -> String {
    format!("{SQL_MARKER}FETCH {rows} FROM {CURSOR}")
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
    /// перестал ждать, а сервер, возможно, ещё выполняет `FETCH`.
    TimedOut { running: bool },
    /// Клиент отменил вызов; `FETCH`, возможно, ещё выполняется.
    Cancelled,
}

/// `FETCH` пачками в текстовом формате (простой протокол) до пределов `Collector`, срока `deadline` или отмены.
/// Перед каждой пачкой `statement_timeout` = остаток срока: 10 с — на всю выборку, а не на каждый `FETCH`.
async fn fetch(
    c: &Client,
    col: &mut Collector,
    deadline: Instant,
    ctx: &ObserveCtx,
) -> Result<Stop, ToolError> {
    loop {
        let want = col.want();
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
        let messages = tokio::select! {
            _ = ctx.cancel.cancelled() => return Ok(Stop::Cancelled),
            r = tokio::time::timeout(left + CLIENT_MARGIN, step) => match r {
                Err(_) => return Ok(Stop::TimedOut { running: true }),
                Ok(Err(e)) if e.code() == Some(&SqlState::QUERY_CANCELED) => {
                    return Ok(Stop::TimedOut { running: false });
                }
                Ok(Err(e)) => return Err(server_error(&e)),
                Ok(Ok(m)) => m,
            },
        };
        let mut got = 0;
        for m in &messages {
            if let SimpleQueryMessage::Row(r) = m {
                got += 1;
                let values: Vec<Option<&str>> = (0..r.len()).map(|i| r.get(i)).collect();
                if !col.accept(&values) {
                    return Ok(Stop::Done);
                }
            }
        }
        if got < want {
            return Ok(Stop::Done);
        }
    }
}

/// Отмена `FETCH`, оставшегося на сервере после отмены клиентом или истёкшего клиентского срока (уточнение 4 плана
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

/// Проверки на сервере до выборки (§7.3) и подготовка курсора: `prepare` под точкой сохранения; функции дерева
/// разбора — до пробы (планировщик может вычислить функцию с константами уже при EXPLAIN); проба VERBOSE-плана с
/// поиском запрещённых функций; функции плана; колонки; факт роли; сеанс для отмены; `DECLARE`. `original` — текст,
/// как его прислала модель (позиции ошибок — в нём), `sql` — нормализованный.
async fn prepare_cursor(
    st: &AppState,
    connection_id: &str,
    tx: &GuardTx,
    original: &str,
    sql: &str,
    tree: &[FuncRef],
) -> Result<(Vec<SelectColumn>, Backend), ToolError> {
    tx.prepare_one(original).await?;
    reject_functions(tx.client(), tree, &[]).await?;
    let raw = tx.probe(original, &Params::None).await?;
    reject_functions(tx.client(), &[], &plan_functions(&raw)).await?;
    let columns = describe(tx.client(), sql).await?;
    refresh_role_warning(st, connection_id, tx.client()).await;
    let backend = backend_of(tx.client()).await?;
    tx.client()
        .execute(declare_sql(sql).as_str(), &[])
        .await
        .map_err(|e| server_error(&e))?;
    Ok((columns, backend))
}

fn select_profile(st: &AppState, p: &RunSelectParams) -> Result<ConnectionProfile, ToolError> {
    // §4.3 шаг 3 (доступ) и шаг 4 (уровень data).
    let (profile, level) = check_connection(st, &p.connection_id)?;
    data_check(profile.llm_access, level)?;
    Ok(profile)
}

/// `run_select` (спец. §7.4): доступ и уровень `data`, SQL модели до соединения (`$N` не принимаются), узел по
/// умолчанию — реплика с наименьшим лагом (`pick_member`), база по умолчанию — база подключения (решение владельца
/// Q2), проверки на сервере (`prepare_cursor`), выборка до пределов (`fetch`). Отмена клиентом и истёкший срок отдают
/// уже собранные строки с `truncated: true`; срок без строк — `TIMEOUT`.
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
    let (columns, backend) =
        match prepare_cursor(st, &p.connection_id, &tx, &p.sql, &sql, &checked.functions).await {
            Ok(v) => v,
            Err(e) => {
                tx.finish().await;
                return Err(e);
            }
        };
    let max_rows = p.max_rows.unwrap_or(SELECT_DEFAULT_ROWS) as usize;
    let mut col = Collector::new(max_rows, rows_budget(&member, &columns));
    let stop = match fetch(tx.client(), &mut col, deadline, &ctx).await {
        Ok(s) => s,
        Err(e) => {
            tx.finish().await;
            return Err(e);
        }
    };
    if matches!(stop, Stop::Cancelled | Stop::TimedOut { running: true }) {
        // Сервер, возможно, ещё выполняет `FETCH`: соединение закрывается сразу (`ROLLBACK` ждал бы его), сеанс
        // отменяется отдельно.
        tokio::spawn(cancel_backend(
            st.clone(),
            p.connection_id.clone(),
            member.clone(),
            backend,
        ));
        drop(tx);
    } else {
        tx.finish().await;
    }
    if matches!(stop, Stop::TimedOut { .. }) && col.rows.is_empty() {
        return Err(err(
            "TIMEOUT",
            format!(
                "run_select did not return rows within {SELECT_TIMEOUT_S} s and was cancelled; narrow the query (WHERE, LIMIT, fewer joins)"
            ),
        ));
    }
    if stop != Stop::Done {
        col.truncated = true;
    }
    let row_count = col.rows.len() as u64;
    Ok(PgAnswer {
        body: PgOutput::Select(SelectOut {
            member,
            columns,
            rows: col.rows,
            row_count,
            truncated: col.truncated,
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
    use serde_json::json;

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

    /// Имена из дерева разбора ищутся в своей схеме или в `search_path`, имена из плана — в любой схеме (план печатает
    /// их без схемы, если она видна); точные повторы не уходят на сервер.
    #[test]
    fn lookups_scope_tree_names_and_spread_plan_names() {
        let tree = vec![
            FuncRef {
                schema: Some("shop".into()),
                name: "order_count".into(),
            },
            FuncRef {
                schema: None,
                name: "lower".into(),
            },
            FuncRef {
                schema: None,
                name: "lower".into(),
            },
        ];
        let plan = vec!["order_total".to_string(), "lower".to_string()];
        let l = Lookups::new(&tree, &plan);
        assert_eq!(l.schemas, ["shop", "", "", ""]);
        assert_eq!(l.names, ["order_count", "lower", "order_total", "lower"]);
        assert_eq!(
            l.anywhere,
            [false, false, true, true],
            "повтор дерева разбора убран, имя плана — своей областью"
        );
        let only_plan = Lookups::new(&[], &plan);
        assert_eq!(only_plan.anywhere, [true, true]);
        assert!(Lookups::new(&[], &[]).is_empty());
    }

    /// Все вызовы VERBOSE-плана (`calls_in_plan` с шаблоном `*`): функции из представлений и запись через атрибут
    /// (`r.f` — это `f(r)`) план печатает вызовами; имя в литерале — не вызов.
    #[test]
    fn plan_functions_are_every_call_of_the_verbose_plan() {
        let raw = json!([{"Plan": {
            "Node Type": "Seq Scan",
            "Relation Name": "orders",
            "Output": [
                "shop.order_total(o.*)",
                "lower(o.note)",
                "\"MixedCase\"(o.id)",
                "'evil(1)'::text"
            ],
            "Filter": "(o.id = ANY ('{1,2}'::integer[]))",
            "Plans": [{"Node Type": "Function Scan", "Function Call": "net.http_get('http://x'::text)"}]
        }}]);
        let names = plan_functions(&raw);
        for name in ["order_total", "lower", "MixedCase", "http_get"] {
            assert!(names.contains(&name.to_string()), "{name}: {names:?}");
        }
        assert!(!names.contains(&"evil".to_string()), "{names:?}");
    }

    fn hit(schema: &str, name: &str, denied_schema: bool) -> FuncHit {
        FuncHit {
            schema: schema.into(),
            name: name.into(),
            denied_schema,
        }
    }

    /// Решение по найденному в `pg_proc` (без базы): функция из схемы `net` (pg_net) — `forbidden_function`, кто бы ей ни
    /// владел; функция вне `pg_catalog`, не принадлежащая расширению, — `user_function` (её тело не проверить).
    /// Сервер отдаёт только такие функции (расширения вне `net` — нет), запрещённая схема — первой.
    #[test]
    fn found_functions_are_rejected_with_the_reason() {
        assert!(function_rejection(&[], false).is_none());
        let e = function_rejection(&[hit("shop", "order_count", false)], false).unwrap();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "user_function",
                "User-defined function shop.order_count is not allowed in run_select: its body cannot be checked"
            )
        );
        let e = function_rejection(&[hit("shop", "order_total", false)], true).unwrap();
        assert_eq!(e.code, "user_function");
        assert!(
            e.message
                .starts_with("User-defined function shop.order_total is not allowed")
                && e.message.contains("view"),
            "из плана — может быть внутри представления: {}",
            e.message
        );
        let e = function_rejection(
            &[hit("net", "http_get", true), hit("shop", "f", false)],
            true,
        )
        .unwrap();
        assert_eq!(e.code, "forbidden_function");
        assert!(e.message.contains("net.http_get"), "{}", e.message);
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
        for sql in [USER_FUNCTIONS_SQL, ROLE_SQL, BACKEND_SQL, CANCEL_SQL] {
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
