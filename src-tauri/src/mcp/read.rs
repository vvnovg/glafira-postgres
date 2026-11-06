//! Операции чтения MCP в приложении (спец. P4 §3.2, §5.2): `AppBackend` (`backend.rs`) отдаёт сюда каждый вызов
//! `PgBackend`. На вызов: доступ к подключению (§4.3 шаг 3), открытие закрытого подключения тем же путём, что щелчок в
//! дереве, существующая функция приложения (`diag::run`, карточка, каталог) на общих пулах, маскирование на уровне
//! `diagnostics` (§6.1) и английские тексты находок. SQL модели (EXPLAIN, hypopg) — не на пулах: своё соединение на
//! вызов со слоями §7.3 (`mcp::guard`). Наблюдение — `mcp::observe`. Диапазоны параметров, усечение ответа и журнал — в
//! крейте шлюза.

use std::time::Duration;

use futures_util::future::join_all;
use glafira_mcp_core::error::ToolError;
use glafira_pg_mcp::model::{
    ActivityOut, ColumnStat, ConnectionInfo, ExplainOut, FindingOut, HealthSummary, HypoIndexOut,
    HypoOut, MemberInfo, ObserveCtx, PatroniInfo, PgAnswer, PgObserve, PgOutput, PgRead,
    PlanSummaryOut, Report, SettingOut, TableOut, TopologyInfo, UnavailableOut, join_hints,
};
use glafira_pg_mcp::params::{
    ACTIVITY_LIMIT_DEFAULT, ActivityParams, ExplainParams, HypoIndexParams, MODE_SINCE_RESET,
    MODE_WINDOW, QueryDetailsParams, RunDiagnosticParams, SettingsParams, TOP_LIMIT_DEFAULT,
    TOPOLOGY_EVENTS_MAX, TableDetailsParams, TopQueriesParams, default_top_mode,
};
use pg_advisor::ash::WaitsData;
use pg_advisor::findings::FindingTarget;
use pg_advisor::plan::json::from_json;
use pg_advisor::plan::relations;
use pg_advisor::plan::text_out::to_text;
use pg_advisor::queries::{QuerySort, QueryStat};
use pg_advisor::rules::sessions::SessionRow;
use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio_postgres::{Client, Row};

use super::access::{
    check_connection, effective_access, ensure_open, err, invalid_argument, masks, model_app_error,
    open_for_llm, tool_error,
};
use super::guard::{self, sql_error};
use super::mask::{
    MAX_QUERY_BYTES, cut_text, has_texts, mask_opt, mask_plan_value, mask_query_detail,
    mask_recommendation, mask_report,
};
use super::model_sql::{asked, check_index_ddls, resolve_database, sql_source};
use super::observe;
use super::secrets::redact_setting;
use super::texts::{diag_catalog, finding_out, recommendation_out, tool_id};
use crate::catalog::ddl::quote_ident;
use crate::catalog::table::{self as catalog_table, Column, RelationRef, TableDetails};
use crate::commands::catalog::{catalog_limit, on_catalog_node_within};
use crate::conn::errors::from_pg;
use crate::conn::profile::{ConnectionKind, LlmAccess};
use crate::diag::caps::V16;
use crate::diag::queries::{LIMITS, QueriesMode, QueriesParams};
use crate::diag::query_detail::{self, QueryDetail};
use crate::diag::{
    self, DEFAULT_WINDOW_S, DiagData, DiagReport, DiagRequest, Tool, Unavailable, on_member,
};
use crate::error::{AppError, AppResult};
use crate::explain::Params;
use crate::explain::hypopg::{self, HypoManyRequest, HypoManyResult};
use crate::explain::params::{count_params, normalize};
use crate::sampler::statements::HistoryPoint;
use crate::state::AppState;
use crate::storage::connections::ConnectionFolder;
use crate::topology::journal::TopologyEvent;
use crate::topology::model::{Member, Topology};
use crate::topology::runtime::{ConnState, not_connected};

/// События журнала топологии в ответе `get_topology` (спец. §5.2), новые первыми.
pub const MAX_EVENTS: usize = TOPOLOGY_EVENTS_MAX;
/// Дедлайн `get_health_summary` (спец. §5.2): не уложившийся инструмент — в `unavailable`.
pub const HEALTH_DEADLINE: Duration = Duration::from_secs(20);
/// Быстрые узловые инструменты сводки; у Patroni к ним — `failover` и `health`.
const HEALTH_TOOLS: [Tool; 5] = [
    Tool::Sessions,
    Tool::Locks,
    Tool::Horizon,
    Tool::Replication,
    Tool::Slots,
];
/// Строк `get_activity` по умолчанию (спец. §5.2); крейт шлюза подставляет его сам, здесь — для вызова без `validate`.
pub const ACTIVITY_LIMIT: u32 = ACTIVITY_LIMIT_DEFAULT;
/// Строк `get_top_queries` и `observe_top_queries` по умолчанию (спец. §5.2).
pub(crate) const TOP_LIMIT: u32 = TOP_LIMIT_DEFAULT;
/// Истории pgss узла для окна ещё нет: подсказка модели (спец. §5.2).
const NO_WINDOW_HINT: &str = "Glafira has no pg_stat_statements history for this node yet (it is collected while the Queries tab or a query card is open in Glafira): call observe_top_queries for a fresh window or use mode sinceReset";
/// Модель прислала `windowS` при явном `sinceReset` (E1): окно не применено — подсказка модели.
const SINCE_RESET_WINDOW_HINT: &str = "windowS is ignored in mode sinceReset: these are totals since the last pg_stat_statements reset (data.pgss.statsResetMs); use mode window for a window";
/// Имён баз в отказе `not_found` фильтра `database` у `get_top_queries` (E3); остальные — «…and N more».
pub const MAX_DATABASE_NAMES: usize = 50;
/// Значение `pg_stats` длиннее — обрезается (спец. §5.2).
pub const MAX_STATS_VALUE_BYTES: usize = 2048;
const TRUNCATED_MARK: &str = "…[truncated]";
/// Выполняющихся экземпляров в карточке `get_query_details` — самые долгие; полное число — в `runningTotal`
/// (запрос карточки их не ограничивает, а предел ответа — 100 КБ, спец. §10.1).
pub const MAX_RUNNING: usize = 20;
/// Партиций в `get_table_details` (список и строка `-- partitions:` в DDL); полное число — в
/// `partitioning.partitionsTotal`.
pub const MAX_PARTITIONS: usize = 200;
/// Текст запроса в строке топа для модели (смоук P4b, G2) — первые 500 символов с `textTruncated: true`; полный текст —
/// в `get_query_details`.
pub const TOP_TEXT_CHARS: usize = 500;
/// Счётчики строки pgss (поля `Counters` без `calls`, `totalExecMs` и `rows`), которые модели не отдаются, если равны 0
/// (смоук P4b, G2; описания `get_top_queries` и `get_query_details` говорят об этом).
const ZERO_OMITTED_COUNTERS: [&str; 26] = [
    "plans",
    "totalPlanMs",
    "sharedHit",
    "sharedRead",
    "sharedDirtied",
    "sharedWritten",
    "localHit",
    "localRead",
    "localDirtied",
    "localWritten",
    "tempRead",
    "tempWritten",
    "ioReadMs",
    "ioWriteMs",
    "tempIoMs",
    "walRecords",
    "walFpi",
    "walBytes",
    "jitFunctions",
    "jitMs",
    "parallelPlanned",
    "parallelLaunched",
    "cpuUserMs",
    "cpuSysMs",
    "diskReadBytes",
    "diskWriteBytes",
];
/// Точек истории в карточке запроса для модели (смоук P4b, G4): соседние минутные точки сливаются.
pub const MAX_HISTORY_POINTS: usize = 12;
/// Профиль ожиданий без единого сэмпла и без тиков сэмплера в окне (смоук P4b, G1).
const NO_WAIT_SAMPLES_HINT: &str = "The wait sampler collected no samples in this window (it runs while the wait profile or the sessions of this node are open in Glafira, or during observe_activity): call observe_activity for a live sample";

/// Операция чтения.
pub async fn read_op(st: &AppState, op: PgRead) -> Result<PgAnswer, ToolError> {
    match op {
        PgRead::ListConnections => Ok(answer(PgOutput::Connections(list_connections(st)))),
        PgRead::ListDiagnostics => Ok(answer(PgOutput::Diagnostics(diag_catalog()))),
        PgRead::Topology(p) => topology(st, &p.connection_id).await,
        PgRead::HealthSummary(p) => health_summary(st, &p.connection_id).await,
        PgRead::RunDiagnostic(p) => run_diagnostic(st, p).await,
        PgRead::Activity(p) => activity(st, p).await,
        PgRead::TopQueries(p) => top_queries(st, p).await,
        PgRead::QueryDetails(p) => query_details(st, p).await,
        PgRead::TableDetails(p) => table_details(st, p).await,
        PgRead::Settings(p) => settings(st, p).await,
        PgRead::Explain(p) => explain(st, p).await,
        PgRead::HypoIndex(p) => hypo(st, p).await,
    }
}

/// Наблюдение (`observe_activity`, `observe_top_queries`) — `mcp::observe`.
pub async fn observe_op(
    st: &AppState,
    op: PgObserve,
    ctx: ObserveCtx,
) -> Result<PgAnswer, ToolError> {
    match op {
        PgObserve::Activity(p) => observe::observe_activity(st, p, ctx).await,
        PgObserve::TopQueries(p) => observe::observe_top_queries(st, p, ctx).await,
    }
}

fn answer(body: PgOutput) -> PgAnswer {
    PgAnswer {
        body,
        masked: None,
        rows: None,
    }
}

/// Значение перечисления как его строка serde (`dev`, `primary`, `diagnostics`).
fn name_of<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn kind_name(k: ConnectionKind) -> &'static str {
    match k {
        ConnectionKind::Server => "server",
        ConnectionKind::HostList => "hosts",
        ConnectionKind::Patroni => "patroni",
    }
}

/// Имена папок от корня до папки `id`. Папки нет (удалена) — путь обрывается на ней; цикл `parentId` (испорченный
/// файл) не зацикливает: шагов не больше, чем папок.
fn folder_path(folders: &[ConnectionFolder], id: &str) -> Vec<String> {
    let mut path = Vec::new();
    let mut next = Some(id);
    while let Some(current) = next
        && path.len() < folders.len()
    {
        let Some(f) = folders.iter().find(|f| f.id == current) else {
            break;
        };
        path.push(f.name.clone());
        next = f.parent_id.as_deref();
    }
    path.reverse();
    path
}

/// `list_connections`: только подключения с действующим уровнем ≠ `off`; строк подключения и секретов нет (§6.2).
fn list_connections(st: &AppState) -> Vec<ConnectionInfo> {
    let folders = st.connections.list_folders();
    st.connections
        .list()
        .into_iter()
        .filter_map(|p| {
            let access = effective_access(&p, &st.policy.status);
            if access.is_off() {
                return None;
            }
            let connected = matches!(
                st.runtimes.view(&p.id).state,
                ConnState::Connected | ConnState::Degraded
            );
            Some(ConnectionInfo {
                folder_path: p
                    .folder_id
                    .as_deref()
                    .map(|f| folder_path(&folders, f))
                    .unwrap_or_default(),
                environment: name_of(&p.environment),
                kind: kind_name(p.kind).to_string(),
                database: p.pg.database.clone(),
                llm_access: access.as_str().to_string(),
                connected,
                role_warning: (access == LlmAccess::Data)
                    .then(|| crate::mcp::access::role_warning(st, &p.id))
                    .flatten(),
                id: p.id,
                name: p.name,
                read_only: p.read_only,
            })
        })
        .collect()
}

fn member_info(m: &Member) -> MemberInfo {
    MemberInfo {
        name: m.name.clone(),
        role: name_of(&m.role),
        state: if m.reachable {
            m.state.clone()
        } else {
            Some("unreachable".into())
        },
        host: m.host.clone(),
        port: m.port,
        timeline: m.timeline,
        lag_bytes: m.lag_bytes,
        tags: (!m.tags.is_empty()).then(|| Value::Object(m.tags.clone())),
        pending_restart: m.pending_restart.then_some(true),
        nofailover: m
            .tags
            .get("nofailover")
            .and_then(Value::as_bool)
            .filter(|b| *b),
    }
}

/// Топология для модели: участники, у Patroni — пауза и запланированные действия, события журнала за сессию
/// приложения (новые первыми, не больше `MAX_EVENTS`).
pub fn topology_info(t: &Topology, events: &[TopologyEvent]) -> TopologyInfo {
    let restarts: Vec<Value> = t
        .members
        .iter()
        .filter_map(|m| {
            m.scheduled_restart.as_ref().map(
                |r| json!({ "member": m.name, "at": r.at, "postgresVersion": r.postgres_version }),
            )
        })
        .collect();
    let scheduled_restart = (!restarts.is_empty()).then_some(Value::Array(restarts));
    TopologyInfo {
        kind: kind_name(t.kind).to_string(),
        members: t.members.iter().map(member_info).collect(),
        patroni: (t.kind == ConnectionKind::Patroni).then(|| PatroniInfo {
            paused: t.paused.unwrap_or(false),
            scheduled_switchover: t
                .scheduled_switchover
                .as_ref()
                .and_then(|s| serde_json::to_value(s).ok()),
            scheduled_restart,
        }),
        events: events
            .iter()
            .take(MAX_EVENTS)
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect(),
    }
}

async fn topology(st: &AppState, id: &str) -> Result<PgAnswer, ToolError> {
    open_for_llm(st, id).await?;
    let t = st
        .runtimes
        .view(id)
        .topology
        .ok_or_else(|| tool_error(not_connected()))?;
    let events = st.runtimes.events(id);
    Ok(answer(PgOutput::Topology(topology_info(&t, &events))))
}

pub(crate) fn unavailable_out(tool: Option<&str>, u: Unavailable) -> UnavailableOut {
    UnavailableOut {
        tool: tool.map(str::to_string),
        part: u.part,
        member: u.member,
        reason: u.reason.text,
    }
}

/// Ошибки узлов в данных отчёта (`PulseNode.error`, клетки `HealthCell.error`) — в виде для модели
/// (`access::model_app_error`: без `details`, путей и цели ssh). Интерфейс получает данные как есть: замена — только на
/// границе MCP. Исчерпывающий `match`: новый инструмент с ошибками в данных не пройдёт сборку незамеченным.
fn hide_error_details(data: &mut DiagData) {
    let hide = |e: &mut Option<crate::error::AppError>| {
        if let Some(e) = e.as_mut() {
            *e = model_app_error(e);
        }
    };
    match data {
        DiagData::Pulse(d) => d.nodes.iter_mut().for_each(|n| hide(&mut n.error)),
        DiagData::Health(d) => d
            .members
            .iter_mut()
            .flat_map(|m| m.cells.iter_mut())
            .for_each(|c| hide(&mut c.error)),
        DiagData::Sessions(_)
        | DiagData::Waits(_)
        | DiagData::Locks(_)
        | DiagData::Horizon(_)
        | DiagData::Replication(_)
        | DiagData::Slots(_)
        | DiagData::Failover(_)
        | DiagData::Queries(_) => {}
    }
}

/// Отчёт инструмента для модели: конверт P2, `data` — JSON-сериализация P2/P3 (та же форма, что у интерфейса, но
/// ошибки узлов — без подробностей, `hide_error_details`; короче для модели, смоук P4b: профиль ожиданий — без
/// интервалов без сэмплов, `waits_for_model`, строки `queries` — `top_rows_for_model`), находки — с английскими текстами,
/// причины недоступности — английским текстом.
pub fn report_out(mut r: DiagReport) -> Report {
    hide_error_details(&mut r.data);
    let hint = match &mut r.data {
        DiagData::Waits(d) => waits_for_model(d),
        _ => None,
    };
    let mut data = serde_json::to_value(&r.data).unwrap_or(Value::Null);
    if matches!(r.data, DiagData::Queries(_)) {
        top_rows_for_model(&mut data);
    }
    Report {
        tool: tool_id(r.tool),
        data,
        findings: r.findings.iter().map(finding_out).collect(),
        unavailable: r
            .unavailable
            .into_iter()
            .map(|u| unavailable_out(None, u))
            .collect(),
        member: r.member,
        follows_leader: r.follows_leader,
        observed_at_ms: r.observed_at_ms,
        server_version_num: r.server_version_num,
        in_recovery: r.in_recovery,
        truncated: false,
        hint,
    }
}

