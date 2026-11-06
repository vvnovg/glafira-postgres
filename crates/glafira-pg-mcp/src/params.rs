//! Параметры инструментов P4a (спец. P4 §5.2). Doc-комментарии полей уходят в JSON Schema для LLM — только
//! по-английски (у структур их нет намеренно). `validate` сначала обнуляет пустые (или из пробелов) строки
//! необязательных строковых полей — они «не заданы», как опущенные; затем проверяет диапазоны (`invalid_argument`) и
//! заполняет значения по умолчанию: бэкенд получает `limit`, `durationSeconds`, `changedOnly`, `mode`, `sort` и
//! `windowS` (в режиме `window`) уже заполненными. `windowS` без `mode` выбирает режим `window`; при явном
//! `sinceReset` присланный `windowS` доходит до бэкенда как есть — признак подсказки, что окно не применено.

use glafira_mcp_core::ToolError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Окна профиля ожиданий и топа запросов, секунды (`diag::WINDOWS_S` приложения).
pub const WINDOWS_S: [u32; 3] = [300, 900, 3600];
pub const DEFAULT_WINDOW_S: u32 = 900;
pub const ACTIVITY_LIMIT_DEFAULT: u32 = 100;
pub const ACTIVITY_LIMIT_MAX: u32 = 500;
pub const TOP_LIMIT_DEFAULT: u32 = 10;
pub const TOP_LIMIT_MAX: u32 = 100;
pub const OBSERVE_DEFAULT_S: u32 = 30;
pub const OBSERVE_MIN_S: u32 = 10;
pub const OBSERVE_MAX_S: u32 = 50;
/// Страховочный запас наблюдения сверх `durationSeconds` (≤ 56 с — ниже таймаута 60 с клиентов).
pub const OBSERVE_GRACE_S: u32 = 6;
pub const SETTINGS_NAMES_MAX: usize = 200;
pub const HYPO_INDEXES_MAX: usize = 5;
/// Событий топологии в ответе `get_topology`.
pub const TOPOLOGY_EVENTS_MAX: usize = 50;
pub const MODE_SINCE_RESET: &str = "sinceReset";
pub const MODE_WINDOW: &str = "window";
pub const DEFAULT_SORT: &str = "totalTime";
/// Отказ правила «ровно один из `sql` и `queryId`» (общий с проверкой приложения `mcp::read::asked`).
pub const SQL_AND_QUERY_ID_BOTH: &str = "pass exactly one of sql and queryId: both were given";
pub const SQL_AND_QUERY_ID_NEITHER: &str = "pass exactly one of sql and queryId: neither was given";
/// Сортировки топа — варианты `pg_advisor::queries::QuerySort` в camelCase (крейт не зависит от pg-advisor).
pub const QUERY_SORTS: [&str; 14] = [
    "totalTime",
    "meanTime",
    "maxTime",
    "stddevTime",
    "calls",
    "rowsPerCall",
    "sharedRead",
    "hitRatio",
    "tempWritten",
    "walBytes",
    "ioTime",
    "planTime",
    "jitTime",
    "cpuTime",
];

fn in_range(
    v: Option<u32>,
    default: u32,
    min: u32,
    max: u32,
    name: &str,
) -> Result<u32, ToolError> {
    match v {
        None => Ok(default),
        Some(x) if (min..=max).contains(&x) => Ok(x),
        Some(x) => Err(ToolError::invalid_argument(format!(
            "{name} must be between {min} and {max}, got {x}"
        ))),
    }
}

fn window(v: Option<u32>, name: &str) -> Result<Option<u32>, ToolError> {
    match v {
        Some(w) if !WINDOWS_S.contains(&w) => Err(ToolError::invalid_argument(format!(
            "{name} must be 300, 900 or 3600, got {w}"
        ))),
        other => Ok(other),
    }
}

fn not_blank(v: &str, name: &str) -> Result<(), ToolError> {
    if v.trim().is_empty() {
        Err(ToolError::invalid_argument(format!(
            "{name} must not be empty"
        )))
    } else {
        Ok(())
    }
}

