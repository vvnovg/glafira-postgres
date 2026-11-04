//! Операции чтения MCP в приложении (спец. P4 §3.2, §5.2): `AppBackend` (`backend.rs`) отдаёт сюда каждый вызов
//! `PgBackend`. На вызов: доступ к подключению (§4.3 шаг 3), открытие закрытого подключения тем же путём, что щелчок в
//! дереве, существующая функция приложения (`diag::run`, карточка, каталог, EXPLAIN, hypopg) на общих пулах, маскирование
//! на уровне `diagnostics` (§6.1) и английские тексты находок. Диапазоны параметров, усечение ответа и журнал — в крейте
//! шлюза.

use std::time::Duration;

use futures_util::future::join_all;
use glafira_mcp_core::error::ToolError;
use glafira_pg_mcp::model::{
    ActivityOut, ConnectionInfo, HealthSummary, MemberInfo, ObserveCtx, PatroniInfo, PgAnswer,
    PgObserve, PgOutput, PgRead, Report, TopologyInfo, UnavailableOut,
};
use glafira_pg_mcp::params::{
    ACTIVITY_LIMIT_DEFAULT, ActivityParams, RunDiagnosticParams, TOPOLOGY_EVENTS_MAX,
};
use pg_advisor::findings::FindingTarget;
use pg_advisor::queries::QueryStat;
use pg_advisor::rules::sessions::SessionRow;
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::access::{
    check_connection, effective_access, ensure_open, err, invalid_argument, masks, model_app_error,
    open_for_llm, tool_error,
};
use super::mask::{MAX_QUERY_BYTES, cut_text, has_texts, mask_opt, mask_report};
use super::texts::{diag_catalog, finding_out, tool_id};
use crate::conn::profile::{ConnectionKind, LlmAccess};
use crate::diag::queries::{LIMITS, QueriesMode, QueriesParams};
use crate::diag::{self, DiagData, DiagReport, DiagRequest, Tool, Unavailable};
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

/// Операция чтения. Варианты задач 11–12 до них отвечают `internal: not implemented yet`.
pub async fn read_op(st: &AppState, op: PgRead) -> Result<PgAnswer, ToolError> {
    match op {
        PgRead::ListConnections => Ok(answer(PgOutput::Connections(list_connections(st)))),
        PgRead::ListDiagnostics => Ok(answer(PgOutput::Diagnostics(diag_catalog()))),
        PgRead::Topology(p) => topology(st, &p.connection_id).await,
        PgRead::HealthSummary(p) => health_summary(st, &p.connection_id).await,
        PgRead::RunDiagnostic(p) => run_diagnostic(st, p).await,
        PgRead::Activity(p) => activity(st, p).await,
        PgRead::TopQueries(_)
        | PgRead::QueryDetails(_)
        | PgRead::TableDetails(_)
        | PgRead::Settings(_)
        | PgRead::Explain(_)
        | PgRead::HypoIndex(_) => Err(not_yet()),
    }
}

/// Наблюдение (`observe_activity`, `observe_top_queries`) — задача 12.
pub async fn observe_op(
    _st: &AppState,
    _op: PgObserve,
    _ctx: ObserveCtx,
) -> Result<PgAnswer, ToolError> {
    Err(not_yet())
}

/// Операция ещё не подключена (уходит в задаче 12 плана P4a).
fn not_yet() -> ToolError {
    err("internal", "not implemented yet")
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
                llm_access: access.as_str().to_string(),
                connected,
                role_warning: None,
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
/// ошибки узлов — без подробностей, `hide_error_details`), находки — с английскими текстами, причины недоступности —
/// английским текстом.
pub fn report_out(mut r: DiagReport) -> Report {
    hide_error_details(&mut r.data);
    Report {
        tool: tool_id(r.tool),
        data: serde_json::to_value(&r.data).unwrap_or(Value::Null),
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
        hint: None,
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
        return Err(tool_error(crate::error::AppError::internal_with(
            "sessions report without sessions data",
        )));
    };
    let (total, rows) = filter_sessions(std::mem::take(&mut d.rows), p);
    d.rows = rows;
    let masked = mask_if(access, &mut report);
    let DiagData::Sessions(mut d) = report.data else {
        return Err(tool_error(crate::error::AppError::internal_with(
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glafira_pg_mcp::params::ActivityParams;
    use pg_advisor::findings::{Finding, FindingTarget, Severity};
    use pg_advisor::rules::sessions::{SessionRow, SessionsData};
    use serde_json::json;

    use super::*;
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
}