/// Профиль ожиданий для модели (смоук P4b, G1): интервалы без сэмплов (`sampledS` 0 и пустой `aas`) не отдаются. Без
/// единого сэмпла в окне интервалов нет совсем, а подсказка говорит почему: сэмплер не работал (`NO_WAIT_SAMPLES_HINT`)
/// или работал, но активных сессий не видел. Итог — подсказка.
fn waits_for_model(d: &mut WaitsData) -> Option<String> {
    if d.total_samples > 0 {
        d.buckets.retain(|b| b.sampled_s > 0.0 || !b.aas.is_empty());
        return None;
    }
    let sampled_s: f64 = d.buckets.iter().map(|b| b.sampled_s).sum();
    d.buckets.clear();
    Some(if sampled_s > 0.0 {
        format!(
            "The wait sampler ran for {} s of this window and saw no active sessions",
            sampled_s.round()
        )
    } else {
        NO_WAIT_SAMPLES_HINT.to_string()
    })
}

/// Строка pgss для модели (смоук P4b, G2): счётчики `ZERO_OMITTED_COUNTERS`, равные 0, и `own: false` не отдаются
/// (описания инструментов говорят об этом); `calls`, `totalExecMs`, `meanExecMs`, `rows`, ключ, база, роль, текст и
/// `textTruncated` остаются всегда, доли (`sharePct`, `hitRatio`) — тоже.
fn stat_row_for_model(row: &mut Value) {
    let Some(row) = row.as_object_mut() else {
        return;
    };
    for key in ZERO_OMITTED_COUNTERS {
        if row.get(key).and_then(Value::as_f64) == Some(0.0) {
            row.remove(key);
        }
    }
    if row.get("own") == Some(&Value::Bool(false)) {
        row.remove("own");
    }
}

/// Строки топа в `data.rows` отчёта `queries` (`get_top_queries`, `observe_top_queries`, `run_diagnostic`):
/// `stat_row_for_model`, текст — первые `TOP_TEXT_CHARS` символов с `textTruncated: true`.
fn top_rows_for_model(data: &mut Value) {
    let Some(rows) = data.get_mut("rows").and_then(Value::as_array_mut) else {
        return;
    };
    for row in rows {
        stat_row_for_model(row);
        let long = row["text"]
            .as_str()
            .and_then(|t| t.char_indices().nth(TOP_TEXT_CHARS))
            .map(|(cut, _)| cut);
        if let (Some(cut), Some(Value::String(text))) = (long, row.get_mut("text")) {
            text.truncate(cut);
            row["textTruncated"] = Value::Bool(true);
        }
    }
}

/// Маскирует тексты отчёта на уровне `diagnostics` (`mask::mask_report`, без чтения `standard_conforming_strings`);
/// итог — число литералов для журнала. Отчёт без текстов и уровень без маскирования — `None`.
pub(crate) fn mask_if(access: LlmAccess, report: &mut DiagReport) -> Option<u64> {
    if !masks(access) || !has_texts(&report.data) {
        return None;
    }
    Some(mask_report(report) as u64)
}

/// Поиск по тексту запроса, отложенный до маскирования (`take_masked_search`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MaskedSearch {
    /// Искомая подстрока в нижнем регистре.
    needle: String,
    /// Строк в ответе — предел запроса до расширения.
    limit: usize,
}

impl MaskedSearch {
    /// Замаскированный текст строки содержит подстроку (без учёта регистра); строки без прочитанного текста — нет.
    fn matches(&self, r: &QueryStat) -> bool {
        r.text
            .as_deref()
            .is_some_and(|t| t.to_lowercase().contains(&self.needle))
    }
}

/// Поиск `search` инструмента `queries` на уровне, где тексты маскируются (решение владельца 2026-11-04). Поиск P3
/// идёт по исходным текстам pgss (`ILIKE` на сервере, кеш текстов узла), и ответ «есть строки / нет строк» выдавал
/// бы литералы по символу. Поэтому здесь `search` убирается из параметров P3, а предел строк расширяется до
/// наибольшего (`LIMITS`, 200: тексты читаются только у строк топа); `mask_statements` маскирует строки и
/// оставляет те, чей ЗАМАСКИРОВАННЫЙ текст содержит подстроку (без учёта регистра), не больше предела запроса. Итоги не
/// меняются: поиск сужает строки, а не итоги (`search_restricts_rows_but_not_the_totals`). Строки вне расширенного топа
/// (по выбранной сортировке) и без прочитанного текста поиск не находит — об этом говорит описание параметра модели.
/// На уровне без маскирования (P4b `data`) — `None`, поиск P3 по исходным текстам остаётся. Тот же порядок — у
/// `run_diagnostic` (`queries.search`) и `get_top_queries` (`search`): `take_masked_search` до `diag::run`, затем
/// `mask_statements` (маскирование, поиск, свой предел ответа).
pub(crate) fn take_masked_search(q: &mut QueriesParams, access: LlmAccess) -> Option<MaskedSearch> {
    if !masks(access) {
        return None;
    }
    let needle = q.search()?.to_lowercase();
    q.search = None;
    let limit = q.limit;
    q.limit = LIMITS.iter().copied().max().unwrap_or(limit).max(limit);
    Some(MaskedSearch { needle, limit })
}

/// Тексты отчёта `queries` для модели и строки ответа, по шагам: тексты маскируются (уровень `diagnostics`, `mask_opt`
/// у каждой строки); поиск `take_masked_search` оставляет строки с подстрокой в замаскированном тексте, не больше
/// предела запроса, и находки строк, не попавших в результат поиска, уходят (как у поиска P3: правила видят только
/// найденные строки; находки без запроса, `queries_incomplete`, остаются); затем строки режутся до `limit` ответа
/// (`None` — не режутся). Итог — литералы строк, оставшихся в ответе (журнал `masked`); уровень без маскирования —
/// `None`. Отчёт другого инструмента — `mask_if`.
pub(crate) fn mask_statements(
    access: LlmAccess,
    report: &mut DiagReport,
    search: Option<&MaskedSearch>,
    limit: Option<usize>,
) -> Option<u64> {
    let DiagData::Queries(d) = &mut report.data else {
        return mask_if(access, report);
    };
    let mask = masks(access);
    // Литералы считаются у каждой строки: поиск и предел ниже убирают строки, а в журнал идут только отданные.
    let mut rows: Vec<(QueryStat, usize)> = std::mem::take(&mut d.rows)
        .into_iter()
        .map(|mut r| {
            let n = if mask { mask_opt(&mut r.text) } else { 0 };
            (r, n)
        })
        .collect();
    if let Some(s) = search {
        rows.retain(|(r, _)| s.matches(r));
        rows.truncate(s.limit);
        report.findings.retain(|f| match &f.target {
            Some(FindingTarget::Query { query_id, database }) => rows.iter().any(|(r, _)| {
                r.key.query_id.to_string() == *query_id
                    && r.datname.as_deref().unwrap_or_default() == database
            }),
            _ => true,
        });
    }
    rows.truncate(limit.unwrap_or(usize::MAX));
    let masked: usize = rows.iter().map(|(_, n)| n).sum();
    d.rows = rows.into_iter().map(|(r, _)| r).collect();
    mask.then_some(masked as u64)
}

/// id инструмента из параметра `tool`.
fn parse_tool(s: &str) -> Result<Tool, ToolError> {
    serde_json::from_value(Value::String(s.to_string())).map_err(|_| {
        invalid_argument(format!(
            "Unknown diagnostic tool {s}; call list_diagnostics for the available tools"
        ))
    })
}

/// Параметры инструмента `queries` (форма P3: `mode`, `sort`, `limit`, `database`, `search`) с умолчаниями `sinceReset`,
/// `totalTime`, 50. «Между снимками» через шлюз недоступно: снимки pgss модели не отдаются (§6.2). Крейт шлюза `queries`
/// не разбирает: сортировка проверяется разбором, предел (50 / 100 / 200) и окно (300 / 900 / 3600) — проверкой P3
/// (`diag::queries::validate`), до открытия подключения.
fn queries_params(v: Option<Value>) -> Result<QueriesParams, ToolError> {
    let mut obj = match v {
        None => Map::new(),
        Some(Value::Object(m)) => m,
        Some(_) => return Err(invalid_argument("queries must be an object")),
    };
    obj.entry("mode").or_insert(json!({ "kind": "sinceReset" }));
    obj.entry("sort").or_insert(json!("totalTime"));
    obj.entry("limit").or_insert(json!(50));
    let p: QueriesParams = serde_json::from_value(Value::Object(obj))
        .map_err(|e| invalid_argument(format!("queries: {e}")))?;
    if matches!(p.mode, QueriesMode::Between { .. }) {
        return Err(invalid_argument(
            "queries.mode between (pg_stat_statements snapshots) is not available through MCP",
        ));
    }
    diag::queries::validate(&p).map_err(tool_error)?;
    Ok(p)
}

async fn run_diagnostic(st: &AppState, p: RunDiagnosticParams) -> Result<PgAnswer, ToolError> {
    let (profile, access) = check_connection(st, &p.connection_id)?;
    let tool = parse_tool(&p.tool)?;
    let mut queries = if tool == Tool::Queries {
        Some(queries_params(p.queries)?)
    } else {
        None
    };
    let search = queries.as_mut().and_then(|q| take_masked_search(q, access));
    let req = DiagRequest {
        tool,
        member: p.member,
        window_s: p.window_s,
        queries,
    };
    // Инструмент Patroni у другого вида подключения и неверное окно отказывают до открытия подключения.
    diag::validate(&req, profile.kind).map_err(tool_error)?;
    ensure_open(st, &p.connection_id).await?;
    let mut report = diag::run(st, &p.connection_id, req)
        .await
        .map_err(tool_error)?;
    let masked = mask_statements(access, &mut report, search.as_ref(), None);
    Ok(PgAnswer {
        body: PgOutput::Report(report_out(report)),
        masked,
        rows: None,
    })
}

/// Сводка из отчётов: пульс — данными; находки всех инструментов — по серьёзности (`findings::sort`), а находка пульса,
/// которую уточнил узловой инструмент (тот же id и узел), не повторяется. Упавший или не уложившийся инструмент и
/// пометки отчётов — в `unavailable` с id инструмента.
pub fn health_of(results: Vec<(Tool, Result<DiagReport, String>)>) -> HealthSummary {
    let mut pulse = Value::Null;
    let (mut from_pulse, mut findings, mut unavailable) = (Vec::new(), Vec::new(), Vec::new());
    for (tool, result) in results {
        let id = tool_id(tool);
        match result {
            Ok(mut r) => {
                unavailable.extend(
                    r.unavailable
                        .into_iter()
                        .map(|u| unavailable_out(Some(&id), u)),
                );
                if tool == Tool::Pulse {
                    hide_error_details(&mut r.data);
                    pulse = serde_json::to_value(&r.data).unwrap_or(Value::Null);
                    from_pulse.extend(r.findings);
                } else {
                    findings.extend(r.findings);
                }
            }
            Err(reason) => unavailable.push(UnavailableOut {
                tool: Some(id.clone()),
                part: id,
                member: None,
                reason,
            }),
        }
    }
    from_pulse.retain(|p| {
        !findings
            .iter()
            .any(|f| f.id == p.id && f.member == p.member)
    });
    findings.extend(from_pulse);
    pg_advisor::findings::sort(&mut findings);
    HealthSummary {
        pulse,
        findings: findings.iter().map(finding_out).collect(),
        unavailable,
    }
}

/// `get_health_summary`: пульс и быстрые инструменты параллельно, общий дедлайн `HEALTH_DEADLINE`. Текстов запросов в
/// ответе нет (находки и пульс — числа и имена), маскировать нечего.
async fn health_summary(st: &AppState, id: &str) -> Result<PgAnswer, ToolError> {
    let (profile, _) = check_connection(st, id)?;
    ensure_open(st, id).await?;
    let mut tools = vec![Tool::Pulse];
    tools.extend(HEALTH_TOOLS);
    if profile.kind == ConnectionKind::Patroni {
        tools.extend([Tool::Failover, Tool::Health]);
    }
    let deadline = tokio::time::Instant::now() + HEALTH_DEADLINE;
    let runs = tools.into_iter().map(|tool| async move {
        let req = DiagRequest {
            tool,
            member: None,
            window_s: None,
            queries: None,
        };
        let result = match tokio::time::timeout_at(deadline, diag::run(st, id, req)).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => {
                let e = tool_error(e);
                Err(format!("{}: {}", e.code, e.message))
            }
            Err(_) => Err(format!(
                "did not finish within {} s",
                HEALTH_DEADLINE.as_secs()
            )),
        };
        (tool, result)
    });
    Ok(answer(PgOutput::HealthSummary(health_of(
        join_all(runs).await,
    ))))
}

/// Длительность строки для фильтра и порядка: у активной — запроса, у простаивающей в транзакции — транзакции; у
/// остальных (`idle`, фоновые процессы) её нет — такие строки идут в конце и не проходят `minDurationS`.
pub fn duration_s(r: &SessionRow) -> Option<f64> {
    match r.state.as_deref() {
        Some("active") => r.query_age_s,
        Some(s) if s.starts_with("idle in transaction") => r.xact_age_s,
        _ => None,
    }
}

/// Строки `sessions` после фильтров `get_activity` (состояние и тип ожидания — без учёта регистра, база — точно,
/// приложение — подстрока без учёта регистра, как обещает схема параметра `application` крейта шлюза), по убыванию
/// длительности, не больше `limit`; первое значение — сколько строк прошло фильтры.
pub fn filter_sessions(rows: Vec<SessionRow>, p: &ActivityParams) -> (u64, Vec<SessionRow>) {
    let same = |value: &Option<String>, wanted: &Option<String>| {
        wanted
            .as_deref()
            .is_none_or(|w| value.as_deref().is_some_and(|v| v.eq_ignore_ascii_case(w)))
    };
    let application = p.application.as_deref().map(str::to_lowercase);
    let mut out: Vec<SessionRow> = rows
        .into_iter()
        .filter(|r| {
            same(&r.state, &p.state)
                && same(&r.wait_event_type, &p.wait_event_type)
                && p.database
                    .as_deref()
                    .is_none_or(|d| r.database.as_deref() == Some(d))
                && application.as_deref().is_none_or(|a| {
                    r.application
                        .as_deref()
                        .is_some_and(|v| v.to_lowercase().contains(a))
                })
                && p.min_duration_s
                    .is_none_or(|m| duration_s(r).is_some_and(|d| d >= f64::from(m)))
        })
        .collect();
    let total = out.len() as u64;
    out.sort_by(|a, b| {
        duration_s(b)
            .unwrap_or(-1.0)
            .total_cmp(&duration_s(a).unwrap_or(-1.0))
            .then(a.pid.cmp(&b.pid))
    });
    out.truncate(p.limit.unwrap_or(ACTIVITY_LIMIT) as usize);
    (total, out)
}

/// `get_activity`: строки инструмента `sessions` после фильтров (`activity_answer`).
async fn activity(st: &AppState, p: ActivityParams) -> Result<PgAnswer, ToolError> {
    let access = open_for_llm(st, &p.connection_id).await?;
    let req = DiagRequest {
        tool: Tool::Sessions,
        member: p.member.clone(),
        window_s: None,
        queries: None,
    };
    let report = diag::run(st, &p.connection_id, req)
        .await
        .map_err(tool_error)?;
    activity_answer(report, &p, access)
}

