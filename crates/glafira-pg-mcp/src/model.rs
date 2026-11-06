//! Контракт крейта с приложением (спец. P4 §3.2): операции, ответы и трейт [`PgBackend`]. Типы ответов — стабильный
//! контракт для LLM, а не DTO интерфейса; исключения — `data` отчёта инструмента, карточка запроса и описание таблицы:
//! они идут JSON-сериализацией P2/P3 (Уточнение 1 плана P4a), их изменение — изменение контракта.

use std::sync::Arc;
use std::time::Duration;

use glafira_mcp_core::{BoxFuture, Gate, ToolError};
use serde::Serialize;
use serde_json::Value;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::params::{
    ActivityParams, ConnectionParams, ExplainParams, HypoIndexParams, OBSERVE_GRACE_S,
    ObserveActivityParams, ObserveQueriesParams, QueryDetailsParams, RunDiagnosticParams,
    RunSelectParams, SettingsParams, TableDetailsParams, TopQueriesParams,
};

/// Подсказка `more` после подсказки `first` (поле `hint` отчёта): у каждой части — точка в конце, если её нет, между
/// ними — пробел, чтобы модель видела границу предложений. Тексты подсказок пишутся без точки; одна подсказка идёт как
/// есть.
pub fn join_hints(first: &str, more: &str) -> String {
    fn sentence(s: &str) -> String {
        let s = s.trim_end();
        if s.ends_with(['.', '!', '?']) {
            s.to_string()
        } else {
            format!("{s}.")
        }
    }
    format!("{} {}", sentence(first), sentence(more))
}