/// Необязательная строка: пустая или из пробелов считается «не задана» (`None`) — как опущенная (у `database`
/// приложение подставит базу подключения по умолчанию, `database` из `list_connections`). Модели присылают `""` в
/// необязательных полях (`queryId: ""` рядом с `sql`, `state: ""`); журнал хранит то, что прислала модель.
fn blank_to_none(v: &mut Option<String>) {
    if v.as_deref().is_some_and(|s| s.trim().is_empty()) {
        *v = None;
    }
}

/// `blank_to_none` для всех необязательных строковых полей структуры — до остальных проверок `validate`.
fn blanks_to_none<const N: usize>(fields: [&mut Option<String>; N]) {
    for f in fields {
        blank_to_none(f);
    }
}

fn count_in(len: usize, max: usize, name: &str) -> Result<(), ToolError> {
    if (1..=max).contains(&len) {
        Ok(())
    } else {
        Err(ToolError::invalid_argument(format!(
            "{name}: pass 1..={max} items, got {len}"
        )))
    }
}

fn query_id(v: &str) -> Result<(), ToolError> {
    v.trim().parse::<i64>().map(|_| ()).map_err(|_| {
        ToolError::invalid_argument(format!(
            "queryId must be a pg_stat_statements queryid (a 64-bit integer), got {v}"
        ))
    })
}

fn sort(v: Option<String>) -> Result<String, ToolError> {
    match v {
        None => Ok(DEFAULT_SORT.to_string()),
        Some(s) if QUERY_SORTS.contains(&s.as_str()) => Ok(s),
        Some(s) => Err(ToolError::invalid_argument(format!(
            "sort must be one of {}, got {s}",
            QUERY_SORTS.join(", ")
        ))),
    }
}