/// Ответ `get_activity` из отчёта `sessions`: фильтры и предел (`filter_sessions`), маскирование только отданных строк
/// (уровень `diagnostics`), затем тексты — не длиннее `MAX_QUERY_BYTES` по границе символа, как у `observe_activity`
/// (на любом уровне: 500 строк по 1 МБ `track_activity_query_size` не уложились бы в ответ).
fn activity_answer(
    mut report: DiagReport,
    p: &ActivityParams,
    access: LlmAccess,
) -> Result<PgAnswer, ToolError> {
    let DiagData::Sessions(d) = &mut report.data else {
        return Err(tool_error(AppError::internal_with(
            "sessions report without sessions data",
        )));
    };
    let (total, rows) = filter_sessions(std::mem::take(&mut d.rows), p);
    d.rows = rows;
    let masked = mask_if(access, &mut report);
    let DiagData::Sessions(mut d) = report.data else {
        return Err(tool_error(AppError::internal_with(
            "sessions report without sessions data",
        )));
    };
    for q in d.rows.iter_mut().filter_map(|r| r.query.as_mut()) {
        cut_text(q, MAX_QUERY_BYTES);
    }
    Ok(PgAnswer {
        body: PgOutput::Activity(ActivityOut {
            total,
            sessions: d
                .rows
                .iter()
                .filter_map(|r| serde_json::to_value(r).ok())
                .collect(),
        }),
        masked,
        rows: None,
    })
}

/// Узел операции: названный или «за лидером» (`diag_target`, как у вкладок). Подключение должно быть открыто.
pub(crate) fn member_or_leader(
    st: &AppState,
    id: &str,
    member: Option<&str>,
) -> Result<String, ToolError> {
    st.runtimes
        .diag_target(id, member, false)
        .map_err(tool_error)?
        .member
        .map(|m| m.name)
        .ok_or_else(|| tool_error(AppError::internal_with("node operation without a member")))
}

/// Сортировка топа (`totalTime` по умолчанию) — имена P3 (`QuerySort`, camelCase).
pub(crate) fn parse_sort(s: Option<&str>) -> Result<QuerySort, ToolError> {
    let s = s.unwrap_or("totalTime");
    serde_json::from_value(Value::String(s.to_string())).map_err(|_| {
        invalid_argument(format!(
            "Unknown sort {s}: use totalTime, meanTime, maxTime, stddevTime, calls, rowsPerCall, sharedRead, hitRatio, tempWritten, walBytes, ioTime, planTime, jitTime or cpuTime"
        ))
    })
}

/// Запрос инструмента `queries` по параметрам `get_top_queries`: предел отчёта — ближайший допустимый в P3 (50 или 100),
/// ответ потом режется до `limit`; окно по умолчанию — 900 с, окно проверяется P3 (`diag::queries::validate`) до
/// открытия подключения; без `mode` (вызов не через шлюз — без `validate` крейта) режим — по правилу крейта
/// `default_top_mode`. Итог: параметры, `limit`, режим окна.
pub(crate) fn top_request(p: &TopQueriesParams) -> Result<(QueriesParams, usize, bool), ToolError> {
    let limit = p.limit.unwrap_or(TOP_LIMIT) as usize;
    let window = match p
        .mode
        .as_deref()
        .unwrap_or_else(|| default_top_mode(p.window_s))
    {
        MODE_SINCE_RESET => false,
        MODE_WINDOW => true,
        m => {
            return Err(invalid_argument(format!(
                "Unknown mode {m}: use sinceReset or window"
            )));
        }
    };
    let q = QueriesParams {
        mode: if window {
            QueriesMode::Window {
                s: p.window_s.unwrap_or(DEFAULT_WINDOW_S),
            }
        } else {
            QueriesMode::SinceReset
        },
        sort: parse_sort(p.sort.as_deref())?,
        limit: if limit <= 50 { 50 } else { 100 },
        database: p.database.clone(),
        toplevel_only: true,
        search: p.search.clone(),
    };
    diag::queries::validate(&q).map_err(tool_error)?;
    Ok((q, limit, window))
}

/// `get_top_queries`: отчёт инструмента `queries` (P3 §4.5). Порядок (решение владельца 2026-11-04): доступ и
/// аргументы — до открытия подключения; поиск на уровне `diagnostics` откладывается (`take_masked_search`), затем
/// `diag::run` и `top_answer`. Фильтр `database` без единой строки отчёта — проверка, есть ли такая база на узле
/// (`filter_database_missing`, E3). `windowS` при явном `sinceReset` — подсказка в ответе (E1).
async fn top_queries(st: &AppState, p: TopQueriesParams) -> Result<PgAnswer, ToolError> {
    let (profile, access) = check_connection(st, &p.connection_id)?;
    let (mut q, limit, window) = top_request(&p)?;
    let ignored_window = !window && p.window_s.is_some();
    let search = take_masked_search(&mut q, access);
    ensure_open(st, &p.connection_id).await?;
    let req = DiagRequest {
        tool: Tool::Queries,
        member: p.member.clone(),
        window_s: None,
        queries: Some(q),
    };
    let report = diag::run(st, &p.connection_id, req)
        .await
        .map_err(tool_error)?;
    let no_rows = matches!(&report.data, DiagData::Queries(d) if d.rows.is_empty());
    if let (true, Some(database), Some(member)) =
        (no_rows, p.database.as_deref(), report.member.as_deref())
    {
        let connection_db = resolve_database(&profile, None);
        if let Some(e) =
            filter_database_missing(st, &p.connection_id, member, database, &connection_db).await
        {
            return Err(e);
        }
    }
    Ok(top_answer(
        report,
        access,
        search.as_ref(),
        limit,
        window,
        ignored_window,
    ))
}

/// Ответ `get_top_queries` из отчёта `queries`: маскирование, поиск по замаскированному тексту и только потом предел
/// ответа `limit` (`mask_statements`); окно без истории — покрытие 0 и подсказка; `ignored_window` — модель прислала
/// `windowS` при явном `sinceReset` (E1): подсказка, что окно не применено. Подсказки склеиваются (`add_hint`).
fn top_answer(
    mut report: DiagReport,
    access: LlmAccess,
    search: Option<&MaskedSearch>,
    limit: usize,
    window: bool,
    ignored_window: bool,
) -> PgAnswer {
    let no_history =
        window && matches!(&report.data, DiagData::Queries(d) if d.coverage_s.is_none());
    let masked = mask_statements(access, &mut report, search, Some(limit));
    let mut out = report_out(report);
    if no_history {
        out.data["coverageS"] = json!(0);
        add_hint(&mut out.hint, NO_WINDOW_HINT);
    }
    if ignored_window {
        add_hint(&mut out.hint, SINCE_RESET_WINDOW_HINT);
    }
    PgAnswer {
        body: PgOutput::Report(out),
        masked,
        rows: None,
    }
}

/// Подсказка `more` к уже имеющейся (`join_hints` крейта: граница предложения между ними): ни одна не теряется.
pub(crate) fn add_hint(hint: &mut Option<String>, more: &str) {
    *hint = Some(match hint.take() {
        Some(h) => join_hints(&h, more),
        None => more.to_string(),
    });
}

/// Базы узла (E3): имя и «показывать ли модели» — не шаблон и с `datallowconn`. Только имена (§6.2: ни хостов, ни
/// владельцев, ни настроек).
const DATABASES_SQL: &str =
    marked_sql!("SELECT datname::text, datallowconn AND NOT datistemplate FROM pg_database");

/// Базы узла `member` (E3) — тем же путём узла, что `pgss_text` (`diag_target` + `on_member`, пул каталога узла,
/// только чтение).
async fn node_databases(
    st: &AppState,
    id: &str,
    member: &str,
) -> Result<Vec<(String, bool)>, ToolError> {
    let target = st
        .runtimes
        .diag_target(id, Some(member), false)
        .map_err(tool_error)?;
    on_member(st, id, &target, |c| {
        Box::pin(async move {
            let rows = c.query(DATABASES_SQL, &[]).await.map_err(|e| from_pg(&e))?;
            Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
        })
    })
    .await
    .map_err(tool_error)
}

/// Фильтр `database` у `get_top_queries` дал пустой отчёт (E3): модель могла подставить базу другого подключения и
/// решить, что сервер простаивает. Базы нет на узле — `not_found` со списком баз (`database_missing`); есть — `None`,
/// ответ пустой, как раньше. Сбой служебного запроса — тоже `None` (с записью `debug` в журнал приложения): отчёт
/// важнее проверки.
async fn filter_database_missing(
    st: &AppState,
    id: &str,
    member: &str,
    database: &str,
    connection_db: &str,
) -> Option<ToolError> {
    let all = match node_databases(st, id, member).await {
        Ok(all) => all,
        Err(e) => {
            tracing::debug!(
                "get_top_queries: databases of {member} not read, the filter database is not checked: {}: {}",
                e.code,
                e.message
            );
            return None;
        }
    };
    database_missing(database, member, all, connection_db)
}

/// Отказ E3, если базы `database` нет среди баз узла `all` (имя, показывается ли): текст называет узел, базы узла без
/// шаблонов и баз без `datallowconn` (по алфавиту, не больше `MAX_DATABASE_NAMES`, дальше «…and N more») и базу
/// подключения. Есть (даже шаблоном) — `None`.
fn database_missing(
    database: &str,
    member: &str,
    all: Vec<(String, bool)>,
    connection_db: &str,
) -> Option<ToolError> {
    if all.iter().any(|(name, _)| name == database) {
        return None;
    }
    let mut names: Vec<String> = all
        .into_iter()
        .filter_map(|(name, listed)| listed.then_some(name))
        .collect();
    names.sort();
    let more = names.len().saturating_sub(MAX_DATABASE_NAMES);
    names.truncate(MAX_DATABASE_NAMES);
    let mut list = if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    };
    if more > 0 {
        list.push_str(&format!(", …and {more} more"));
    }
    Some(err(
        "not_found",
        format!(
            "Database {database} does not exist on {member}; databases: {list} (the connection's database: {connection_db})"
        ),
    ))
}

/// `get_query_details`: карточка P3 §7.2 (`query_detail_answer`); доступ и `queryId` проверяются до открытия
/// подключения.
async fn query_details(st: &AppState, p: QueryDetailsParams) -> Result<PgAnswer, ToolError> {
    let (profile, access) = check_connection(st, &p.connection_id)?;
    let database = resolve_database(&profile, p.database.as_deref());
    let qid = query_detail::parse_query_id(&p.query_id).map_err(tool_error)?;
    ensure_open(st, &p.connection_id).await?;
    let member = member_or_leader(st, &p.connection_id, p.member.as_deref())?;
    let d = query_detail::detail(st, &p.connection_id, &member, &database, qid)
        .await
        .map_err(tool_error)?;
    query_detail_answer(d, &member, access)
}

/// Карточка для модели — JSON-сериализация `QueryDetail` (та же форма, что у интерфейса): выполняющихся экземпляров —
/// не больше `MAX_RUNNING` самых долгих (полное число — в `runningTotal`), тексты маскируются (уровень
/// `diagnostics`, `mask_query_detail`), тексты выполняющихся экземпляров затем — не длиннее `MAX_QUERY_BYTES` (как у
/// `get_activity`, на любом уровне; обрезанный помечается `truncated`), находки — с английскими текстами, причины
/// недоступности — английским текстом, узел — в поле `member`.
fn query_detail_answer(
    mut d: QueryDetail,
    member: &str,
    access: LlmAccess,
) -> Result<PgAnswer, ToolError> {
    // Сначала предел (литералы считаются только у отданных экземпляров): самые долгие, без длительности — в конце.
    let running_total = d.running.len();
    let duration = |r: &query_detail::RunningInstance| r.duration_s.unwrap_or(f64::NEG_INFINITY);
    d.running
        .sort_by(|a, b| duration(b).total_cmp(&duration(a)));
    d.running.truncate(MAX_RUNNING);
    let masked = masks(access).then(|| mask_query_detail(&mut d) as u64);
    for r in &mut d.running {
        let len = r.query.len();
        cut_text(&mut r.query, MAX_QUERY_BYTES);
        r.truncated |= r.query.len() < len;
    }
    let findings: Vec<FindingOut> = d.findings.iter().map(finding_out).collect();
    let unavailable: Vec<UnavailableOut> = std::mem::take(&mut d.unavailable)
        .into_iter()
        .map(|u| unavailable_out(None, u))
        .collect();
    d.history = history_for_model(std::mem::take(&mut d.history));
    let mut v =
        serde_json::to_value(&d).map_err(|e| tool_error(AppError::internal_with(e.to_string())))?;
    card_rows_for_model(&mut v);
    v["member"] = json!(member);
    if running_total > MAX_RUNNING {
        v["runningTotal"] = json!(running_total);
    }
    v["findings"] = serde_json::to_value(findings).unwrap_or_default();
    v["unavailable"] = serde_json::to_value(unavailable).unwrap_or_default();
    Ok(PgAnswer {
        body: PgOutput::QueryDetails(v),
        masked,
        rows: None,
    })
}

/// Строки pgss карточки (`sinceReset`, `windows[].stat`, `byUser[].stat`) — `stat_row_for_model`.
fn card_rows_for_model(v: &mut Value) {
    stat_row_for_model(&mut v["sinceReset"]);
    for key in ["windows", "byUser"] {
        if let Some(items) = v[key].as_array_mut() {
            items
                .iter_mut()
                .for_each(|item| stat_row_for_model(&mut item["stat"]));
        }
    }
}

/// История карточки для модели (смоук P4b, G4): не больше `MAX_HISTORY_POINTS` точек. Соседние точки сливаются в
/// почти равные группы (размеры отличаются не больше чем на 1), порядок ряда сохраняется: `calls` и `totalExecMs`
/// суммируются, `meanExecMs = totalExecMs / calls` (без вызовов — 0), `atMs` — начало слитого диапазона.
pub fn history_for_model(points: Vec<HistoryPoint>) -> Vec<HistoryPoint> {
    let n = points.len();
    if n <= MAX_HISTORY_POINTS {
        return points;
    }
    (0..MAX_HISTORY_POINTS)
        .map(|g| {
            let part = &points[g * n / MAX_HISTORY_POINTS..(g + 1) * n / MAX_HISTORY_POINTS];
            let calls: i64 = part.iter().map(|p| p.calls).sum();
            let total_exec_ms: f64 = part.iter().map(|p| p.total_exec_ms).sum();
            HistoryPoint {
                at_ms: part[0].at_ms,
                calls,
                total_exec_ms,
                mean_exec_ms: if calls > 0 {
                    total_exec_ms / calls as f64
                } else {
                    0.0
                },
            }
        })
        .collect()
}

/// Статистика колонок без значений (уровень `diagnostics`): у таблицы с наследниками — строка по всей иерархии
/// (`inherited`), если она есть. `pg_stats` показывает только колонки, которые роль может читать.
const STATS_SQL: &str = marked_sql!(
    "SELECT DISTINCT ON (s.attname) s.attname::text, s.null_frac::float8, s.avg_width, s.n_distinct::float8, \
     s.correlation::float8 FROM pg_stats s WHERE s.schemaname = $1 AND s.tablename = $2 \
     ORDER BY s.attname, s.inherited DESC"
);

/// То же со значениями (уровень `data`, P4b): частые значения, их частоты и границы гистограммы — JSON-массивы в
/// текстовом виде PostgreSQL.
const STATS_VALUES_SQL: &str = marked_sql!(
    "SELECT DISTINCT ON (s.attname) s.attname::text, s.null_frac::float8, s.avg_width, s.n_distinct::float8, \
     s.correlation::float8, array_to_json(s.most_common_vals::text::text[]), array_to_json(s.most_common_freqs), \
     array_to_json(s.histogram_bounds::text::text[]) FROM pg_stats s WHERE s.schemaname = $1 AND s.tablename = $2 \
     ORDER BY s.attname, s.inherited DESC"
);

/// Строки длиннее `MAX_STATS_VALUE_BYTES` в массиве значений обрезаются по границе символа с пометкой.
pub fn cut_values(v: Value) -> Value {
    match v {
        Value::String(s) if s.len() > MAX_STATS_VALUE_BYTES => {
            let mut end = MAX_STATS_VALUE_BYTES;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            Value::String(format!("{}{TRUNCATED_MARK}", &s[..end]))
        }
        Value::Array(a) => Value::Array(a.into_iter().map(cut_values).collect()),
        other => other,
    }
}