/// Источник данных шлюза. Реализация приложения на каждый вызов проверяет доступ к подключению (§4.3), открывает его
/// при необходимости, маскирует литералы на уровне `diagnostics` и ставит английские тексты находок.
pub trait PgBackend: Gate {
    /// Операция чтения; параметры уже проверены и нормализованы (`validate`).
    fn read(&self, op: PgRead) -> BoxFuture<'_, Result<PgAnswer, ToolError>>;
    /// Наблюдение `observe_*`: до `ctx.duration`, `ctx.progress` после каждого образца, по `ctx.cancel` — досрочно с
    /// уже собранным.
    fn observe(&self, op: PgObserve, ctx: ObserveCtx)
    -> BoxFuture<'_, Result<PgAnswer, ToolError>>;
    /// P4b: `run_select` на уровне `data` (§7.4). Крейт держит слот базы на весь вызов; `ctx.duration` — срок выборки
    /// (`statement_timeout` 10 с), `ctx.cancel` — отмена клиентом: приложение отдаёт уже собранные строки.
    fn select(
        &self,
        _op: SelectOp,
        _ctx: ObserveCtx,
    ) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        Box::pin(async {
            Err(ToolError::new(
                "not_supported",
                "run_select is not supported by this Glafira build",
            ))
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PgRead {
    ListConnections,
    Topology(ConnectionParams),
    HealthSummary(ConnectionParams),
    ListDiagnostics,
    RunDiagnostic(RunDiagnosticParams),
    Activity(ActivityParams),
    TopQueries(TopQueriesParams),
    QueryDetails(QueryDetailsParams),
    TableDetails(TableDetailsParams),
    Settings(SettingsParams),
    Explain(ExplainParams),
    HypoIndex(HypoIndexParams),
}

impl PgRead {
    pub fn connection_id(&self) -> Option<&str> {
        match self {
            PgRead::ListConnections | PgRead::ListDiagnostics => None,
            PgRead::Topology(p) | PgRead::HealthSummary(p) => Some(&p.connection_id),
            PgRead::RunDiagnostic(p) => Some(&p.connection_id),
            PgRead::Activity(p) => Some(&p.connection_id),
            PgRead::TopQueries(p) => Some(&p.connection_id),
            PgRead::QueryDetails(p) => Some(&p.connection_id),
            PgRead::TableDetails(p) => Some(&p.connection_id),
            PgRead::Settings(p) => Some(&p.connection_id),
            PgRead::Explain(p) => Some(&p.connection_id),
            PgRead::HypoIndex(p) => Some(&p.connection_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PgObserve {
    Activity(ObserveActivityParams),
    TopQueries(ObserveQueriesParams),
}

impl PgObserve {
    pub fn connection_id(&self) -> &str {
        match self {
            PgObserve::Activity(p) => &p.connection_id,
            PgObserve::TopQueries(p) => &p.connection_id,
        }
    }
}

/// Операция `run_select` (P4b): параметры уже проверены крейтом (`RunSelectParams::validate`).
#[derive(Debug, Clone, PartialEq)]
pub struct SelectOp(pub RunSelectParams);

/// Ход наблюдения: собрано образцов и всего (`notifications/progress`, если клиент передал `progressToken`).
pub type ProgressFn = Arc<dyn Fn(u32, u32) + Send + Sync>;

/// Окно наблюдения, отмена (клиент отменил вызов или вышел страховочный срок) и ход.
#[derive(Clone)]
pub struct ObserveCtx {
    pub duration: Duration,
    /// Начало вызова (создание контекста): окно отсчитывается от него — подготовка приложения (открытие подключения,
    /// туннель ssh, первое чтение) входит в `duration`, а не добавляется к нему.
    pub started: Instant,
    /// Страховочный срок ядра (`started` + `durationSeconds` + `OBSERVE_GRACE_S` единиц): в этот момент вызов
    /// обрывается с `TIMEOUT`, и собранное теряется — последний шаг приложения должен закончиться раньше.
    pub deadline: Instant,
    pub cancel: CancellationToken,
    pub progress: ProgressFn,
}

impl ObserveCtx {
    /// Контекст без хода и без внешней отмены (тесты приложения): начало — сейчас, срок — через `duration` и
    /// `OBSERVE_GRACE_S` секунд, как у ядра в обычной единице времени.
    pub fn new(duration: Duration) -> Self {
        let started = Instant::now();
        Self {
            duration,
            started,
            deadline: started + duration + Duration::from_secs(u64::from(OBSERVE_GRACE_S)),
            cancel: CancellationToken::new(),
            progress: Arc::new(|_, _| {}),
        }
    }
}

/// Ответ бэкенда и счётчики для журнала: замаскировано литералов, строк (P4b `run_select`).
#[derive(Debug, Clone, PartialEq)]
pub struct PgAnswer {
    pub body: PgOutput,
    pub masked: Option<u64>,
    pub rows: Option<u64>,
}

impl PgAnswer {
    pub fn new(body: PgOutput) -> Self {
        Self {
            body,
            masked: None,
            rows: None,
        }
    }

    pub fn with_masked(mut self, n: u64) -> Self {
        self.masked = Some(n);
        self
    }

    pub fn with_rows(mut self, n: u64) -> Self {
        self.rows = Some(n);
        self
    }
}

/// Варианты разного размера живут недолго и один на вызов: `Box` не нужен (как `DiagData` приложения).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum PgOutput {
    Connections(Vec<ConnectionInfo>),
    Topology(TopologyInfo),
    HealthSummary(HealthSummary),
    Diagnostics(Vec<DiagToolInfo>),
    Report(Report),
    Activity(ActivityOut),
    QueryDetails(Value),
    Table(TableOut),
    Settings(Vec<SettingOut>),
    Explain(ExplainOut),
    Hypo(HypoOut),
    ObservedActivity(ObservedActivity),
    ObservedQueries(Report),
    Select(SelectOut),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub id: String,
    pub name: String,
    /// `local`, `dev`, `test`, `stage` или `prod`.
    pub environment: String,
    /// `server`, `hosts` или `patroni`.
    pub kind: String,
    pub folder_path: Vec<String>,
    /// База подключения по умолчанию (`profile.pg.database`): её берут `get_query_details`, `get_table_details`,
    /// `explain_query`, `hypothetical_index` и `run_select` без `database`. Хосты, порты и пользователь не отдаются (§6.2).
    pub database: String,
    pub read_only: bool,
    /// Действующий уровень с учётом политики: `diagnostics` или `data`.
    pub llm_access: String,
    pub connected: bool,
    /// Предупреждение о роли (P4b, §4.5), английский текст.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role_warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberInfo {
    pub name: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub host: String,
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeline: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lag_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_restart: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nofailover: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatroniInfo {
    pub paused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_switchover: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_restart: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyInfo {
    pub kind: String,
    pub members: Vec<MemberInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patroni: Option<PatroniInfo>,
    /// События журнала топологии за сессию приложения, новые первыми; крейт отдаёт первые 50.
    pub events: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingOut {
    pub id: String,
    /// `info`, `warning` или `critical`.
    pub severity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Value>,
    pub params: Value,
    pub title: String,
    pub fix: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnavailableOut {
    /// Инструмент — у `get_health_summary`; у отчёта одного инструмента пусто.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub part: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    /// Причина по-английски.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthSummary {
    pub pulse: Value,
    pub findings: Vec<FindingOut>,
    pub unavailable: Vec<UnavailableOut>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagToolInfo {
    pub id: String,
    pub title: String,
    /// `node` или `cluster`.
    pub scope: String,
    pub patroni_only: bool,
    pub params: Vec<String>,
}

/// Конверт `DiagReport` P2 для модели; `data` — JSON-сериализация данных инструмента.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    pub follows_leader: bool,
    pub observed_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_version_num: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_recovery: Option<bool>,
    pub data: Value,
    pub findings: Vec<FindingOut>,
    pub unavailable: Vec<UnavailableOut>,
    /// Массивы в `data` урезаны до 100 КБ (§10.1); ставит крейт.
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityOut {
    /// Сессий после фильтров до `limit`.
    pub total: u64,
    pub sessions: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnStat {
    pub column: String,
    pub null_frac: f64,
    pub avg_width: i32,
    pub n_distinct: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation: Option<f64>,
    /// Только на уровне `data` (P4b).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub most_common_vals: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub most_common_freqs: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub histogram_bounds: Option<Value>,
}

/// `TableDetails` P1 (`table`, JSON-сериализация) и статистика колонок из `pg_stats`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableOut {
    pub table: Value,
    pub column_stats: Vec<ColumnStat>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingOut {
    pub name: String,
    pub setting: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub source: String,
    pub context: String,
    pub pending_restart: bool,
    /// Значение вычищено (секреты, §6.2); выводится только при `true`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSummaryOut {
    pub total_cost: f64,
    pub plan_rows: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planning_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainOut {
    pub member: String,
    /// План текстом в виде psql (`plan::text_out::to_text`).
    pub plan_text: String,
    pub summary: PlanSummaryOut,
    pub recommendations: Vec<FindingOut>,
    pub tables: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HypoIndexOut {
    pub ddl: String,
    pub used: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_size_bytes: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HypoOut {
    pub cost_before: f64,
    pub cost_after: f64,
    pub indexes: Vec<HypoIndexOut>,
    pub plan_text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedActivity {
    pub started_at_ms: i64,
    pub duration_ms: u64,
    pub samples: u32,
    pub waits: Vec<Value>,
    pub top_queries: Vec<Value>,
    pub appeared: Vec<Value>,
    pub disappeared: Vec<Value>,
    pub peak_sessions: u32,
    pub peak_lock_waits: u32,
    pub summary: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectColumn {
    pub name: String,
    /// Имя типа PostgreSQL (`int4`, `text`, `timestamptz`…).
    #[serde(rename = "type")]
    pub type_name: String,
}

/// Ответ `run_select` (спец. §7.4): значения — текст PostgreSQL или `null`; длиннее 2 КБ — с пометкой `…[truncated]`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectOut {
    pub member: String,
    pub columns: Vec<SelectColumn>,
    pub rows: Vec<Vec<Option<String>>>,
    pub row_count: u64,
    /// Строк было больше, чем отдано: предел строк, байт, времени или отмена.
    pub truncated: bool,
    /// Значений, обрезанных до 2 КБ.
    pub truncated_values: u64,
    pub ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Подсказки склеиваются границей предложения: у каждой части — точка в конце (если её не было), между ними — пробел.
    #[test]
    fn hints_are_joined_as_sentences() {
        assert_eq!(join_hints("First", "Second"), "First. Second.");
        assert_eq!(join_hints("First.", "Second?"), "First. Second?");
        assert_eq!(join_hints("First. ", "Second"), "First. Second.");
    }
}