/// Ровно один источник запроса: непустой текст или queryId из pg_stat_statements. Пустые `sql` и `queryId` считаются
/// отсутствующими (`None`), `queryId` обрезается по краям — приложение получает уже нормализованную пару; отказ
/// называет случай (оба заданы или ни одного).
fn sql_or_query_id(sql: &mut Option<String>, qid: &mut Option<String>) -> Result<(), ToolError> {
    blanks_to_none([&mut *sql, &mut *qid]);
    if let Some(q) = qid.as_mut() {
        *q = q.trim().to_string();
    }
    match (sql.is_some(), qid.as_deref()) {
        (true, None) => Ok(()),
        (false, Some(q)) => query_id(q),
        (true, Some(_)) => Err(ToolError::invalid_argument(SQL_AND_QUERY_ID_BOTH)),
        (false, None) => Err(ToolError::invalid_argument(SQL_AND_QUERY_ID_NEITHER)),
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionParams {
    /// Connection id from list_connections.
    pub connection_id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunDiagnosticParams {
    /// Connection id from list_connections.
    pub connection_id: String,
    /// Diagnostic tool id from list_diagnostics, e.g. sessions, locks, horizon, replication, slots, waits, pulse, failover, health.
    pub tool: String,
    /// Member (node) name from get_topology. Without it the tool follows the leader (primary); cluster tools ignore it.
    #[serde(default)]
    pub member: Option<String>,
    /// Window of the waits tool: 300, 900 or 3600 seconds. Default: 900.
    #[serde(default)]
    pub window_s: Option<u32>,
    /// Parameters of the queries tool; prefer get_top_queries.
    #[serde(default)]
    pub queries: Option<Value>,
}

impl RunDiagnosticParams {
    pub fn validate(mut self) -> Result<Self, ToolError> {
        blank_to_none(&mut self.member);
        not_blank(&self.tool, "tool")?;
        window(self.window_s, "windowS")?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityParams {
    /// Connection id from list_connections.
    pub connection_id: String,
    /// Member (node) name; default: the leader (primary).
    #[serde(default)]
    pub member: Option<String>,
    /// Only sessions in this state: active, idle, idle in transaction, idle in transaction (aborted), fastpath function call, disabled.
    #[serde(default)]
    pub state: Option<String>,
    /// Only sessions connected to this database.
    #[serde(default)]
    pub database: Option<String>,
    /// Only sessions whose application_name contains this text (case-insensitive).
    #[serde(default)]
    pub application: Option<String>,
    /// Only sessions whose running query (active) or open transaction (idle in transaction) has lasted at least this many seconds; other states never match.
    #[serde(default)]
    pub min_duration_s: Option<u32>,
    /// Only sessions waiting on this wait event type: Lock, LWLock, IO, IPC, Client, Timeout, BufferPin, Activity, Extension.
    #[serde(default)]
    pub wait_event_type: Option<String>,
    /// Maximum number of sessions, 1..=500. Default: 100.
    #[serde(default)]
    pub limit: Option<u32>,
}

impl ActivityParams {
    pub fn validate(mut self) -> Result<Self, ToolError> {
        blanks_to_none([
            &mut self.member,
            &mut self.state,
            &mut self.database,
            &mut self.application,
            &mut self.wait_event_type,
        ]);
        self.limit = Some(in_range(
            self.limit,
            ACTIVITY_LIMIT_DEFAULT,
            1,
            ACTIVITY_LIMIT_MAX,
            "limit",
        )?);
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObserveActivityParams {
    /// Connection id from list_connections.
    pub connection_id: String,
    /// Member (node) name; default: the leader (primary).
    #[serde(default)]
    pub member: Option<String>,
    /// Observation window, 10..=50 seconds. Default: 30.
    #[serde(default)]
    pub duration_seconds: Option<u32>,
}

impl ObserveActivityParams {
    pub fn validate(mut self) -> Result<Self, ToolError> {
        blank_to_none(&mut self.member);
        self.duration_seconds = Some(in_range(
            self.duration_seconds,
            OBSERVE_DEFAULT_S,
            OBSERVE_MIN_S,
            OBSERVE_MAX_S,
            "durationSeconds",
        )?);
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TopQueriesParams {
    /// Connection id from list_connections.
    pub connection_id: String,
    /// Member (node) name; default: the leader (primary).
    #[serde(default)]
    pub member: Option<String>,
    /// sinceReset: totals since the last pg_stat_statements reset; window: the last windowS seconds from Glafira's buffer. Default: window when windowS is given, otherwise sinceReset.
    #[serde(default)]
    pub mode: Option<String>,
    /// Window for mode window: 300, 900 or 3600 seconds. Default: 900. windowS without mode selects mode window; in mode sinceReset it is ignored.
    #[serde(default)]
    pub window_s: Option<u32>,
    /// Sort key: totalTime (default), meanTime, maxTime, stddevTime, calls, rowsPerCall, sharedRead, hitRatio, tempWritten, walBytes, ioTime, planTime, jitTime, cpuTime.
    #[serde(default)]
    pub sort: Option<String>,
    /// Number of queries, 1..=100. Default: 10.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Only queries of this database.
    #[serde(default)]
    pub database: Option<String>,
    /// Case-insensitive substring of the query text. At the diagnostics access level it matches the masked text ($N in place of literals) within the top 200 queries by the chosen sort, so an empty result may mean the query is outside that window.
    #[serde(default)]
    pub search: Option<String>,
}

impl TopQueriesParams {
    pub fn validate(mut self) -> Result<Self, ToolError> {
        blanks_to_none([
            &mut self.member,
            &mut self.mode,
            &mut self.sort,
            &mut self.database,
            &mut self.search,
        ]);
        // `windowS` без `mode` — окно (модель спрашивает «за последние 15 минут»), иначе по умолчанию `sinceReset`.
        let mode = self.mode.take().unwrap_or_else(|| {
            if self.window_s.is_some() {
                MODE_WINDOW
            } else {
                MODE_SINCE_RESET
            }
            .to_string()
        });
        if mode != MODE_SINCE_RESET && mode != MODE_WINDOW {
            return Err(ToolError::invalid_argument(format!(
                "mode must be sinceReset or window, got {mode}"
            )));
        }
        let w = window(self.window_s, "windowS")?;
        // В `sinceReset` присланный `windowS` остаётся: бэкенд скажет в `hint`, что окно не применено.
        self.window_s = if mode == MODE_WINDOW {
            Some(w.unwrap_or(DEFAULT_WINDOW_S))
        } else {
            w
        };
        self.mode = Some(mode);
        self.sort = Some(sort(self.sort.take())?);
        self.limit = Some(in_range(
            self.limit,
            TOP_LIMIT_DEFAULT,
            1,
            TOP_LIMIT_MAX,
            "limit",
        )?);
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObserveQueriesParams {
    /// Connection id from list_connections.
    pub connection_id: String,
    /// Member (node) name; default: the leader (primary).
    #[serde(default)]
    pub member: Option<String>,
    /// Observation window, 10..=50 seconds. Default: 30.
    #[serde(default)]
    pub duration_seconds: Option<u32>,
    /// Sort key, as in get_top_queries. Default: totalTime.
    #[serde(default)]
    pub sort: Option<String>,
    /// Number of queries, 1..=100. Default: 10.
    #[serde(default)]
    pub limit: Option<u32>,
}

impl ObserveQueriesParams {
    pub fn validate(mut self) -> Result<Self, ToolError> {
        blanks_to_none([&mut self.member, &mut self.sort]);
        self.duration_seconds = Some(in_range(
            self.duration_seconds,
            OBSERVE_DEFAULT_S,
            OBSERVE_MIN_S,
            OBSERVE_MAX_S,
            "durationSeconds",
        )?);
        self.sort = Some(sort(self.sort.take())?);
        self.limit = Some(in_range(
            self.limit,
            TOP_LIMIT_DEFAULT,
            1,
            TOP_LIMIT_MAX,
            "limit",
        )?);
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryDetailsParams {
    /// Connection id from list_connections.
    pub connection_id: String,
    /// Member (node) name; default: the leader (primary).
    #[serde(default)]
    pub member: Option<String>,
    /// Database of the query (datname from get_top_queries); default: the connection's database (`database` in list_connections).
    #[serde(default)]
    pub database: Option<String>,
    /// queryid from get_top_queries, as a string.
    pub query_id: String,
}

impl QueryDetailsParams {
    pub fn validate(mut self) -> Result<Self, ToolError> {
        blanks_to_none([&mut self.member, &mut self.database]);
        query_id(&self.query_id)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TableDetailsParams {
    /// Connection id from list_connections.
    pub connection_id: String,
    /// Database of the table; default: the connection's database (`database` in list_connections).
    #[serde(default)]
    pub database: Option<String>,
    /// Schema name, e.g. public.
    pub schema: String,
    /// Table name without the schema.
    pub table: String,
}

impl TableDetailsParams {
    pub fn validate(mut self) -> Result<Self, ToolError> {
        blank_to_none(&mut self.database);
        not_blank(&self.schema, "schema")?;
        not_blank(&self.table, "table")?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsParams {
    /// Connection id from list_connections.
    pub connection_id: String,
    /// Member (node) name; default: the leader (primary).
    #[serde(default)]
    pub member: Option<String>,
    /// Return exactly these settings (up to 200), changed or not.
    #[serde(default)]
    pub names: Option<Vec<String>>,
    /// Only settings changed from their built-in defaults (ignored when names is set). Default: true.
    #[serde(default)]
    pub changed_only: Option<bool>,
}

impl SettingsParams {
    pub fn validate(mut self) -> Result<Self, ToolError> {
        blank_to_none(&mut self.member);
        if let Some(names) = &self.names {
            count_in(names.len(), SETTINGS_NAMES_MAX, "names")?;
        }
        self.changed_only = Some(self.changed_only.unwrap_or(true));
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExplainParams {
    /// Connection id from list_connections.
    pub connection_id: String,
    /// Member (node) name; default: the leader (primary).
    #[serde(default)]
    pub member: Option<String>,
    /// Database to plan the query in; default: the connection's database (`database` in list_connections).
    #[serde(default)]
    pub database: Option<String>,
    /// One SELECT, INSERT, UPDATE, DELETE or MERGE statement; it is planned, not executed. Pass sql or queryId.
    #[serde(default)]
    pub sql: Option<String>,
    /// queryid from get_top_queries: its normalized text from pg_stat_statements is planned. Pass sql or queryId.
    #[serde(default)]
    pub query_id: Option<String>,
    /// Values for $1, $2… in order, as text; null is SQL NULL.
    #[serde(default)]
    pub params: Option<Vec<Option<String>>>,
    /// Build a generic plan without parameter values (EXPLAIN GENERIC_PLAN, PostgreSQL 16+). Used by default for queryId without params.
    #[serde(default)]
    pub generic_plan: Option<bool>,
}

impl ExplainParams {
    pub fn validate(mut self) -> Result<Self, ToolError> {
        blanks_to_none([&mut self.member, &mut self.database]);
        sql_or_query_id(&mut self.sql, &mut self.query_id)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HypoIndexParams {
    /// Connection id from list_connections.
    pub connection_id: String,
    /// Member (node) name; default: the leader (primary).
    #[serde(default)]
    pub member: Option<String>,
    /// Database to plan the query in; default: the connection's database (`database` in list_connections).
    #[serde(default)]
    pub database: Option<String>,
    /// The query to check, as in explain_query. Pass sql or queryId.
    #[serde(default)]
    pub sql: Option<String>,
    /// queryid from get_top_queries. Pass sql or queryId.
    #[serde(default)]
    pub query_id: Option<String>,
    /// Values for $1, $2… in order, as text; null is SQL NULL.
    #[serde(default)]
    pub params: Option<Vec<Option<String>>>,
    /// 1..=5 statements CREATE INDEX ON <table> ... (no CONCURRENTLY, no UNIQUE; an index name, if given, is ignored); they are created only as hypothetical hypopg indexes.
    pub indexes: Vec<String>,
}

impl HypoIndexParams {
    pub fn validate(mut self) -> Result<Self, ToolError> {
        blanks_to_none([&mut self.member, &mut self.database]);
        sql_or_query_id(&mut self.sql, &mut self.query_id)?;
        count_in(self.indexes.len(), HYPO_INDEXES_MAX, "indexes")?;
        for ddl in &self.indexes {
            not_blank(ddl, "indexes")?;
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::json;

    fn parse<T: DeserializeOwned>(v: Value) -> T {
        serde_json::from_value(v).unwrap()
    }

    fn top(v: Value) -> Result<TopQueriesParams, ToolError> {
        parse::<TopQueriesParams>(v).validate()
    }

    /// E1: `windowS` без `mode` выбирает окно; явный `sinceReset` с `windowS` остаётся `sinceReset`, а `windowS` доходит до
    /// бэкенда — признак подсказки, что окно не применено.
    #[test]
    fn window_s_without_mode_selects_the_window() {
        let p = top(json!({"connectionId": "c", "windowS": 300})).unwrap();
        assert_eq!(
            (p.mode.as_deref(), p.window_s),
            (Some(MODE_WINDOW), Some(300))
        );
        let p = top(json!({"connectionId": "c", "mode": " ", "windowS": 3600})).unwrap();
        assert_eq!(
            (p.mode.as_deref(), p.window_s),
            (Some(MODE_WINDOW), Some(3600)),
            "пустой mode — не задан"
        );
        let p = top(json!({"connectionId": "c"})).unwrap();
        assert_eq!(
            (p.mode.as_deref(), p.window_s),
            (Some(MODE_SINCE_RESET), None)
        );
        let p = top(json!({"connectionId": "c", "mode": "window"})).unwrap();
        assert_eq!(
            (p.mode.as_deref(), p.window_s),
            (Some(MODE_WINDOW), Some(DEFAULT_WINDOW_S))
        );
        let p = top(json!({"connectionId": "c", "mode": "sinceReset", "windowS": 900})).unwrap();
        assert_eq!(
            (p.mode.as_deref(), p.window_s),
            (Some(MODE_SINCE_RESET), Some(900))
        );
        let e = top(json!({"connectionId": "c", "windowS": 60})).unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "invalid_argument",
                "windowS must be 300, 900 or 3600, got 60"
            )
        );
        let e = top(json!({"connectionId": "c", "mode": "sinceReset", "windowS": 60})).unwrap_err();
        assert_eq!(e.message, "windowS must be 300, 900 or 3600, got 60");
    }

    /// E2: пустая или из пробелов строка необязательного параметра — «не задан» во всех структурах; значения не
    /// обрезаются.
    #[test]
    fn blank_optional_strings_are_not_given() {
        let r = parse::<RunDiagnosticParams>(
            json!({"connectionId": "c", "tool": "sessions", "member": " "}),
        )
        .validate()
        .unwrap();
        assert_eq!(r.member, None);

        let a = parse::<ActivityParams>(json!({
            "connectionId": "c", "member": "", "state": "", "database": " ", "application": "\t",
            "waitEventType": " \n "
        }))
        .validate()
        .unwrap();
        assert_eq!(
            (
                a.member,
                a.state,
                a.database,
                a.application,
                a.wait_event_type
            ),
            (None, None, None, None, None)
        );
        let a =
            parse::<ActivityParams>(json!({"connectionId": "c", "state": "idle in transaction"}))
                .validate()
                .unwrap();
        assert_eq!(a.state.as_deref(), Some("idle in transaction"));

        let o = parse::<ObserveActivityParams>(json!({"connectionId": "c", "member": ""}))
            .validate()
            .unwrap();
        assert_eq!(o.member, None);

        let t = top(json!({
            "connectionId": "c", "member": "", "mode": "", "sort": " ", "database": "", "search": "  "
        }))
        .unwrap();
        assert_eq!(
            (
                t.member,
                t.mode.as_deref(),
                t.sort.as_deref(),
                t.database,
                t.search
            ),
            (None, Some(MODE_SINCE_RESET), Some(DEFAULT_SORT), None, None)
        );

        let q =
            parse::<ObserveQueriesParams>(json!({"connectionId": "c", "member": " ", "sort": ""}))
                .validate()
                .unwrap();
        assert_eq!((q.member, q.sort.as_deref()), (None, Some(DEFAULT_SORT)));

        let d = parse::<QueryDetailsParams>(
            json!({"connectionId": "c", "member": "", "database": "", "queryId": "42"}),
        )
        .validate()
        .unwrap();
        assert_eq!((d.member, d.database), (None, None));

        let t = parse::<TableDetailsParams>(
            json!({"connectionId": "c", "database": " ", "schema": "public", "table": "t"}),
        )
        .validate()
        .unwrap();
        assert_eq!(t.database, None);

        let s = parse::<SettingsParams>(json!({"connectionId": "c", "member": ""}))
            .validate()
            .unwrap();
        assert_eq!(s.member, None);

        let x = parse::<ExplainParams>(json!({
            "connectionId": "c", "member": " ", "database": "", "sql": "SELECT 1", "queryId": ""
        }))
        .validate()
        .unwrap();
        assert_eq!(
            (x.member, x.database, x.sql.as_deref(), x.query_id),
            (None, None, Some("SELECT 1"), None)
        );

        let h = parse::<HypoIndexParams>(json!({
            "connectionId": "c", "member": "", "database": " ", "sql": "", "queryId": " 7 ",
            "indexes": ["CREATE INDEX ON t (a)"]
        }))
        .validate()
        .unwrap();
        assert_eq!(
            (h.member, h.database, h.sql, h.query_id.as_deref()),
            (None, None, None, Some("7"))
        );
    }

    /// E2: ровно один из `sql` и `queryId` после обнуления пустых; сообщение называет случай.
    #[test]
    fn sql_or_query_id_names_the_case() {
        let explain = |v: Value| parse::<ExplainParams>(v).validate();
        assert!(explain(json!({"connectionId": "c", "sql": "SELECT 1", "queryId": " "})).is_ok());
        for (v, want) in [
            (
                json!({"connectionId": "c", "sql": "SELECT 1", "queryId": "42"}),
                "pass exactly one of sql and queryId: both were given",
            ),
            (
                json!({"connectionId": "c", "sql": "", "queryId": ""}),
                "pass exactly one of sql and queryId: neither was given",
            ),
            (
                json!({"connectionId": "c"}),
                "pass exactly one of sql and queryId: neither was given",
            ),
        ] {
            let e = explain(v.clone()).unwrap_err();
            assert_eq!(
                (e.code.as_str(), e.message.as_str()),
                ("invalid_argument", want),
                "{v}"
            );
        }
        let e = parse::<HypoIndexParams>(json!({
            "connectionId": "c", "sql": "SELECT 1", "queryId": "1", "indexes": ["CREATE INDEX ON t (a)"]
        }))
        .validate()
        .unwrap_err();
        assert_eq!(
            e.message,
            "pass exactly one of sql and queryId: both were given"
        );
    }

    /// Описания параметров для модели (JSON Schema): `windowS` без `mode` выбирает окно, имя индекса гипотезы
    /// игнорируется.
    #[test]
    fn schema_describes_the_window_and_the_index_name() {
        let top = serde_json::to_string(&schemars::schema_for!(TopQueriesParams)).unwrap();
        assert!(
            top.contains("windowS without mode selects mode window"),
            "{top}"
        );
        let hypo = serde_json::to_string(&schemars::schema_for!(HypoIndexParams)).unwrap();
        assert!(
            hypo.contains("an index name, if given, is ignored"),
            "{hypo}"
        );
    }
}