/// Статистика колонок `pg_stats` таблицы (спец. §5.2): `with_values` — уровень `data` (частые значения и гистограмма),
/// без него запрос значений не читает вовсе.
pub async fn column_stats(
    c: &Client,
    schema: &str,
    table: &str,
    with_values: bool,
) -> AppResult<Vec<ColumnStat>> {
    let sql = if with_values {
        STATS_VALUES_SQL
    } else {
        STATS_SQL
    };
    let rows = c
        .query(sql, &[&schema, &table])
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(rows
        .iter()
        .map(|r| {
            let values = |i: usize| {
                if with_values {
                    r.get::<_, Option<Value>>(i).map(cut_values)
                } else {
                    None
                }
            };
            ColumnStat {
                column: r.get(0),
                null_frac: r.get(1),
                avg_width: r.get(2),
                n_distinct: r.get(3),
                correlation: r.get(4),
                most_common_vals: values(5),
                most_common_freqs: values(6),
                histogram_bounds: values(7),
            }
        })
        .collect())
}

/// Порядок колонок таблицы; статистика колонок, которых нет в описании, — в конце по имени.
fn order_like(mut stats: Vec<ColumnStat>, columns: &[Column]) -> Vec<ColumnStat> {
    let pos = |name: &str| {
        columns
            .iter()
            .position(|c| c.name == name)
            .unwrap_or(usize::MAX)
    };
    stats.sort_by(|a, b| {
        pos(&a.column)
            .cmp(&pos(&b.column))
            .then_with(|| a.column.cmp(&b.column))
    });
    stats
}

/// `get_table_details`: вкладка таблицы P1 (`catalog::table::get` — DDL не маскируется: это схема, §6.1) и статистика
/// колонок; узел каталога подключения, база запроса.
async fn table_details(st: &AppState, p: TableDetailsParams) -> Result<PgAnswer, ToolError> {
    let access = open_for_llm(st, &p.connection_id).await?;
    let with_values = !masks(access);
    let (_, profile) = st
        .runtimes
        .target(&p.connection_id, None)
        .map_err(tool_error)?;
    let database = resolve_database(&profile, p.database.as_deref());
    let (schema, table) = (p.schema.clone(), p.table.clone());
    let (details, stats) = on_catalog_node_within(
        catalog_limit(&profile),
        st.client(&p.connection_id, None, Some(&database)),
        move |c| {
            Box::pin(async move {
                let details = catalog_table::get(c, &schema, &table).await?;
                let stats = column_stats(c, &schema, &table, with_values).await?;
                Ok((details, stats))
            })
        },
    )
    .await
    .map_err(tool_error)?;
    Ok(answer(PgOutput::Table(table_out(details, stats)?)))
}

/// Таблица для модели — JSON-сериализация `TableDetails` (форма интерфейса) с партициями не длиннее `MAX_PARTITIONS`
/// (`cut_partitions`, полное число — в `partitioning.partitionsTotal`) и статистика колонок в порядке таблицы.
fn table_out(mut details: TableDetails, stats: Vec<ColumnStat>) -> Result<TableOut, ToolError> {
    let total = cut_partitions(&mut details);
    let column_stats = order_like(stats, &details.columns);
    let mut table = serde_json::to_value(&details)
        .map_err(|e| tool_error(AppError::internal_with(e.to_string())))?;
    if let Some(total) = total {
        table["partitioning"]["partitionsTotal"] = json!(total);
    }
    Ok(TableOut {
        table,
        column_stats,
    })
}

/// Прямые партиции — первые `MAX_PARTITIONS` (в порядке каталога); строка `-- partitions:` в DDL (`ddl::build_ddl`:
/// `schema.name` через `quote_ident`, управляющие символы — пробелы) заменяется строкой с теми же партициями и
/// полным числом. Вернёт полное число, если список обрезан.
fn cut_partitions(d: &mut TableDetails) -> Option<usize> {
    let total = d.partitioning.partitions.len();
    if total <= MAX_PARTITIONS {
        return None;
    }
    let names = |parts: &[RelationRef]| -> String {
        parts
            .iter()
            .map(|r| format!("{}.{}", quote_ident(&r.schema), quote_ident(&r.name)))
            .collect::<Vec<_>>()
            .join(", ")
            .chars()
            .map(|c| {
                if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
                    ' '
                } else {
                    c
                }
            })
            .collect()
    };
    let full = format!("\n-- partitions: {}\n", names(&d.partitioning.partitions));
    d.partitioning.partitions.truncate(MAX_PARTITIONS);
    let cut = format!(
        "\n-- partitions ({MAX_PARTITIONS} of {total}): {}\n",
        names(&d.partitioning.partitions)
    );
    d.ddl = d.ddl.replacen(&full, &cut, 1);
    Some(total)
}

/// Параметры сервера (спец. §5.2): `$1` — ровно эти имена (без учёта регистра: `DateStyle`, `TimeZone` в `pg_settings`
/// — с заглавными), иначе при `$2` — изменённые: источник не `default` и не `override`, и не настройки самого
/// соединения Glafira (`client`, `session`).
const SETTINGS_SQL: &str = marked_sql!(
    "SELECT name, setting, unit, source, context, pending_restart FROM pg_settings \
     WHERE ($1::text[] IS NULL OR lower(name) = ANY($1)) \
     AND (NOT $2 OR source NOT IN ('default', 'override', 'client', 'session')) ORDER BY name"
);

/// Строка `pg_settings` для модели; значение — без секретов (`redact_setting`), `hidden` — значение изменено.
fn setting_out(r: &Row) -> SettingOut {
    let name: String = r.get(0);
    let raw = r.get::<_, Option<String>>(1).unwrap_or_default();
    let (setting, hidden) = redact_setting(&name, &raw);
    SettingOut {
        name,
        setting,
        unit: r.get(2),
        source: r.get::<_, Option<String>>(3).unwrap_or_default(),
        context: r.get::<_, Option<String>>(4).unwrap_or_default(),
        pending_restart: r.get::<_, Option<bool>>(5).unwrap_or(false),
        hidden,
    }
}

/// `get_settings`: `pg_settings` узла; секреты — `redact_setting` (§6.2).
async fn settings(st: &AppState, p: SettingsParams) -> Result<PgAnswer, ToolError> {
    open_for_llm(st, &p.connection_id).await?;
    let member = member_or_leader(st, &p.connection_id, p.member.as_deref())?;
    let (_, profile) = st
        .runtimes
        .target(&p.connection_id, Some(&member))
        .map_err(tool_error)?;
    let names: Option<Vec<String>> = p
        .names
        .map(|v| v.into_iter().map(|n| n.trim().to_lowercase()).collect());
    let changed_only = names.is_none() && p.changed_only.unwrap_or(true);
    let rows = on_catalog_node_within(
        catalog_limit(&profile),
        st.client(&p.connection_id, Some(&member), None),
        move |c| {
            Box::pin(async move {
                c.query(SETTINGS_SQL, &[&names, &changed_only])
                    .await
                    .map_err(|e| from_pg(&e))
            })
        },
    )
    .await
    .map_err(tool_error)?;
    Ok(answer(PgOutput::Settings(
        rows.iter().map(setting_out).collect(),
    )))
}

/// Срок EXPLAIN модели, секунды (спец. §10.1): `statement_timeout` транзакции защиты и шагов hypopg.
pub const EXPLAIN_TIMEOUT_S: u32 = 5;
const GENERIC_WARNING: &str = "The plan is generic (GENERIC_PLAN): parameter values are unknown, estimates use average selectivity";
/// Параметры `$N` (§5.2): значения модели; без них — `GENERIC_PLAN` (PostgreSQL 16+, на старых значения обязательны);
/// `genericPlan` — явная просьба об обобщённом плане.
fn plan_params(
    sql: &str,
    values: Option<&[Option<String>]>,
    generic: bool,
    version_num: i32,
) -> Result<Params, ToolError> {
    let expected = count_params(sql);
    match values {
        Some(_) if generic => Err(invalid_argument(
            "Pass either params or genericPlan, not both",
        )),
        Some(v) if v.len() != expected => Err(invalid_argument(format!(
            "The query has {expected} parameters, {} values given",
            v.len()
        ))),
        Some([]) => Ok(Params::None),
        Some(v) => Ok(Params::Values { values: v.to_vec() }),
        None if generic || expected > 0 => {
            if version_num < V16 {
                Err(invalid_argument(format!(
                    "GENERIC_PLAN needs PostgreSQL 16 or newer (server_version_num {version_num}): pass params with {expected} values"
                )))
            } else {
                Ok(Params::Generic)
            }
        }
        None => Ok(Params::None),
    }
}

fn plan_tree(raw: &Value) -> Result<pg_advisor::plan::PlanTree, ToolError> {
    from_json(raw)
        .map_err(|e| tool_error(AppError::internal_with(format!("plan JSON: {}", e.message))))
}

/// `explain_query` (§5.2, §7): `sqlcheck` до соединения, слои `GuardTx` (`prepare` под точкой сохранения, проба
/// VERBOSE-плана с поиском запрещённых функций), план пробы — ответ (`explain_answer`).
async fn explain(st: &AppState, p: ExplainParams) -> Result<PgAnswer, ToolError> {
    let (profile, access) = check_connection(st, &p.connection_id)?;
    let database = resolve_database(&profile, p.database.as_deref());
    let asked = asked(p.sql.as_deref(), p.query_id.as_deref())?;
    ensure_open(st, &p.connection_id).await?;
    let member = member_or_leader(st, &p.connection_id, p.member.as_deref())?;
    let source = sql_source(st, &p.connection_id, &member, &database, asked).await?;
    let tx = guard::begin(
        st,
        &p.connection_id,
        Some(&member),
        &database,
        EXPLAIN_TIMEOUT_S,
    )
    .await?;
    let probed = async {
        let params = plan_params(
            &normalize(&source.text),
            p.params.as_deref(),
            p.generic_plan.unwrap_or(false),
            tx.version_num(),
        )?;
        // Текст — как его прислала модель: `GuardTx` нормализует сам, позиции ошибок — в исходном тексте.
        tx.prepare_one_hinted(&source.text, source.syntax_hint.as_deref())
            .await?;
        let raw = tx.probe(&source.text, &params).await?;
        Ok::<_, ToolError>((params, raw))
    }
    .await;
    tx.finish().await;
    let (params, raw) = probed?;
    explain_answer(member, raw, &params, source.from_pgss && masks(access))
}

/// Ответ `explain_query`: текст плана в виде psql, итоги, рекомендации P3b (английские тексты), таблицы, предупреждения.
/// `mask` — план построен не по SQL модели (текст pgss) на уровне `diagnostics` (§6.1): выражения узлов плана
/// маскируются до разбора дерева (текст плана и рекомендации строятся уже из замаскированного), а рекомендации — ещё
/// раз целиком (`mask_recommendation`: параметры, команда, гипотеза); SQL модели и его план не маскируются.
fn explain_answer(
    member: String,
    mut raw: Value,
    params: &Params,
    mask: bool,
) -> Result<PgAnswer, ToolError> {
    let mut masked = mask.then(|| mask_plan_value(&mut raw));
    let tree = plan_tree(&raw)?;
    let mut report = pg_advisor::plan_rules::evaluate(&tree);
    if let Some(n) = masked.as_mut() {
        *n += report
            .recommendations
            .iter_mut()
            .map(mask_recommendation)
            .sum::<usize>();
    }
    let mut warnings = Vec::new();
    if *params == Params::Generic {
        warnings.push(GENERIC_WARNING.to_string());
    }
    if !report.skipped.is_empty() {
        warnings.push(format!(
            "Rules that need execution data were not checked (the gateway never executes queries): {}",
            report.skipped.join(", ")
        ));
    }
    Ok(PgAnswer {
        body: PgOutput::Explain(ExplainOut {
            member,
            plan_text: to_text(&tree),
            summary: PlanSummaryOut {
                total_cost: tree.root.total_cost,
                plan_rows: tree.root.plan_rows,
                planning_ms: tree.planning_ms,
            },
            recommendations: report
                .recommendations
                .iter()
                .map(recommendation_out)
                .collect(),
            tables: relations(&tree)
                .iter()
                .map(|r| match &r.schema {
                    Some(s) => format!("{s}.{}", r.name),
                    None => r.name.clone(),
                })
                .collect(),
            warnings,
        }),
        masked: masked.map(|n| n as u64),
        rows: None,
    })
}

/// `hypothetical_index` (§5.2): запрос — те же проверки и слои, что у `explain_query`, строки `indexes` — `sqlcheck`
/// (`HypoIndex`) и `validate_ddl` до соединения; затем на том же соединении (после `ROLLBACK` транзакции защиты, сессия —
/// с `standard_conforming_strings = on`) `hypopg::check_many`: все гипотезы в одной транзакции `READ ONLY` и
/// `hypopg_reset()`; соединение закрывается (`finish`, а при брошенном вызове — сброс `GuardTx`).
async fn hypo(st: &AppState, p: HypoIndexParams) -> Result<PgAnswer, ToolError> {
    let (profile, access) = check_connection(st, &p.connection_id)?;
    let database = resolve_database(&profile, p.database.as_deref());
    let asked = asked(p.sql.as_deref(), p.query_id.as_deref())?;
    let index_ddls = check_index_ddls(&p.indexes)?;
    ensure_open(st, &p.connection_id).await?;
    let member = member_or_leader(st, &p.connection_id, p.member.as_deref())?;
    let source = sql_source(st, &p.connection_id, &member, &database, asked).await?;
    let tx = guard::begin(
        st,
        &p.connection_id,
        Some(&member),
        &database,
        EXPLAIN_TIMEOUT_S,
    )
    .await?;
    let checked = async {
        let params = plan_params(
            &normalize(&source.text),
            p.params.as_deref(),
            false,
            tx.version_num(),
        )?;
        tx.prepare_one_hinted(&source.text, source.syntax_hint.as_deref())
            .await?;
        tx.probe(&source.text, &params).await?;
        tx.rollback().await?;
        // Исходный текст: `check_many` нормализует его сам (`validate_many`), а позиции ошибок EXPLAIN считает по нему.
        let req = HypoManyRequest {
            member,
            database: database.clone(),
            sql: source.text.clone(),
            params,
            search_path: None,
            index_ddls,
            timeout_s: EXPLAIN_TIMEOUT_S,
        };
        hypopg::check_many(tx.client(), &req)
            .await
            .map_err(sql_error)
    }
    .await;
    tx.finish().await;
    hypo_answer(checked?, source.from_pgss && masks(access))
}

/// Ответ `hypothetical_index`; `mask` — как у `explain_answer`: выражения плана «после» по тексту pgss маскируются, и
/// его текст строится из замаскированного JSON. DDL гипотез — от модели, не маскируются.
fn hypo_answer(mut r: HypoManyResult, mask: bool) -> Result<PgAnswer, ToolError> {
    let masked = if mask {
        let n = mask_plan_value(&mut r.raw);
        r.plan = plan_tree(&r.raw)?;
        Some(n as u64)
    } else {
        None
    };
    Ok(PgAnswer {
        body: PgOutput::Hypo(HypoOut {
            cost_before: r.cost_before,
            cost_after: r.cost_after,
            indexes: r
                .indexes
                .into_iter()
                .map(|i| HypoIndexOut {
                    ddl: i.ddl,
                    used: i.used,
                    estimated_size_bytes: Some(i.estimated_size_bytes),
                })
                .collect(),
            plan_text: to_text(&r.plan),
        }),
        masked,
        rows: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glafira_pg_mcp::params::ActivityParams;
    use pg_advisor::findings::{Finding, FindingTarget, Severity};
    use pg_advisor::rules::sessions::{SessionRow, SessionsData};
    use serde_json::json;

    use super::*;
    use crate::catalog::table::Partitioning;
    use crate::conn::profile::{ConnectionProfile, sample_profile};
    use crate::diag::pulse::PulseData;
    use crate::error::AppResult;
    use crate::secrets::MemoryStore;
    use crate::topology::BoxFuture;
    use crate::topology::model::{Role, member, topo};
    use crate::topology::runtime::{Runtimes, TopologySource};

    /// Источник топологии, отдающий одну и ту же топологию (подключение открывается без стенда).
    struct Fixed(Topology);

    impl TopologySource for Fixed {
        fn observe<'a>(
            &'a self,
            _profile: &'a ConnectionProfile,
            _previous: Option<&'a Topology>,
        ) -> BoxFuture<'a, AppResult<Topology>> {
            Box::pin(async move { Ok(self.0.clone()) })
        }
    }

    /// Подключения `off` (уровень по умолчанию) и `on` (`diagnostics`), топология — `members`.
    fn state_with(members: Vec<Member>) -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let mut st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        st.runtimes = Runtimes::new(
            Arc::new(Fixed(topo(members))),
            st.pools.clone(),
            st.tunnels.clone(),
        );
        st.connections.upsert(sample_profile("off")).unwrap();
        let mut on = sample_profile("on");
        on.llm_access = LlmAccess::Diagnostics;
        st.connections.upsert(on).unwrap();
        (dir, st)
    }

    fn ok(r: Result<PgAnswer, ToolError>) -> PgAnswer {
        match r {
            Ok(a) => a,
            Err(e) => panic!("{}: {}", e.code, e.message),
        }
    }

    fn err(r: Result<PgAnswer, ToolError>) -> ToolError {
        match r {
            Ok(_) => panic!("ожидалась ошибка"),
            Err(e) => e,
        }
    }

    fn topology_of(id: &str) -> PgRead {
        PgRead::Topology(serde_json::from_value(json!({ "connectionId": id })).unwrap())
    }

    #[tokio::test]
    async fn only_connections_open_for_llm_are_listed() {
        let (_dir, st) = state_with(vec![member("pg-1", Role::Primary)]);
        let a = ok(read_op(&st, PgRead::ListConnections).await);
        let PgOutput::Connections(list) = a.body else {
            panic!("не список подключений")
        };
        let v = serde_json::to_value(&list).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1, "{v}");
        assert_eq!(
            (
                &v[0]["id"],
                &v[0]["kind"],
                &v[0]["llmAccess"],
                &v[0]["environment"]
            ),
            (
                &json!("on"),
                &json!("server"),
                &json!("diagnostics"),
                &json!("dev")
            )
        );
        assert_eq!(
            (&v[0]["connected"], &v[0]["folderPath"]),
            (&json!(false), &json!([]))
        );
        assert!(v[0].get("roleWarning").is_none());
        assert_eq!(
            v[0]["database"],
            json!(sample_profile("on").pg.database),
            "база подключения по умолчанию"
        );
        assert!(
            v[0].get("host").is_none()
                && v[0].get("username").is_none()
                && v[0].get("hosts").is_none(),
            "хосты и пользователь не отдаются (§6.2): {v}"
        );
    }

    #[tokio::test]
    async fn closed_for_llm_connection_is_not_opened_and_open_one_is() {
        let mut primary = member("pg-1", Role::Primary);
        primary.tags.insert("nofailover".into(), json!(true));
        let (_dir, st) = state_with(vec![primary, member("pg-2", Role::Replica)]);
        for id in ["off", "nope"] {
            let e = err(read_op(&st, topology_of(id)).await);
            assert_eq!(
                (e.code.as_str(), e.message.as_str()),
                (
                    "not_allowed",
                    "Unknown connection or LLM access is off for it; call list_connections"
                )
            );
        }
        assert_eq!(st.runtimes.view("off").state, ConnState::Disconnected);
        let a = ok(read_op(&st, topology_of("on")).await);
        assert_eq!(
            st.runtimes.view("on").state,
            ConnState::Connected,
            "открыто тем же путём, что щелчок в дереве"
        );
        let PgOutput::Topology(t) = a.body else {
            panic!("не топология")
        };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["kind"], "patroni");
        assert_eq!(v["members"].as_array().unwrap().len(), 2);
        assert_eq!(
            (&v["members"][0]["role"], &v["members"][0]["nofailover"]),
            (&json!("primary"), &json!(true))
        );
        assert!(v["members"][1].get("nofailover").is_none());
        assert_eq!(v["patroni"]["paused"], false);
        assert_eq!(v["events"], json!([]));
    }

    #[tokio::test]
    async fn catalog_needs_no_connection() {
        let (_dir, st) = state_with(vec![]);
        let a = ok(read_op(&st, PgRead::ListDiagnostics).await);
        let PgOutput::Diagnostics(tools) = a.body else {
            panic!("не каталог")
        };
        assert_eq!(tools.len(), 10);
    }

    #[test]
    fn report_goes_to_the_model_with_english_texts() {
        let r = DiagReport {
            tool: Tool::Sessions,
            member: Some("pg-1".into()),
            follows_leader: true,
            observed_at_ms: 5,
            server_version_num: Some(170_004),
            in_recovery: Some(false),
            data: DiagData::Sessions(SessionsData::default()),
            findings: vec![
                Finding::new("idleInTransaction", Severity::Warning)
                    .member(Some("pg-1"))
                    .param("maxS", 3725.0)
                    .link_tool("idleTx", Some("pg-1")),
            ],
            unavailable: vec![diag::needs_monitor("queryId")],
        };
        let v = serde_json::to_value(report_out(r)).unwrap();
        assert_eq!(
            (&v["tool"], &v["followsLeader"]),
            (&json!("sessions"), &json!(true))
        );
        assert_eq!(v["data"]["kind"], "sessions");
        assert_eq!(
            v["findings"][0]["title"],
            "Idle in transaction for 1 h 2 min"
        );
        assert!(v["findings"][0].get("links").is_none());
        assert_eq!(
            v["unavailable"][0]["reason"],
            "queryId: needs the pg_monitor role"
        );
        assert_eq!(v["truncated"], false);
    }

    fn report(tool: Tool, data: DiagData, findings: Vec<Finding>) -> DiagReport {
        DiagReport {
            tool,
            member: None,
            follows_leader: false,
            observed_at_ms: 1,
            server_version_num: None,
            in_recovery: None,
            data,
            findings,
            unavailable: vec![],
        }
    }

    #[test]
    fn health_summary_merges_findings_and_lists_failed_tools() {
        let idle = || {
            Finding::new("idleInTransaction", Severity::Warning)
                .member(Some("pg-1"))
                .param("maxS", 120.0)
        };
        let pulse = report(
            Tool::Pulse,
            DiagData::Pulse(PulseData { nodes: vec![] }),
            vec![
                idle(),
                Finding::new("cacheHitLow", Severity::Info).member(Some("pg-1")),
            ],
        );
        let sessions = report(
            Tool::Sessions,
            DiagData::Sessions(SessionsData::default()),
            vec![idle().target(FindingTarget::Session {
                pid: 7,
                backend_start_us: 1,
            })],
        );
        let h = health_of(vec![
            (Tool::Pulse, Ok(pulse)),
            (Tool::Sessions, Ok(sessions)),
            (Tool::Locks, Err("did not finish within 20 s".into())),
        ]);
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(v["pulse"]["kind"], "pulse");
        let ids: Vec<&str> = h.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            ids,
            ["idleInTransaction", "cacheHitLow"],
            "по серьёзности, без повтора пульса"
        );
        assert_eq!(
            h.findings[0].target,
            Some(json!({"kind": "session", "pid": 7, "backendStartUs": 1}))
        );
        assert_eq!(
            (&v["unavailable"][0]["tool"], &v["unavailable"][0]["reason"]),
            (&json!("locks"), &json!("did not finish within 20 s"))
        );
    }

    #[test]
    fn activity_filters_and_orders_by_duration() {
        let row = |pid, state: &str, xact: Option<f64>, query: Option<f64>, wait: Option<&str>| {
            SessionRow {
                pid,
                state: Some(state.into()),
                xact_age_s: xact,
                query_age_s: query,
                wait_event_type: wait.map(str::to_string),
                database: Some("orders".into()),
                application: Some("app".into()),
                ..Default::default()
            }
        };
        let rows = vec![
            row(1, "active", Some(5.0), Some(5.0), None),
            row(
                2,
                "idle in transaction",
                Some(300.0),
                Some(299.0),
                Some("Client"),
            ),
            row(3, "idle", None, Some(9_999.0), Some("Client")),
            row(4, "active", Some(60.0), Some(60.0), Some("Lock")),
        ];
        let p = |v: serde_json::Value| -> ActivityParams { serde_json::from_value(v).unwrap() };
        let (total, got) = filter_sessions(rows.clone(), &p(json!({"connectionId": "c"})));
        assert_eq!(total, 4);
        assert_eq!(
            got.iter().map(|r| r.pid).collect::<Vec<_>>(),
            [2, 4, 1, 3],
            "idle — без длительности, в конце"
        );
        let (total, got) = filter_sessions(
            rows.clone(),
            &p(json!({"connectionId": "c", "minDurationS": 30, "limit": 1})),
        );
        assert_eq!((total, got.len(), got[0].pid), (2, 1, 2));
        let (_, got) = filter_sessions(
            rows.clone(),
            &p(json!({"connectionId": "c", "state": "ACTIVE", "waitEventType": "lock"})),
        );
        assert_eq!(got.iter().map(|r| r.pid).collect::<Vec<_>>(), [4]);
        let (total, _) =
            filter_sessions(rows, &p(json!({"connectionId": "c", "database": "other"})));
        assert_eq!(total, 0);
    }

    #[test]
    fn diagnostic_arguments_are_checked() {
        assert!(matches!(parse_tool("locks"), Ok(Tool::Locks)));
        let e = parse_tool("idleTx").err().unwrap();
        assert_eq!(e.code, "invalid_argument");
        assert!(e.message.contains("list_diagnostics"), "{}", e.message);
        let q = queries_params(None).ok().unwrap();
        assert_eq!((q.mode, q.limit), (QueriesMode::SinceReset, 50));
        let q = queries_params(Some(
            json!({"mode": {"kind": "window", "s": 900}, "sort": "calls"}),
        ))
        .ok()
        .unwrap();
        assert_eq!(q.mode, QueriesMode::Window { s: 900 });
        let e = queries_params(Some(
            json!({"mode": {"kind": "between", "from": "a", "to": "b"}}),
        ))
        .err()
        .unwrap();
        assert_eq!(e.code, "invalid_argument");
        let e = queries_params(Some(json!("sinceReset"))).err().unwrap();
        assert_eq!(e.code, "invalid_argument");
    }

    // Сверх плана: проверки до открытия подключения, разбор `queries`, путь папки, фильтр приложения и брошенный вызов.

    fn diagnostic(id: &str, tool: &str, extra: Value) -> PgRead {
        let mut v = json!({ "connectionId": id, "tool": tool });
        if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
            o.extend(e);
        }
        PgRead::RunDiagnostic(serde_json::from_value(v).unwrap())
    }

    #[tokio::test]
    async fn diagnostic_checks_access_then_arguments_before_opening() {
        let (_dir, st) = state_with(vec![member("pg-1", Role::Primary)]);
        let e = err(read_op(&st, diagnostic("off", "idleTx", json!({}))).await);
        assert_eq!(e.code, "not_allowed", "доступ — раньше аргументов");
        let e = err(read_op(&st, diagnostic("on", "idleTx", json!({}))).await);
        assert_eq!(e.code, "invalid_argument");
        let e = err(read_op(&st, diagnostic("on", "failover", json!({}))).await);
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "FEATURE_UNAVAILABLE",
                "This tool is available only for Patroni clusters"
            )
        );
        let e = err(read_op(
            &st,
            diagnostic("on", "queries", json!({"queries": {"limit": 30}})),
        )
        .await);
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            ("invalid_argument", "The row limit must be 50, 100 or 200")
        );
        assert_eq!(
            st.runtimes.view("on").state,
            ConnState::Disconnected,
            "неверный вызов не открывает подключение"
        );
    }

    #[test]
    fn queries_arguments_are_validated_like_the_interface() {
        let q = queries_params(Some(
            json!({"limit": 200, "database": "orders", "search": "orders"}),
        ))
        .ok()
        .unwrap();
        assert_eq!(
            (q.limit, q.database.as_deref(), q.search()),
            (200, Some("orders"), Some("orders"))
        );
        assert!(
            q.toplevel_only,
            "по умолчанию — только верхний уровень, как у вкладки"
        );
        for bad in [
            json!({"sort": "fastest"}),
            json!({"limit": 0}),
            json!({"limit": -1}),
            json!({"mode": {"kind": "window", "s": 60}}),
            json!({"mode": {"kind": "window"}}),
            json!({"mode": "window"}),
        ] {
            let e = queries_params(Some(bad.clone())).err().unwrap();
            assert_eq!(e.code, "invalid_argument", "{bad}");
        }
    }

    #[test]
    fn folder_path_goes_from_the_root_and_survives_broken_files() {
        let f = |id: &str, name: &str, parent: Option<&str>| ConnectionFolder {
            id: id.into(),
            name: name.into(),
            parent_id: parent.map(str::to_string),
            order: 0,
            extra: Default::default(),
        };
        let folders = vec![
            f("a", "Prod", None),
            f("b", "EU", Some("a")),
            f("x", "X", Some("y")),
            f("y", "Y", Some("x")),
        ];
        assert_eq!(folder_path(&folders, "b"), ["Prod", "EU"]);
        assert_eq!(folder_path(&folders, "a"), ["Prod"]);
        assert!(folder_path(&folders, "gone").is_empty());
        assert!(
            folder_path(&folders, "x").len() <= folders.len(),
            "цикл не зацикливает"
        );
    }

    #[test]
    fn application_filter_is_a_case_insensitive_substring() {
        let row = |pid, app: Option<&str>| SessionRow {
            pid,
            application: app.map(str::to_string),
            ..Default::default()
        };
        let rows = vec![
            row(1, Some("billing-API")),
            row(2, Some("psql")),
            row(3, None),
        ];
        let p: ActivityParams =
            serde_json::from_value(json!({"connectionId": "c", "application": "api"})).unwrap();
        let (total, got) = filter_sessions(rows, &p);
        assert_eq!((total, got[0].pid), (1, 1));
    }

    /// Источник топологии, который отвечает только после сигнала.
    struct Slow(Arc<tokio::sync::Notify>, Topology);

    impl TopologySource for Slow {
        fn observe<'a>(
            &'a self,
            _profile: &'a ConnectionProfile,
            _previous: Option<&'a Topology>,
        ) -> BoxFuture<'a, AppResult<Topology>> {
            Box::pin(async move {
                self.0.notified().await;
                Ok(self.1.clone())
            })
        }
    }

    #[tokio::test]
    async fn dropped_call_does_not_leave_the_connection_half_open() {
        use futures_util::FutureExt;
        let (_dir, mut st) = state_with(vec![]);
        let release = Arc::new(tokio::sync::Notify::new());
        st.runtimes = Runtimes::new(
            Arc::new(Slow(
                release.clone(),
                topo(vec![member("pg-1", Role::Primary)]),
            )),
            st.pools.clone(),
            st.tunnels.clone(),
        );
        // Сессия MCP закрылась, пока подключение открывалось: тело вызова брошено на `await`.
        assert!(read_op(&st, topology_of("on")).now_or_never().is_none());
        for _ in 0..1000 {
            if st.runtimes.view("on").state == ConnState::Connecting {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(st.runtimes.view("on").state, ConnState::Connecting);
        release.notify_one();
        for _ in 0..1000 {
            if st.runtimes.view("on").state != ConnState::Connecting {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            st.runtimes.view("on").state,
            ConnState::Connected,
            "открытие доведено до конца без вызова"
        );
    }

    // Исправления первого раунда ревью: поиск по замаскированному тексту, предел текста сеанса, ошибки узлов.

    fn statement(id: i64, text: &str, calls: i64) -> pg_advisor::queries::QueryStat {
        use pg_advisor::queries::{Counters, QueryKey, Totals, stat};
        let mut row = stat(
            QueryKey {
                query_id: id,
                dbid: 1,
                userid: 10,
                toplevel: true,
            },
            Some("orders".into()),
            None,
            &Counters {
                calls,
                ..Counters::default()
            },
            None,
            &Totals::default(),
        );
        row.text = Some(text.into());
        row
    }

    /// Отчёт `queries` с ответом сервера на поиск без `search`: строки топа с исходными текстами pgss.
    fn statements_report() -> DiagReport {
        use crate::diag::queries::QueriesData;
        use pg_advisor::queries::Totals;
        let query = |id: i64| {
            Finding::new("query_n_plus_one", Severity::Warning).target(FindingTarget::Query {
                query_id: id.to_string(),
                database: "orders".into(),
            })
        };
        report(
            Tool::Queries,
            DiagData::Queries(QueriesData {
                totals: Totals {
                    calls: 1_000,
                    ..Totals::default()
                },
                rows: vec![
                    statement(1, "ALTER ROLE app PASSWORD 'hunter2'", 1),
                    statement(2, "SELECT * FROM orders WHERE note = 'hunter'", 900),
                    statement(3, "SELECT 1", 99),
                ],
                ..QueriesData::empty(QueriesMode::SinceReset)
            }),
            vec![
                query(1),
                query(2),
                Finding::new("queries_incomplete", Severity::Warning),
            ],
        )
    }

    fn texts(r: &DiagReport) -> Vec<String> {
        let DiagData::Queries(d) = &r.data else {
            panic!("не queries")
        };
        d.rows.iter().filter_map(|r| r.text.clone()).collect()
    }

    /// `search` на уровне `diagnostics` ищет по замаскированному тексту: по исходному ответ «есть / нет строк»
    /// выдавал бы литералы по символу (решение владельца 2026-11-04).
    #[test]
    fn diagnostics_search_looks_only_at_masked_texts() {
        let mut q = queries_params(Some(json!({"search": "hunter", "limit": 50}))).unwrap();
        let s = take_masked_search(&mut q, LlmAccess::Diagnostics).expect("отложен");
        assert_eq!(
            (q.search(), q.limit),
            (None, 200),
            "серверу поиск не уходит, топ шире"
        );
        let mut r = statements_report();
        assert_eq!(
            mask_statements(LlmAccess::Diagnostics, &mut r, Some(&s), None),
            Some(0),
            "литералы считаются только у отданных строк"
        );
        assert!(texts(&r).is_empty(), "{:?}", texts(&r));
        assert_eq!(
            r.findings.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["queries_incomplete"],
            "находки строк вне ответа уходят"
        );
        let DiagData::Queries(d) = &r.data else {
            unreachable!()
        };
        assert_eq!(d.totals.calls, 1_000, "итоги не меняются");

        let mut q = queries_params(Some(json!({"search": "alter ROLE"}))).unwrap();
        let s = take_masked_search(&mut q, LlmAccess::Diagnostics).unwrap();
        let mut r = statements_report();
        assert_eq!(
            mask_statements(LlmAccess::Diagnostics, &mut r, Some(&s), None),
            Some(1)
        );
        assert_eq!(texts(&r), ["ALTER ROLE app PASSWORD $1"]);
        assert_eq!(r.findings.len(), 2, "находка найденной строки остаётся");

        // Совпадение по замаскированному тексту: `$1` есть у всех трёх строк; строк — не больше предела запроса.
        let mut q = queries_params(Some(json!({"search": "$1"}))).unwrap();
        let mut s = take_masked_search(&mut q, LlmAccess::Diagnostics).unwrap();
        s.limit = 2;
        let mut r = statements_report();
        assert_eq!(
            mask_statements(LlmAccess::Diagnostics, &mut r, Some(&s), None),
            Some(2)
        );
        assert_eq!(
            texts(&r),
            [
                "ALTER ROLE app PASSWORD $1",
                "SELECT * FROM orders WHERE note = $1"
            ]
        );
    }

    /// Без поиска строки режутся до предела ответа после маскирования, и в журнал идут литералы оставшихся строк;
    /// без маскирования тексты и счёт не трогаются.
    #[test]
    fn statements_are_cut_to_the_answer_limit_and_counted_after_the_cut() {
        let mut r = statements_report();
        assert_eq!(
            mask_statements(LlmAccess::Diagnostics, &mut r, None, Some(1)),
            Some(1)
        );
        assert_eq!(texts(&r), ["ALTER ROLE app PASSWORD $1"]);
        assert_eq!(r.findings.len(), 3, "без поиска находки не сужаются");
        let mut r = statements_report();
        assert_eq!(mask_statements(LlmAccess::Off, &mut r, None, Some(2)), None);
        assert_eq!(
            texts(&r),
            [
                "ALTER ROLE app PASSWORD 'hunter2'",
                "SELECT * FROM orders WHERE note = 'hunter'"
            ]
        );
        // Отчёт другого инструмента — как `mask_if`.
        let mut r = report(
            Tool::Sessions,
            DiagData::Sessions(SessionsData {
                rows: vec![SessionRow {
                    pid: 1,
                    query: Some("SELECT 'x'".into()),
                    ..Default::default()
                }],
                settings: Default::default(),
            }),
            vec![],
        );
        assert_eq!(
            mask_statements(LlmAccess::Diagnostics, &mut r, None, Some(0)),
            Some(1)
        );
    }

    #[test]
    fn search_without_masking_stays_on_the_server() {
        let mut q = queries_params(Some(json!({"search": "hunter"}))).unwrap();
        assert_eq!(take_masked_search(&mut q, LlmAccess::Off), None);
        assert_eq!((q.search(), q.limit), (Some("hunter"), 50));
        let mut q = queries_params(Some(json!({"search": "  "}))).unwrap();
        assert_eq!(
            take_masked_search(&mut q, LlmAccess::Diagnostics),
            None,
            "пусто — не поиск"
        );
        assert_eq!(q.limit, 50);
    }

    /// Текст сеанса в `get_activity` — не длиннее 2 КБ, обрезка по символу после маскирования (как `observe_activity`).
    #[test]
    fn activity_texts_are_masked_then_cut_to_two_kilobytes() {
        let long = format!(
            "SELECT * FROM t WHERE email = 'ann@example.com' AND x IN ({}) /* ё */",
            "aaaa, ".repeat(600)
        );
        let rows = vec![
            SessionRow {
                pid: 1,
                state: Some("active".into()),
                query_age_s: Some(9.0),
                query: Some(long.clone()),
                ..Default::default()
            },
            SessionRow {
                pid: 2,
                state: Some("active".into()),
                query_age_s: Some(1.0),
                query: Some("SELECT 'короткий'".into()),
                ..Default::default()
            },
        ];
        let r = report(
            Tool::Sessions,
            DiagData::Sessions(SessionsData {
                rows,
                settings: Default::default(),
            }),
            vec![],
        );
        let p: ActivityParams = serde_json::from_value(json!({"connectionId": "c"})).unwrap();
        let a = activity_answer(r.clone(), &p, LlmAccess::Diagnostics).unwrap();
        assert_eq!(a.masked, Some(2));
        let PgOutput::Activity(out) = a.body else {
            panic!("не активность")
        };
        let first = out.sessions[0]["query"].as_str().unwrap();
        assert!(
            first.len() <= MAX_QUERY_BYTES && first.len() > MAX_QUERY_BYTES - 4,
            "{}",
            first.len()
        );
        assert!(
            first.starts_with("SELECT * FROM t WHERE email = $1 AND"),
            "{first}"
        );
        assert_eq!(out.sessions[1]["query"], "SELECT $1");
        // Без маскирования текст тоже обрезается.
        let a = activity_answer(r, &p, LlmAccess::Off).unwrap();
        let PgOutput::Activity(out) = a.body else {
            panic!("не активность")
        };
        let first = out.sessions[0]["query"].as_str().unwrap();
        assert_eq!(first, &long[..MAX_QUERY_BYTES]);
        assert_eq!(a.masked, None);
    }

    /// Ошибки узлов в данных (`PulseNode.error`, `HealthCell.error`) уходят модели без цели ssh, путей и `details`.
    #[test]
    fn node_errors_in_reports_lose_ssh_targets_and_details() {
        use crate::diag::patroni::{HealthCell, HealthData, HealthMember};
        use crate::diag::pulse::PulseNode;
        use crate::net::ssh::process::{SshErrorKind, ssh_error};
        use pg_advisor::rules::patroni::HealthGrade;
        let ssh = || {
            ssh_error(
                SshErrorKind::AuthFailed,
                "admin@bastion.corp.example",
                "admin@bastion.corp.example: Permission denied",
            )
        };
        let pulse = DiagData::Pulse(PulseData {
            nodes: vec![PulseNode {
                member: "pg-1".into(),
                role: Role::Primary,
                reachable: false,
                error: Some(ssh()),
                facts: None,
                rates: None,
            }],
        });
        let health = DiagData::Health(HealthData {
            endpoints: vec!["/health".into()],
            members: vec![HealthMember {
                member: "pg-1".into(),
                api_url: Some("http://10.0.0.1:8008".into()),
                cells: vec![HealthCell {
                    endpoint: "/health".into(),
                    status: None,
                    error: Some(ssh()),
                    grade: HealthGrade::Bad,
                }],
            }],
        });
        for (tool, data) in [(Tool::Pulse, pulse.clone()), (Tool::Health, health)] {
            let v = serde_json::to_string(&report_out(report(tool, data, vec![]))).unwrap();
            assert!(
                !v.contains("bastion") && !v.contains("Permission") && v.contains("SSH_ERROR"),
                "{v}"
            );
        }
        let h = health_of(vec![(
            Tool::Pulse,
            Ok(report(Tool::Pulse, pulse.clone(), vec![])),
        )]);
        let v = serde_json::to_string(&h).unwrap();
        assert!(!v.contains("bastion") && v.contains("SSH_ERROR"), "{v}");
        // Без цели и текста ssh модель всё же узнаёт, что именно не так: вид ошибки остаётся.
        assert!(v.contains("\"kind\":\"authFailed\""), "{v}");
    }

    // Задача 11: топ, карточка, таблица, настройки.

    #[test]
    fn top_queries_request_maps_mode_sort_and_limit() {
        let p = |v: serde_json::Value| -> TopQueriesParams { serde_json::from_value(v).unwrap() };
        let (q, limit, window) = top_request(&p(json!({"connectionId": "c"}))).ok().unwrap();
        assert_eq!(
            (q.mode, q.sort, q.limit, limit, window),
            (QueriesMode::SinceReset, QuerySort::TotalTime, 50, 10, false)
        );
        assert!(q.toplevel_only);
        let (q, limit, window) = top_request(&p(json!({
            "connectionId": "c", "mode": "window", "windowS": 300, "limit": 70, "sort": "calls",
            "database": "orders", "search": "shop.orders"
        })))
        .ok()
        .unwrap();
        assert_eq!(
            (q.mode, q.sort, q.limit, limit, window),
            (
                QueriesMode::Window { s: 300 },
                QuerySort::Calls,
                100,
                70,
                true
            )
        );
        assert_eq!(
            (q.database.as_deref(), q.search.as_deref()),
            (Some("orders"), Some("shop.orders"))
        );
        let (q, _, _) = top_request(&p(json!({"connectionId": "c", "mode": "window"})))
            .ok()
            .unwrap();
        assert_eq!(q.mode, QueriesMode::Window { s: 900 });
        let e = top_request(&p(json!({"connectionId": "c", "sort": "fastest"})))
            .err()
            .unwrap();
        assert_eq!(e.code, "invalid_argument");
        assert!(e.message.contains("totalTime"), "{}", e.message);
        let e = top_request(&p(json!({"connectionId": "c", "mode": "between"})))
            .err()
            .unwrap();
        assert_eq!(e.code, "invalid_argument");
        // E1: `windowS` без `mode` — окно и при прямом вызове (без `validate` крейта); явный `sinceReset` — нет.
        let (q, _, window) = top_request(&p(json!({"connectionId": "c", "windowS": 3600})))
            .ok()
            .unwrap();
        assert_eq!((q.mode, window), (QueriesMode::Window { s: 3600 }, true));
        let (q, _, window) = top_request(&p(
            json!({"connectionId": "c", "mode": "sinceReset", "windowS": 900}),
        ))
        .ok()
        .unwrap();
        assert_eq!((q.mode, window), (QueriesMode::SinceReset, false));
    }

    /// E1: `sinceReset` с присланным `windowS` — ответ «с момента сброса» и подсказка, что окно не применено; подсказки
    /// склеиваются, ни одна не теряется.
    #[test]
    fn since_reset_with_a_window_says_the_window_is_ignored() {
        let a = top_answer(
            statements_report(),
            LlmAccess::Diagnostics,
            None,
            20,
            false,
            true,
        );
        let PgOutput::Report(r) = &a.body else {
            unreachable!()
        };
        assert_eq!(r.hint.as_deref(), Some(SINCE_RESET_WINDOW_HINT));
        assert!(
            SINCE_RESET_WINDOW_HINT.starts_with("windowS is ignored in mode sinceReset")
                && SINCE_RESET_WINDOW_HINT.contains("use mode window"),
            "{SINCE_RESET_WINDOW_HINT}"
        );
        assert!(r.data.get("coverageS").is_none_or(|c| c != 0), "{}", r.data);
        // Подсказки без точки (как `NO_WINDOW_HINT`, `SINCE_RESET_WINDOW_HINT`): склейка ставит границу предложения.
        let mut hint = Some("First".to_string());
        add_hint(&mut hint, "Second");
        assert_eq!(hint.as_deref(), Some("First. Second."));
        let mut hint = None;
        add_hint(&mut hint, "Only.");
        assert_eq!(hint.as_deref(), Some("Only."));
    }

    /// E3: базы фильтра нет на узле — `not_found` со списком баз узла (без шаблонов и баз без `datallowconn`, по
    /// алфавиту, не больше `MAX_DATABASE_NAMES`) и базой подключения; есть (даже шаблон) — отказа нет.
    #[test]
    fn missing_filter_database_lists_the_databases_of_the_node() {
        let all = |names: &[(&str, bool)]| -> Vec<(String, bool)> {
            names.iter().map(|(n, l)| (n.to_string(), *l)).collect()
        };
        let node = all(&[
            ("postgres", true),
            ("template1", false),
            ("perf", true),
            ("template0", false),
            ("Archive", true),
        ]);
        let e = database_missing("orders", "pg-1", node.clone(), "perf").unwrap();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "not_found",
                "Database orders does not exist on pg-1; databases: Archive, perf, postgres (the connection's database: perf)"
            )
        );
        assert!(database_missing("perf", "pg-1", node.clone(), "perf").is_none());
        assert!(
            database_missing("template1", "pg-1", node, "perf").is_none(),
            "база есть, хоть и не в списке"
        );
        let many: Vec<(String, bool)> = (0..55).map(|i| (format!("db{i:02}"), true)).collect();
        let e = database_missing("nope", "pg-2", many, "db00").unwrap();
        assert!(
            e.message.contains("db00, db01")
                && e.message
                    .contains("db49, …and 5 more (the connection's database: db00)")
                && !e.message.contains("db50"),
            "{}",
            e.message
        );
    }

    #[test]
    fn column_stat_values_are_cut_and_columns_follow_the_table() {
        let long = "x".repeat(MAX_STATS_VALUE_BYTES + 10);
        let v = cut_values(json!([long, "short", null, ["nested"]]));
        let first = v[0].as_str().unwrap();
        assert!(first.ends_with("…[truncated]"), "{first}");
        assert_eq!(first.len(), MAX_STATS_VALUE_BYTES + "…[truncated]".len());
        assert_eq!(
            (&v[1], &v[2], &v[3]),
            (&json!("short"), &json!(null), &json!(["nested"]))
        );
        let euro = "€".repeat(MAX_STATS_VALUE_BYTES);
        assert!(
            cut_values(json!(euro))
                .as_str()
                .unwrap()
                .ends_with("€…[truncated]"),
            "граница символа"
        );
        let col = |name: &str| Column {
            name: name.into(),
            data_type: "text".into(),
            not_null: false,
            default_expr: None,
            identity: None,
            generated: None,
            collation: None,
            stat_target: None,
            storage: "x".into(),
            comment: None,
        };
        let stat = |c: &str| ColumnStat {
            column: c.into(),
            null_frac: 0.0,
            avg_width: 4,
            n_distinct: -1.0,
            correlation: None,
            most_common_vals: None,
            most_common_freqs: None,
            histogram_bounds: None,
        };
        let ordered = order_like(
            vec![stat("status"), stat("zzz"), stat("id")],
            &[col("id"), col("status")],
        );
        let names: Vec<&str> = ordered.iter().map(|s| s.column.as_str()).collect();
        assert_eq!(names, ["id", "status", "zzz"]);
    }

    #[test]
    fn catalog_sql_of_the_gateway_is_marked() {
        for sql in [STATS_SQL, STATS_VALUES_SQL, SETTINGS_SQL] {
            assert!(crate::product::is_marked_sql(sql), "{sql}");
        }
        assert!(STATS_SQL.contains("pg_stats") && !STATS_SQL.contains("most_common_vals"));
        assert!(SETTINGS_SQL.contains("'client', 'session'"));
        // `DateStyle`, `TimeZone`, `IntervalStyle` в `pg_settings` — с заглавными: имена сравниваются без регистра.
        assert!(SETTINGS_SQL.contains("lower(name) = ANY($1)"));
    }

    #[test]
    fn gateway_sorts_are_exactly_the_query_sorts() {
        use pg_advisor::queries::QuerySort;
        for s in glafira_pg_mcp::params::QUERY_SORTS {
            let parsed: QuerySort = serde_json::from_value(serde_json::json!(s))
                .unwrap_or_else(|_| panic!("sort {s} is not a QuerySort"));
            assert_eq!(serde_json::to_value(parsed).unwrap(), serde_json::json!(s));
        }
        // У `QuerySort` 14 вариантов (`cpuTime` — P3b); новый вариант добавляется и в `QUERY_SORTS`.
        assert_eq!(glafira_pg_mcp::params::QUERY_SORTS.len(), 14);
    }

    // Сверх плана: порядок топа (поиск по замаскированному тексту до предела ответа), карточка для модели и проверки
    // до открытия подключения.

    fn top_rows(a: &PgAnswer) -> Vec<String> {
        let PgOutput::Report(r) = &a.body else {
            panic!("не отчёт")
        };
        r.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["text"].as_str().unwrap().to_string())
            .collect()
    }

    /// `get_top_queries` на уровне `diagnostics`: поиск идёт по замаскированным текстам расширенного топа, и только
    /// потом строки режутся до `limit`; в журнал — литералы отданных строк.
    #[test]
    fn top_answer_searches_masked_texts_before_the_limit() {
        let p: TopQueriesParams =
            serde_json::from_value(json!({"connectionId": "c", "search": "select", "limit": 1}))
                .unwrap();
        let (mut q, limit, window) = top_request(&p).ok().unwrap();
        let s = take_masked_search(&mut q, LlmAccess::Diagnostics).expect("поиск отложен");
        assert_eq!(q.limit, 200);
        let a = top_answer(
            statements_report(),
            LlmAccess::Diagnostics,
            Some(&s),
            limit,
            window,
            false,
        );
        assert_eq!(top_rows(&a), ["SELECT * FROM orders WHERE note = $1"]);
        assert_eq!(a.masked, Some(1));
        let PgOutput::Report(r) = &a.body else {
            unreachable!()
        };
        let ids: Vec<&str> = r.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            ids,
            ["query_n_plus_one", "queries_incomplete"],
            "находка строки вне поиска ушла"
        );
        assert!(r.hint.is_none());

        // Без поиска — первые `limit` строк, литералы только у них.
        let a = top_answer(
            statements_report(),
            LlmAccess::Diagnostics,
            None,
            2,
            false,
            false,
        );
        assert_eq!(
            top_rows(&a),
            [
                "ALTER ROLE app PASSWORD $1",
                "SELECT * FROM orders WHERE note = $1"
            ]
        );
        assert_eq!(a.masked, Some(2));

        // Окно без истории: покрытие 0 и подсказка.
        let a = top_answer(
            statements_report(),
            LlmAccess::Diagnostics,
            None,
            20,
            true,
            false,
        );
        let PgOutput::Report(r) = &a.body else {
            unreachable!()
        };
        assert_eq!(r.data["coverageS"], 0);
        assert!(
            r.hint
                .as_deref()
                .is_some_and(|h| h.contains("observe_top_queries")),
            "{:?}",
            r.hint
        );
    }

    fn running(query: String) -> crate::diag::query_detail::RunningInstance {
        crate::diag::query_detail::RunningInstance {
            pid: 1,
            backend_start_us: 1,
            state: Some("active".into()),
            started_ms: None,
            duration_s: Some(3.0),
            wait_event_type: None,
            wait_event: None,
            usename: Some("app".into()),
            application_name: None,
            client_addr: None,
            query,
            truncated: false,
        }
    }

    /// Карточка для модели: тексты замаскированы, тексты выполняющихся экземпляров — не длиннее 2 КБ (как у
    /// `get_activity`), находки и причины недоступности — английским текстом, узел — в поле `member`.
    #[test]
    fn query_card_goes_to_the_model_masked_and_in_english() {
        let card = || {
            let mut d = QueryDetail::unavailable_pgss(170_000, false);
            d.text = Some("SELECT * FROM t WHERE email = 'ann@example.com'".into());
            d.running = vec![
                running(format!("SELECT 'x', {}", "col, ".repeat(600))),
                running("SELECT 7".into()),
            ];
            d.findings = vec![
                Finding::new("query_n_plus_one", Severity::Warning)
                    .member(Some("pg-1"))
                    .param("callsSharePct", 40.0)
                    .param("meanMs", 0.2)
                    .param("calls", 1000),
            ];
            d
        };
        let a = query_detail_answer(card(), "pg-1", LlmAccess::Diagnostics).unwrap();
        assert_eq!(a.masked, Some(3), "текст карточки и два экземпляра");
        let PgOutput::QueryDetails(v) = &a.body else {
            panic!("не карточка")
        };
        assert_eq!(v["member"], "pg-1");
        assert_eq!(v["text"], "SELECT * FROM t WHERE email = $1");
        let first = v["running"][0]["query"].as_str().unwrap();
        assert!(
            first.starts_with("SELECT $1, col,") && first.len() <= MAX_QUERY_BYTES,
            "{first}"
        );
        assert_eq!(
            v["running"][0]["truncated"], true,
            "обрезанный текст помечен"
        );
        assert_eq!(
            (&v["running"][1]["query"], &v["running"][1]["truncated"]),
            (&json!("SELECT $1"), &json!(false))
        );
        assert_eq!(
            v["unavailable"][0]["reason"],
            "statements: pg_stat_statements is not installed in this database (CREATE EXTENSION pg_stat_statements)"
        );
        assert!(
            v["findings"][0]["title"]
                .as_str()
                .is_some_and(|t| t.contains("calls")),
            "{v}"
        );
        assert!(v["findings"][0].get("links").is_none());
        // Без маскирования текст как есть, а длинный текст экземпляра всё равно режется.
        let a = query_detail_answer(card(), "pg-1", LlmAccess::Off).unwrap();
        let PgOutput::QueryDetails(v) = &a.body else {
            panic!("не карточка")
        };
        assert_eq!(a.masked, None);
        assert!(v["text"].as_str().unwrap().contains("ann@example.com"));
        assert!(v["running"][0]["query"].as_str().unwrap().len() <= MAX_QUERY_BYTES);
        assert!(v.get("runningTotal").is_none(), "два экземпляра не режутся");
    }

    /// Выполняющиеся экземпляры не ограничены запросом карточки: модели — `MAX_RUNNING` самых долгих (по
    /// длительности, по убыванию) и полное число в `runningTotal`; литералы считаются только у отданных.
    #[test]
    fn query_card_keeps_the_twenty_longest_running_instances() {
        let mut d = QueryDetail::unavailable_pgss(170_000, false);
        // 25 экземпляров вперемешку (7 и 25 взаимно просты), у нулевого длительности нет.
        d.running = (0..25)
            .map(|i| (i * 7) % 25)
            .map(|k| {
                let mut r = running(format!("SELECT {k}"));
                r.pid = k;
                r.duration_s = (k > 0).then_some(f64::from(k));
                r
            })
            .collect();
        let a = query_detail_answer(d, "pg-1", LlmAccess::Diagnostics).unwrap();
        let PgOutput::QueryDetails(v) = &a.body else {
            panic!("не карточка")
        };
        let pids: Vec<i64> = v["running"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["pid"].as_i64().unwrap())
            .collect();
        assert_eq!(pids, (5..=24).rev().collect::<Vec<i64>>());
        assert_eq!(v["runningTotal"], 25);
        assert_eq!(
            a.masked,
            Some(20),
            "литералы — только у отданных экземпляров"
        );
    }

    fn partitioned(n: usize) -> TableDetails {
        let mut t = TableDetails {
            oid: 1,
            schema: "shop".into(),
            name: "events".into(),
            relkind: "p".into(),
            unlogged: false,
            owner: "postgres".into(),
            tablespace: None,
            comment: None,
            reloptions: vec![],
            columns: vec![],
            indexes: vec![],
            constraints: vec![],
            incoming_fks: vec![],
            stats: Default::default(),
            partitioning: Partitioning {
                key: Some("RANGE (at)".into()),
                // Первое имя — в кавычках в DDL (заглавные), как у `ddl::table_ddl`.
                partitions: (0..n)
                    .map(|i| RelationRef {
                        schema: "shop".into(),
                        name: if i == 0 {
                            "Events-0".into()
                        } else {
                            format!("events_{i:04}")
                        },
                    })
                    .collect(),
                ..Default::default()
            },
            triggers: vec![],
            ddl: String::new(),
        };
        t.ddl = crate::catalog::ddl::table_ddl(&t);
        t
    }

    /// Список партиций не ограничен каталогом: модели — первые `MAX_PARTITIONS` и полное число в
    /// `partitioning.partitionsTotal`; строка `-- partitions:` в DDL режется так же.
    #[test]
    fn table_partitions_are_cut_in_the_list_and_the_ddl() {
        let out = table_out(partitioned(250), vec![]).unwrap();
        let p = &out.table["partitioning"];
        assert_eq!(p["partitions"].as_array().unwrap().len(), MAX_PARTITIONS);
        assert_eq!(p["partitions"][199]["name"], "events_0199");
        assert_eq!(p["partitionsTotal"], 250);
        let ddl = out.table["ddl"].as_str().unwrap();
        assert!(
            ddl.contains("\n-- partitions (200 of 250): shop.\"Events-0\", shop.events_0001, "),
            "{ddl}"
        );
        assert!(
            ddl.contains(", shop.events_0199\n") && !ddl.contains("events_0200"),
            "{ddl}"
        );
        // До предела — как у интерфейса.
        let t = partitioned(MAX_PARTITIONS);
        let ddl = t.ddl.clone();
        let out = table_out(t, vec![]).unwrap();
        let p = &out.table["partitioning"];
        assert_eq!(p["partitions"].as_array().unwrap().len(), MAX_PARTITIONS);
        assert!(p.get("partitionsTotal").is_none());
        assert_eq!(out.table["ddl"], ddl.as_str());
    }

    // Задача 12: EXPLAIN и гипотезы.

    #[test]
    fn plan_parameters_follow_the_server_version() {
        let v = |x: &[&str]| x.iter().map(|s| Some(s.to_string())).collect::<Vec<_>>();
        assert_eq!(
            plan_params("SELECT 1", None, false, 130_000).ok(),
            Some(Params::None)
        );
        assert_eq!(
            plan_params("SELECT $1", Some(v(&["7"]).as_slice()), false, 130_000).ok(),
            Some(Params::Values { values: v(&["7"]) })
        );
        assert_eq!(
            plan_params("SELECT $1", None, false, 170_000).ok(),
            Some(Params::Generic)
        );
        assert_eq!(
            plan_params("SELECT 1", None, true, 160_000).ok(),
            Some(Params::Generic)
        );
        let e = plan_params("SELECT $1", None, false, 150_000)
            .err()
            .unwrap();
        assert!(
            e.code == "invalid_argument" && e.message.contains("16"),
            "{}",
            e.message
        );
        let e = plan_params("SELECT $1, $2", Some(v(&["7"]).as_slice()), false, 170_000)
            .err()
            .unwrap();
        assert_eq!(e.code, "invalid_argument");
        let e = plan_params("SELECT $1", Some(v(&["7"]).as_slice()), true, 170_000)
            .err()
            .unwrap();
        assert_eq!(e.code, "invalid_argument");
    }

    // Сверх плана: маскирование плана по тексту pgss, проверки EXPLAIN и гипотез до соединения.

    /// План `Append` по 10 партициям с фильтром-выражением по дате (правило `plan_no_pruning` без ANALYZE даёт
    /// рекомендацию с условием и его переписью) — литерал `2026-01-05` в каждом `Filter`.
    fn partitions_plan() -> Value {
        let child = |i: usize| {
            json!({
                "Node Type": "Seq Scan", "Parent Relationship": "Member", "Relation Name": format!("events_{i}"),
                "Schema": "shop", "Alias": format!("events_{i}"), "Startup Cost": 0.0, "Total Cost": 10.0,
                "Plan Rows": 1, "Plan Width": 8, "Output": ["id"],
                "Filter": "((created_at)::date = '2026-01-05'::date)"
            })
        };
        json!([{
            "Plan": {
                "Node Type": "Append", "Startup Cost": 0.0, "Total Cost": 100.0, "Plan Rows": 10, "Plan Width": 8,
                "Plans": (0..10).map(child).collect::<Vec<_>>()
            }
        }])
    }

    /// План по тексту pgss на уровне `diagnostics` (§6.1): ни текст плана, ни рекомендации (условие, перепись,
    /// заголовок и доказательство) не несут литерал; план по SQL модели — как есть.
    #[test]
    fn plan_of_a_pgss_text_is_masked_and_plan_of_model_sql_is_not() {
        let a = explain_answer("pg-1".into(), partitions_plan(), &Params::None, true).unwrap();
        let PgOutput::Explain(out) = &a.body else {
            panic!("не план")
        };
        assert!(
            !out.recommendations.is_empty(),
            "фикстура даёт рекомендацию"
        );
        let v = serde_json::to_string(out).unwrap();
        assert!(!v.contains("2026-01-05"), "{v}");
        assert!(out.plan_text.contains("$1"), "{}", out.plan_text);
        assert!(a.masked.is_some_and(|n| n >= 10), "{:?}", a.masked);
        assert_eq!(out.tables.len(), 10);
        assert!(out.tables.contains(&"shop.events_0".to_string()));
        assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
        assert!(
            out.warnings[0].contains("plan_seq_scan_filter"),
            "правила с фактом не проверялись: {:?}",
            out.warnings
        );

        let a = explain_answer("pg-1".into(), partitions_plan(), &Params::Generic, false).unwrap();
        let PgOutput::Explain(out) = &a.body else {
            panic!("не план")
        };
        assert_eq!(a.masked, None);
        assert!(out.plan_text.contains("2026-01-05"), "{}", out.plan_text);
        assert_eq!(out.warnings[0], GENERIC_WARNING);
        assert_eq!(out.member, "pg-1");
    }

    #[test]
    fn hypothesis_plan_of_a_pgss_text_is_masked() {
        let raw = partitions_plan();
        let result = || HypoManyResult {
            cost_before: 100.0,
            cost_after: 50.0,
            indexes: vec![hypopg::HypoIndexResult {
                ddl: "CREATE INDEX ON shop.events_0 (created_at) WHERE kind = 'vip'".into(),
                used: true,
                estimated_size_bytes: 8192,
            }],
            plan: from_json(&raw).unwrap(),
            raw: raw.clone(),
        };
        let a = hypo_answer(result(), true).unwrap();
        let PgOutput::Hypo(out) = &a.body else {
            panic!("не гипотеза")
        };
        assert!(!out.plan_text.contains("2026-01-05"), "{}", out.plan_text);
        assert_eq!(a.masked, Some(10));
        assert!(
            out.indexes[0].ddl.contains("'vip'"),
            "DDL гипотезы — от модели, не маскируется"
        );
        assert_eq!(
            (
                out.cost_before,
                out.cost_after,
                out.indexes[0].estimated_size_bytes
            ),
            (100.0, 50.0, Some(8192))
        );
        let a = hypo_answer(result(), false).unwrap();
        let PgOutput::Hypo(out) = &a.body else {
            panic!("не гипотеза")
        };
        assert_eq!(a.masked, None);
        assert!(out.plan_text.contains("2026-01-05"));
    }

    #[tokio::test]
    async fn explain_and_hypothesis_check_access_and_sql_before_opening() {
        let (_dir, st) = state_with(vec![member("pg-1", Role::Primary)]);
        let explain = |v: Value| PgRead::Explain(serde_json::from_value(v).unwrap());
        let hypo = |v: Value| PgRead::HypoIndex(serde_json::from_value(v).unwrap());
        let e = err(read_op(
            &st,
            explain(json!({"connectionId": "off", "database": "d", "sql": "SELECT pg_sleep(1)"})),
        )
        .await);
        assert_eq!(e.code, "not_allowed", "доступ — раньше SQL");
        for (op, code) in [
            (
                explain(
                    json!({"connectionId": "on", "database": "d", "sql": "SELECT pg_sleep(1)"}),
                ),
                "forbidden_function",
            ),
            (
                explain(
                    json!({"connectionId": "on", "database": "d", "sql": "SELECT 1; SELECT 2"}),
                ),
                "invalid_sql",
            ),
            (
                explain(json!({"connectionId": "on", "database": "d", "queryId": "abc"})),
                "invalid_argument",
            ),
            (
                hypo(
                    json!({"connectionId": "on", "database": "d", "sql": "SELECT 1",
                    "indexes": ["CREATE INDEX ON t (a); DROP TABLE t"]}),
                ),
                "invalid_sql",
            ),
            (
                hypo(
                    json!({"connectionId": "on", "database": "d", "sql": "SELECT dblink('x', 'y')",
                    "indexes": ["CREATE INDEX ON t (a)"]}),
                ),
                "forbidden_function",
            ),
        ] {
            let e = err(read_op(&st, op).await);
            assert_eq!(e.code, code, "{}", e.message);
        }
        assert_eq!(
            st.runtimes.view("on").state,
            ConnState::Disconnected,
            "отвергнутый SQL не открывает подключение"
        );
    }

    #[tokio::test]
    async fn new_tools_check_access_and_arguments_before_opening() {
        let (_dir, st) = state_with(vec![member("pg-1", Role::Primary)]);
        let op = |kind: &str, v: Value| -> PgRead {
            match kind {
                "top" => PgRead::TopQueries(serde_json::from_value(v).unwrap()),
                "card" => PgRead::QueryDetails(serde_json::from_value(v).unwrap()),
                "table" => PgRead::TableDetails(serde_json::from_value(v).unwrap()),
                _ => PgRead::Settings(serde_json::from_value(v).unwrap()),
            }
        };
        for (kind, v) in [
            ("top", json!({"connectionId": "off", "sort": "fastest"})),
            (
                "card",
                json!({"connectionId": "off", "database": "d", "queryId": "x"}),
            ),
            (
                "table",
                json!({"connectionId": "off", "database": "d", "schema": "s", "table": "t"}),
            ),
            ("settings", json!({"connectionId": "nope"})),
        ] {
            let e = err(read_op(&st, op(kind, v)).await);
            assert_eq!(e.code, "not_allowed", "{kind}: доступ — раньше аргументов");
        }
        for v in [
            json!({"connectionId": "on", "sort": "fastest"}),
            json!({"connectionId": "on", "mode": "between"}),
            json!({"connectionId": "on", "mode": "window", "windowS": 60}),
        ] {
            let e = err(read_op(&st, op("top", v.clone())).await);
            assert_eq!(e.code, "invalid_argument", "{v}");
        }
        let e = err(read_op(
            &st,
            op(
                "card",
                json!({"connectionId": "on", "database": "d", "queryId": "x"}),
            ),
        )
        .await);
        assert_eq!(e.code, "invalid_argument");
        assert_eq!(
            st.runtimes.view("on").state,
            ConnState::Disconnected,
            "неверный вызов не открывает подключение"
        );
        assert_eq!(st.runtimes.view("off").state, ConnState::Disconnected);
    }

    // Смоук P4b, G1–G4: ответы короче — интервалы ожиданий без сэмплов, нулевые счётчики и длинные тексты строк pgss,
    // ряд истории карточки.

    fn bucket(start_ms: i64, sampled_s: f64, cpu: Option<f64>) -> pg_advisor::ash::Bucket {
        pg_advisor::ash::Bucket {
            start_ms,
            sampled_s,
            aas: cpu
                .map(|v| (pg_advisor::ash::WaitClass::Cpu, v))
                .into_iter()
                .collect(),
        }
    }

    fn waits_out(data: WaitsData) -> Report {
        report_out(report(Tool::Waits, DiagData::Waits(data), vec![]))
    }

    fn bucket_starts(r: &Report) -> Vec<i64> {
        r.data["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["startMs"].as_i64().unwrap())
            .collect()
    }

    /// G1: сэмплер в окне не работал — 90 пустых интервалов окна 900 с не отдаются, вместо них подсказка: где Glafira
    /// собирает сэмплы и что вызвать.
    #[test]
    fn waits_without_samples_have_no_buckets_and_a_hint() {
        let empty = pg_advisor::ash::profile(&[], &[], &[], 900, 1_000_000_000, None, false);
        assert_eq!(empty.buckets.len(), 90);
        let r = waits_out(empty);
        assert!(bucket_starts(&r).is_empty(), "{}", r.data);
        assert_eq!(
            (&r.data["totalSamples"], &r.data["samplerRunning"]),
            (&json!(0), &json!(false))
        );
        assert_eq!(r.hint.as_deref(), Some(NO_WAIT_SAMPLES_HINT));
        assert!(NO_WAIT_SAMPLES_HINT.contains("call observe_activity"));
    }

    /// Сэмплер работал, но активных сессий не видел: интервалов тоже нет, а подсказка говорит, сколько секунд он
    /// работал, — вызывать `observe_activity` незачем.
    #[test]
    fn waits_of_an_idle_node_have_no_buckets_and_say_so() {
        let data = WaitsData {
            window_s: 300,
            bucket_s: 5,
            sampler_running: true,
            buckets: vec![
                bucket(0, 5.0, None),
                bucket(5_000, 3.0, None),
                bucket(10_000, 0.0, None),
            ],
            ..WaitsData::default()
        };
        let r = waits_out(data);
        assert!(bucket_starts(&r).is_empty(), "{}", r.data);
        assert_eq!(
            r.hint.as_deref(),
            Some("The wait sampler ran for 8 s of this window and saw no active sessions")
        );
    }

    /// Сэмплы есть: уходят только интервалы без сэмплов (`sampledS` 0 и пустой `aas`); интервал, где сэмплер работал,
    /// а активных сессий не было, остаётся (нагрузка 0); подсказки нет.
    #[test]
    fn waits_drop_only_buckets_without_samples() {
        let data = WaitsData {
            window_s: 300,
            bucket_s: 10,
            total_samples: 4,
            buckets: vec![
                bucket(0, 0.0, None),
                bucket(10_000, 10.0, Some(0.4)),
                bucket(20_000, 10.0, None),
                bucket(30_000, 0.0, None),
            ],
            ..WaitsData::default()
        };
        let r = waits_out(data);
        assert_eq!(bucket_starts(&r), [10_000, 20_000]);
        assert_eq!(r.data["buckets"][0]["aas"]["cpu"], 0.4);
        assert!(r.hint.is_none(), "{:?}", r.hint);
    }

    /// Другие инструменты `run_diagnostic` интервалов не несут: отчёт отдаётся как был, без подсказки.
    #[test]
    fn other_reports_are_not_touched() {
        let r = report_out(report(
            Tool::Sessions,
            DiagData::Sessions(SessionsData::default()),
            vec![],
        ));
        assert!(r.hint.is_none());
        assert_eq!(
            r.data,
            serde_json::to_value(DiagData::Sessions(SessionsData::default())).unwrap()
        );
    }

    /// Список счётчиков, которые не отдаются нулевыми, — ровно поля `Counters` без `calls`, `totalExecMs` и `rows`: новое
    /// поле счётчиков попадёт и сюда.
    #[test]
    fn zero_omitted_counters_are_the_counters_but_the_kept_ones() {
        let counters = serde_json::to_value(pg_advisor::queries::Counters::default()).unwrap();
        let mut names: Vec<&str> = counters
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .filter(|k| !["calls", "totalExecMs", "rows"].contains(k))
            .collect();
        names.sort_unstable();
        let mut omitted = ZERO_OMITTED_COUNTERS.to_vec();
        omitted.sort_unstable();
        assert_eq!(names, omitted);
    }

    fn row_keys(row: &Value) -> Vec<&str> {
        row.as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect()
    }

    /// G2: строка топа для модели — без счётчиков, равных 0, и без `own: false`; `calls`, `totalExecMs`, `meanExecMs`,
    /// `rows`, ключ, база, роль, текст, `textTruncated` и доли — всегда; текст длиннее 500 символов обрезается с
    /// `textTruncated: true`. Так же — у `run_diagnostic` с инструментом `queries` (тот же `report_out`).
    #[test]
    fn top_rows_omit_zero_counters_and_cut_long_texts() {
        let mut r = statements_report();
        let DiagData::Queries(d) = &mut r.data else {
            unreachable!()
        };
        d.rows[0].shared_hit = 7;
        d.rows[0].io_read_ms = 1.5;
        d.rows[1].text = Some(format!("SELECT {}", "ж".repeat(600)));
        d.rows[2].own = true;
        let a = top_answer(r.clone(), LlmAccess::Off, None, 10, false, false);
        let PgOutput::Report(out) = &a.body else {
            unreachable!()
        };
        let rows = out.data["rows"].as_array().unwrap();
        let keys = row_keys(&rows[0]);
        for key in [
            "key",
            "datname",
            "rolname",
            "calls",
            "totalExecMs",
            "meanExecMs",
            "rows",
            "text",
            "textTruncated",
            "sharePct",
            "sharedHit",
            "ioReadMs",
        ] {
            assert!(keys.contains(&key), "{key}: {keys:?}");
        }
        for key in [
            "localHit",
            "localRead",
            "localDirtied",
            "localWritten",
            "walBytes",
            "walFpi",
            "walRecords",
            "ioWriteMs",
            "plans",
            "totalPlanMs",
            "sharedRead",
            "tempWritten",
            "own",
        ] {
            assert!(!keys.contains(&key), "{key}: {keys:?}");
        }
        assert_eq!(
            (&rows[0]["totalExecMs"], &rows[0]["rows"], &rows[0]["calls"]),
            (&json!(0.0), &json!(0), &json!(1))
        );
        let text = rows[1]["text"].as_str().unwrap();
        assert_eq!(text.chars().count(), TOP_TEXT_CHARS);
        assert!(text.starts_with("SELECT жж"), "{text}");
        assert_eq!(
            (&rows[0]["textTruncated"], &rows[1]["textTruncated"]),
            (&json!(false), &json!(true))
        );
        assert_eq!(rows[2]["own"], true, "своя строка Glafira помечена");
        // `run_diagnostic` (`queries`) — тот же вид строк.
        let v = report_out(r);
        assert_eq!(v.data["rows"], out.data["rows"]);
    }

    fn point(minute: i64, calls: i64, total_exec_ms: f64) -> HistoryPoint {
        HistoryPoint {
            at_ms: minute * 60_000,
            calls,
            total_exec_ms,
            mean_exec_ms: if calls > 0 {
                total_exec_ms / calls as f64
            } else {
                0.0
            },
        }
    }

    /// G4: 60 минутных точек — 12 точек по 5 минут: вызовы и время суммируются, среднее — по сумме, `atMs` — начало.
    #[test]
    fn history_of_sixty_points_is_merged_into_twelve() {
        let h = history_for_model((0..60).map(|m| point(m, m, m as f64 * 2.0)).collect());
        assert_eq!(h.len(), MAX_HISTORY_POINTS);
        for (g, p) in (0i64..).zip(&h) {
            let calls = 25 * g + 10;
            assert_eq!(
                (p.at_ms, p.calls, p.total_exec_ms, p.mean_exec_ms),
                (5 * g * 60_000, calls, 2.0 * calls as f64, 2.0),
                "группа {g}"
            );
        }
    }

    /// Ряд не длиннее 12 точек — как есть; 13 точек — 12, порядок сохраняется, вызовы не теряются.
    #[test]
    fn short_history_stays_as_it_is() {
        let seven: Vec<HistoryPoint> = (0..7).map(|m| point(m, 3, 1.5)).collect();
        assert_eq!(history_for_model(seven.clone()), seven);
        assert!(history_for_model(vec![]).is_empty());
        let h = history_for_model((0..13).map(|m| point(m, 1, 1.0)).collect());
        assert_eq!(h.len(), MAX_HISTORY_POINTS);
        assert!(h.windows(2).all(|w| w[0].at_ms < w[1].at_ms));
        assert_eq!(h.iter().map(|p| p.calls).sum::<i64>(), 13);
        assert_eq!(h[0].at_ms, 0);
    }

    /// Слитая точка без вызовов — среднее 0, а не NaN.
    #[test]
    fn merged_points_without_calls_have_a_zero_mean() {
        let mut points: Vec<HistoryPoint> = (0..24).map(|m| point(m, 0, 0.0)).collect();
        points[2] = point(2, 4, 10.0);
        let h = history_for_model(points);
        assert_eq!(h.len(), MAX_HISTORY_POINTS);
        assert_eq!((h[0].calls, h[0].mean_exec_ms), (0, 0.0));
        assert_eq!(
            (
                h[1].at_ms,
                h[1].calls,
                h[1].total_exec_ms,
                h[1].mean_exec_ms
            ),
            (2 * 60_000, 4, 10.0, 2.5)
        );
        assert!(h.iter().all(|p| p.mean_exec_ms.is_finite()));
    }

    /// G2, G4: в карточке строки pgss (с момента сброса, за окна, по ролям) — без счётчиков, равных 0, история — не
    /// больше 12 точек.
    #[test]
    fn query_card_rows_omit_zero_counters_and_history_is_short() {
        use crate::diag::query_detail::{ByUser, WindowStat};
        let mut row = statement(5, "SELECT 1", 60);
        row.text = None;
        row.shared_hit = 3;
        let mut d = QueryDetail::unavailable_pgss(170_000, false);
        d.since_reset = Some(row.clone());
        d.windows = vec![
            WindowStat {
                window_s: 300,
                stat: Some(row.clone()),
            },
            WindowStat {
                window_s: 900,
                stat: None,
            },
        ];
        d.by_user = vec![ByUser {
            rolname: Some("app".into()),
            toplevel: true,
            stat: row,
        }];
        d.history = (0..60).map(|m| point(m, 1, 2.0)).collect();
        let a = query_detail_answer(d, "pg-1", LlmAccess::Off).unwrap();
        let PgOutput::QueryDetails(v) = &a.body else {
            panic!("не карточка")
        };
        for row in [
            &v["sinceReset"],
            &v["windows"][0]["stat"],
            &v["byUser"][0]["stat"],
        ] {
            let keys = row_keys(row);
            assert!(
                !keys.contains(&"localHit") && !keys.contains(&"walBytes"),
                "{keys:?}"
            );
            assert_eq!((&row["sharedHit"], &row["calls"]), (&json!(3), &json!(60)));
            assert!(
                keys.contains(&"rows") && keys.contains(&"totalExecMs"),
                "{keys:?}"
            );
        }
        assert_eq!(v["windows"][1]["stat"], Value::Null);
        assert_eq!(v["byUser"][0]["rolname"], "app");
        let history = v["history"].as_array().unwrap();
        assert_eq!(history.len(), MAX_HISTORY_POINTS);
        assert_eq!(history[1]["calls"], 5);
    }
}
