//! Инструменты предложений на стороне приложения (спец. P4 §8.2): `get_optimization_request`, `propose_query_rewrite`,
//! `list_proposals`. Предложенный SQL и индексы не выполняются: до сервера — общие проверки SQL модели
//! (`model_sql`: `sqlcheck` с позицией в тексте модели, строки индексов как у `hypothetical_index`), на сервере —
//! `prepare` и проба плана в слоях §7.3 (`GuardTx`); ошибки сервера уходят модели его текстом (`invalid_sql`).
//! Снимок запроса на оптимизацию модель получает без маскирования (его отправил пользователь, §8.1), уложенным в
//! предел ответа (решение владельца Q3, `request_for_model`).

use glafira_mcp_core::error::ToolError;
use glafira_mcp_core::limits::HARD_LIMIT_BYTES;
use glafira_pg_mcp::model::{LlmOp, PgAnswer, PgOutput, ProposedOut};
use glafira_pg_mcp::params::{
    GetRequestParams, HYPO_INDEXES_MAX, ListProposalsParams, ProposeParams, WITH_REQUEST_ONLY,
};
use pg_advisor::sqlcheck::{Purpose, StatementKind};
use serde_json::Value;

use super::compare::params_for;
use super::proposals::{LlmProposal, NewProposal, ProposalFilter, ProposalSource};
use super::requests::{OptimizationRequest, RequestSource, RequestStatus};
use super::{ProposalsEvent, off_thread, plain_text};
use crate::diag::caps::V16;
use crate::error::AppError;
use crate::explain::Params;
use crate::explain::params::{count_params, normalize};
use crate::mcp::access::{check_connection, ensure_open, err, invalid_argument, tool_error};
use crate::mcp::guard;
use crate::mcp::model_sql::{
    Asked, PlanText, asked, check_index_ddls, check_model_sql, resolve_database, sql_source,
};
use crate::mcp::read::{EXPLAIN_TIMEOUT_S, member_or_leader};
use crate::state::AppState;

/// Бюджет `params` снимка для модели (остальное — запрос, DDL таблиц, план).
const PARAMS_BUDGET_BYTES: usize = 16 * 1024;
/// Проходов подгонки снимка вместе с подсказкой.
const FIT_PASSES: usize = 8;
/// Сколько имён опущенных таблиц называет подсказка (остальные — «and N more»).
const HINT_NAMES: usize = 3;
/// Пометка обрезанного текста плана — как у значений `run_select`.
const TRUNCATED_MARK: &str = "…[truncated]";

pub async fn handle(st: &AppState, op: LlmOp) -> Result<PgAnswer, ToolError> {
    // `validate` — и при вызове не через шлюз (тесты, будущие вызовы приложения): пустые строки — «не заданы» (E2).
    let body = match op {
        LlmOp::GetRequest { params, .. } => get_request(st, params.validate()?)?,
        LlmOp::Propose { params, client, .. } => propose(st, params.validate()?, client).await?,
        LlmOp::ListProposals { params, .. } => list(st, params.validate()?)?,
    };
    Ok(PgAnswer::new(body))
}

fn request_closed(id: &str) -> ToolError {
    err(
        "request_closed",
        format!("Optimization request {id} was cancelled by the user"),
    )
}

fn request_not_found(id: &str) -> ToolError {
    ToolError::not_found(format!(
        "Optimization request {id} not found: Glafira keeps the last 50; ask the user to send the query again with “Send to LLM assistant”"
    ))
}

/// Запрос по id для модели: есть, подключение открыто для модели (иначе `not_allowed`, §4.3), не отменён.
fn open_request(st: &AppState, id: &str) -> Result<OptimizationRequest, ToolError> {
    let r = st
        .llm
        .requests
        .get(id)
        .ok_or_else(|| request_not_found(id))?;
    check_connection(st, &r.connection_id)?;
    if r.status == RequestStatus::Cancelled {
        return Err(request_closed(id));
    }
    Ok(r)
}

/// Ошибка хранилища для модели: предел предложений на запрос — `invalid_argument`, гонка с отменой — `request_closed`,
/// прочее — как у остальных инструментов.
fn store_error(e: AppError) -> ToolError {
    match e.key.as_str() {
        "errors.llm.tooManyProposals" => invalid_argument(format!(
            "{}: ask the user to review them in Glafira",
            e.message
        )),
        "errors.llm.requestClosed" => err(
            "request_closed",
            "The optimization request was cancelled by the user",
        ),
        _ => tool_error(e),
    }
}

fn json_size(v: &Value) -> usize {
    serde_json::to_vec(v).map_or(usize::MAX, |b| b.len())
}

/// Снимок для модели (§8.1): `OptimizationRequest` как есть (camelCase, без маскирования), имя подключения и `next` —
/// следующий шаг сценария; уложен в предел ответа (`fit_snapshot`).
pub fn request_for_model(
    r: &OptimizationRequest,
    connection_name: &str,
) -> Result<Value, ToolError> {
    let mut v = serde_json::to_value(r)
        .map_err(|e| ToolError::internal(format!("optimization request JSON: {e}")))?;
    let m = v
        .as_object_mut()
        .ok_or_else(|| ToolError::internal("optimization request JSON is not an object"))?;
    m.insert("connectionName".into(), connection_name.into());
    m.insert(
        "next".into(),
        format!(
            "Check ideas with explain_query and hypothetical_index (connectionId {}, database {}, member {}), then call propose_query_rewrite with requestId {}.",
            r.connection_id, r.database, r.member, r.id
        )
        .into(),
    );
    // `params` — в бюджет до подгонки остального; подсказка (`hint`) тоже в бюджете: подгонка повторяется с меньшим
    // пределом, пока итог с подсказкой не уложится в предел ответа ядра (сходится за 2–3 прохода).
    let notes = fit_params(&mut v);
    let base = v;
    let mut limit = HARD_LIMIT_BYTES;
    for _ in 0..FIT_PASSES {
        let mut v = base.clone();
        let mut hints = notes.clone();
        hints.extend(fit_snapshot(&mut v, limit));
        if !hints.is_empty() {
            v["hint"] = format!("Omitted to fit the 100 KB response: {}.", hints.join("; ")).into();
        }
        let size = json_size(&v);
        if size <= HARD_LIMIT_BYTES {
            return Ok(v);
        }
        limit = limit.saturating_sub(size - HARD_LIMIT_BYTES);
    }
    Err(ToolError::internal(
        "optimization request does not fit the response limit",
    ))
}

/// Значения `params` в бюджет `PARAMS_BUDGET_BYTES`: каждое значение режется до всё меньшей длины (с пометкой
/// `…[truncated]`), пока список не уложится; если и пометок слишком много — значения отбрасываются (пустой список).
/// Итог — что сделано, для `hint`.
fn fit_params(v: &mut Value) -> Vec<String> {
    let Some(list) = v.get("params").and_then(Value::as_array) else {
        return Vec::new();
    };
    if json_size(&Value::Array(list.clone())) <= PARAMS_BUDGET_BYTES {
        return Vec::new();
    }
    for cap in [2_048, 512, 128, 32, 0] {
        let cut: Vec<Value> = list
            .iter()
            .map(|x| match x.as_str() {
                Some(t) if t.len() > cap => {
                    let mut keep = cap;
                    while !t.is_char_boundary(keep) {
                        keep -= 1;
                    }
                    format!("{}{TRUNCATED_MARK}", &t[..keep]).into()
                }
                _ => x.clone(),
            })
            .collect();
        if json_size(&Value::Array(cut.clone())) <= PARAMS_BUDGET_BYTES {
            v["params"] = Value::Array(cut);
            return vec![format!(
                "values in params cut to {cap} bytes each (the full values are in the user's console)"
            )];
        }
    }
    v["params"] = Value::Array(Vec::new());
    vec!["params (too many values; they are in the user's console)".to_string()]
}

/// Решение владельца Q3: `bundle.text`, равный `sql`, не повторяется (всегда); пока снимок больше `limit` — DDL таблиц
/// уходят с конца `bundle.tables` (`bundle.tablesOmitted` растёт), затем режется конец `bundle.planText` с пометкой
/// `…[truncated]`. Итог — что опущено, для `hint` (по-английски). `sql` не режется: модель должна видеть настоящий
/// запрос (он ограничен 64 КБ при создании снимка).
fn fit_snapshot(v: &mut Value, limit: usize) -> Vec<String> {
    let sql = v.get("sql").and_then(Value::as_str).map(normalize);
    if let Some(bundle) = v.get_mut("bundle").and_then(Value::as_object_mut)
        && bundle
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|t| Some(normalize(t)) == sql)
    {
        bundle.remove("text");
    }
    let mut omitted = Vec::new();
    // Имена опущенных таблиц (их не больше `explain::llm::MAX_TABLES`) — в подсказку: модель прочитает их DDL сама.
    let mut dropped = Vec::new();
    while json_size(v) > limit {
        let Some(bundle) = v.get_mut("bundle").and_then(Value::as_object_mut) else {
            break;
        };
        let Some(list) = bundle.get_mut("tables").and_then(Value::as_array_mut) else {
            break;
        };
        let Some(table) = list.pop() else {
            break;
        };
        dropped.push(match (table["schema"].as_str(), table["name"].as_str()) {
            (Some(s), Some(n)) => format!("{s}.{n}"),
            (None, Some(n)) => n.to_string(),
            _ => "?".to_string(),
        });
        let before = bundle
            .get("tablesOmitted")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        bundle.insert("tablesOmitted".into(), (before + 1).into());
    }
    if !dropped.is_empty() {
        dropped.reverse();
        let mut names = dropped[..dropped.len().min(HINT_NAMES)].join(", ");
        if dropped.len() > HINT_NAMES {
            names.push_str(&format!(" and {} more", dropped.len() - HINT_NAMES));
        }
        omitted.push(format!(
            "DDL of {} tables at the end of bundle.tables, counted in bundle.tablesOmitted ({names}; read them with get_table_details)",
            dropped.len(),
        ));
    }
    let plan = v
        .pointer("/bundle/planText")
        .and_then(Value::as_str)
        .map(String::from);
    if let Some(plan) = plan
        && json_size(v) > limit
    {
        // В JSON байт текста не меньше, чем в самом тексте (экранирование только добавляет): срез на превышение плюс
        // пометка укладывает снимок за один-два шага.
        let mut keep = plan.len();
        loop {
            let over = json_size(v).saturating_sub(limit);
            if over == 0 || keep == 0 {
                break;
            }
            keep = keep.saturating_sub(over + TRUNCATED_MARK.len());
            while !plan.is_char_boundary(keep) {
                keep -= 1;
            }
            v["bundle"]["planText"] = format!("{}{TRUNCATED_MARK}", &plan[..keep]).into();
        }
        omitted.push(
            "the end of bundle.planText (build the full plan with explain_query on the same connection, database and member)"
                .to_string(),
        );
    }
    // Крайние меры (очень большие `sql` и `bundle.text`): сначала другой текст запроса, затем предупреждения.
    for (key, what) in [
        ("text", "bundle.text (a different form of the query text)"),
        ("warnings", "bundle.warnings"),
    ] {
        if json_size(v) <= limit {
            break;
        }
        if let Some(bundle) = v.get_mut("bundle").and_then(Value::as_object_mut)
            && bundle.remove(key).is_some()
        {
            omitted.push(what.to_string());
        }
    }
    omitted
}

/// Элемент `list_proposals` (§8.2): без текстов (они у модели есть), с решением пользователя и итогом сравнения
/// (`Comparison::for_model`: без планов).
pub fn proposal_for_model(p: &LlmProposal) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("proposalId".into(), p.id.clone().into());
    if let Some(r) = &p.request_id {
        m.insert("requestId".into(), r.clone().into());
    }
    m.insert("connectionId".into(), p.connection_id.clone().into());
    m.insert("database".into(), p.database.clone().into());
    m.insert("status".into(), p.status.as_str().into());
    if let Some(r) = &p.reason {
        m.insert("reason".into(), r.clone().into());
    }
    if let Some(c) = &p.comparison {
        m.insert("comparison".into(), c.for_model());
    }
    m.insert("createdAtMs".into(), p.created_at_ms.into());
    if let Some(t) = p.answered_at_ms {
        m.insert("answeredAtMs".into(), t.into());
    }
    Value::Object(m)
}

fn get_request(st: &AppState, p: GetRequestParams) -> Result<PgOutput, ToolError> {
    let mut r = match p.request_id.as_deref().map(str::trim) {
        Some(id) => open_request(st, id)?,
        None => st
            .llm
            .requests
            .latest_waiting(&|id| check_connection(st, id).is_ok())
            .ok_or_else(|| {
                ToolError::not_found(
                    "No optimization request is waiting: ask the user to press “Send to LLM assistant” in the EXPLAIN console or the query card in Glafira",
                )
            })?,
    };
    // Вытесненные предложения остаются в списке запроса (хранилища независимы) — модели уходят только существующие.
    r.proposal_ids
        .retain(|id| st.llm.proposals.get(id).is_some());
    let name = st
        .profile(&r.connection_id)
        .map(|c| c.name)
        .unwrap_or_default();
    Ok(PgOutput::Request(request_for_model(&r, &name)?))
}

/// Текст исходного запроса: известен до сервера (снимок пользователя, `sql` модели; `PlanText` — с приведениями
/// типизированных литералов) или текст pgss по `queryId` — читается на узле.
enum Original {
    Text(PlanText),
    QueryId(i64),
}

/// Исходный запрос предложения: из запроса на оптимизацию или из параметров модели.
struct Origin {
    request_id: Option<String>,
    connection_id: String,
    database: String,
    /// Узел снимка; без запроса — «за лидером».
    member: Option<String>,
    original: Original,
    /// `queryId` исходного запроса строкой (карточка запроса ищет предложения по нему).
    query_id: Option<String>,
    /// Значения `$N` консоли — для пробы плана предложения.
    values: Vec<Option<String>>,
}

/// Поля прямой формы рядом с `requestId` (смоук P4b, M6): совпадающие с запросом принимаются и ничего не меняют —
/// подключение и база (база — после снятия краевых пробелов), SQL после `normalize` (краевые пробелы и хвостовые `;`)
/// и приведений типизированных литералов (`date $1` и `$1::date` — один запрос),
/// `queryId` запроса карточки (как число). Первое несовпадение — `invalid_argument` с полем и подсказкой
/// `WITH_REQUEST_ONLY`. `member` в параметрах нет: лишнее поле модели разбор пропускает.
fn request_conflict(
    r: &OptimizationRequest,
    query_id: Option<&str>,
    p: &ProposeParams,
) -> Result<(), ToolError> {
    let same_id = |a: &str, b: &str| match (a.trim().parse::<i64>(), b.trim().parse::<i64>()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a.trim() == b.trim(),
    };
    let conflict = if let Some(c) = p.connection_id.as_deref().map(str::trim)
        && c != r.connection_id
    {
        Some(format!("connectionId {c} is not the connection of"))
    } else if let Some(d) = p.database.as_deref().map(str::trim)
        && d != r.database
    {
        Some(format!(
            "database {d} is not the database ({}) of",
            r.database
        ))
    } else if let Some(sql) = p.sql.as_deref()
        && normalize(&PlanText::new(sql).text) != normalize(&PlanText::new(&r.sql).text)
    {
        Some("sql differs from the query of".to_string())
    } else if let Some(q) = p.query_id.as_deref()
        && !query_id.is_some_and(|known| same_id(q, known))
    {
        Some(format!("queryId {} is not the query of", q.trim()))
    } else {
        None
    };
    match conflict {
        Some(c) => Err(invalid_argument(format!(
            "{c} optimization request {}: {WITH_REQUEST_ONLY}",
            r.id
        ))),
        None => Ok(()),
    }
}

fn origin(st: &AppState, p: &ProposeParams) -> Result<Origin, ToolError> {
    if let Some(id) = p.request_id.as_deref().map(str::trim) {
        let r = open_request(st, id)?;
        let query_id = match &r.source {
            RequestSource::Query { query_id } => Some(query_id.clone()),
            RequestSource::Console => None,
        };
        request_conflict(&r, query_id.as_deref(), p)?;
        return Ok(Origin {
            request_id: Some(r.id),
            connection_id: r.connection_id,
            database: r.database,
            member: Some(r.member),
            original: Original::Text(PlanText::new(&r.sql)),
            query_id,
            values: r.params,
        });
    }
    // Без запроса `ProposeParams::validate` уже потребовал подключение и ровно одно из `sql` / `queryId`; база —
    // заданная или база подключения (решение владельца Q2, как §16.21).
    let connection_id = p.connection_id.clone().unwrap_or_default();
    let (profile, _) = check_connection(st, &connection_id)?;
    let database = resolve_database(&profile, p.database.as_deref());
    let (original, query_id) = match asked(p.sql.as_deref(), p.query_id.as_deref())? {
        Asked::Sql(plan) => (Original::Text(plan), None),
        Asked::QueryId(q) => (Original::QueryId(q), Some(q.to_string())),
    };
    Ok(Origin {
        request_id: None,
        connection_id,
        database,
        member: None,
        original,
        query_id,
        values: Vec::new(),
    })
}

/// Вид исходного запроса (§8.2: предложение — того же вида). SQL модели и текст pgss уже прошли ту же проверку; снимок
/// пользователя мог её не пройти (консоль принимает и другое, а текст pgss карточки бывает нормализован так, что не
/// разбирается и после приведений `PlanText`) — тогда модель предлагает в прямой форме, со своим текстом исходного
/// запроса.
fn original_kind(plan: &PlanText, request_id: Option<&str>) -> Result<StatementKind, ToolError> {
    check_model_sql(&plan.text, Purpose::Explain)
        .map(|c| c.kind)
        .map_err(|e| plan.restore(e))
        .map_err(|e| match request_id {
            Some(id) => invalid_argument(format!(
                "The original query of optimization request {id} is not accepted by the gateway ({}: {}): propose with connectionId and sql (the original query with values) instead of requestId",
                e.code, e.message
            )),
            None => e,
        })
}

fn kind_name(k: StatementKind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// Предложение — того же вида, что исходный запрос (§8.2): SELECT остаётся SELECT.
fn same_kind(original: StatementKind, rewritten: StatementKind) -> Result<(), ToolError> {
    if original == rewritten {
        return Ok(());
    }
    Err(err(
        "invalid_sql",
        format!(
            "rewrittenSql must be the same kind of statement as the original ({}), got {}",
            kind_name(original),
            kind_name(rewritten)
        ),
    ))
}

/// Строки `indexes` — проверки `hypothetical_index` (`check_index_ddls`: имя снимается — E4, `sqlcheck` `HypoIndex`,
/// правило P3b `validate_ddl`), отказ — с номером строки. Итог — проверенный текст без имени, нормализованный (без
/// краевых пробелов и хвостовой `;`): он хранится в предложении и идёт в hypopg при сравнении.
fn proposal_indexes(ddls: &[String]) -> Result<Vec<String>, ToolError> {
    if ddls.len() > HYPO_INDEXES_MAX {
        return Err(invalid_argument(format!(
            "pass at most {HYPO_INDEXES_MAX} indexes, got {}",
            ddls.len()
        )));
    }
    ddls.iter()
        .enumerate()
        .map(|(i, ddl)| {
            let checked = check_index_ddls(std::slice::from_ref(ddl))
                .map_err(|e| err(&e.code, format!("indexes[{i}]: {}", e.message)))?;
            Ok(checked.first().map(|d| normalize(d)).unwrap_or_default())
        })
        .collect()
}

/// Параметры пробы плана предложенного SQL — то же правило, что у «Сравнить планов» (`compare::params_for`): без `$N` —
/// без параметров; значения запроса консоли — если их столько же; иначе обобщённый план (16+). На старых версиях
/// обобщённого плана нет, и пробы нет (без значений план не построить; разбор и типы уже проверил `prepare`).
fn probe_params(n: usize, values: &[Option<String>], version_num: i32) -> Option<Params> {
    let params = params_for(n, values);
    (params != Params::Generic || version_num >= V16).then_some(params)
}

/// Узел снимка запроса пропал (смена топологии): запасного «лидера» нет — предложение к запросу привязано к узлу, на
/// котором снимали план; модели советуют прямую форму (`connectionId` и `sql`).
fn direct_form_hint(e: ToolError, request_id: Option<&str>, member: Option<&str>) -> ToolError {
    match (request_id, member) {
        (Some(id), Some(m)) => err(
            &e.code,
            format!(
                "{} (the node {m} of optimization request {id} is not available now: propose in the direct form with connectionId and sql instead of requestId)",
                e.message
            ),
        ),
        _ => e,
    }
}

/// `propose_query_rewrite` (§8.2): доступ и исходный запрос, SQL модели и индексы до сервера, вид выражения; затем
/// `prepare` и проба плана предложенного SQL в слоях §7.3 на узле снимка (без запроса — «за лидером»); сохранение и
/// события (`store_proposal`).
async fn propose(
    st: &AppState,
    p: ProposeParams,
    client: Option<String>,
) -> Result<PgOutput, ToolError> {
    let o = origin(st, &p)?;
    // До сервера — те же проверки, что у explain_query и hypothetical_index (типизированные литералы — приведения,
    // `PlanText`); позиции — в тексте модели (§16.17).
    let rewritten_sql = PlanText::new(&p.rewritten_sql);
    let rewritten = check_model_sql(&rewritten_sql.text, Purpose::Explain)
        .map_err(|e| rewritten_sql.restore(e))?;
    let indexes = proposal_indexes(p.indexes.as_deref().unwrap_or_default())?;
    if let Original::Text(plan) = &o.original {
        same_kind(
            original_kind(plan, o.request_id.as_deref())?,
            rewritten.kind,
        )?;
    }
    let rationale = plain_text(&p.rationale);
    if rationale.is_empty() {
        return Err(invalid_argument(
            "rationale must not be empty: explain the rewrite to the user",
        ));
    }
    ensure_open(st, &o.connection_id).await?;
    let member = member_or_leader(st, &o.connection_id, o.member.as_deref())
        .map_err(|e| direct_form_hint(e, o.request_id.as_deref(), o.member.as_deref()))?;
    let original = match o.original {
        Original::Text(plan) => plan.text,
        Original::QueryId(q) => {
            let source = sql_source(
                st,
                &o.connection_id,
                &member,
                &o.database,
                Asked::QueryId(q),
            )
            .await?;
            same_kind(original_kind(&source.plan, None)?, rewritten.kind)?;
            source.plan.text
        }
    };
    // Слои §7.3: READ ONLY, сроки EXPLAIN, `prepare` под точкой сохранения, проба плана с поиском запрещённого. Текст —
    // для плана (`PlanText`): `GuardTx` нормализует сам, позиции ошибок сервера `restore` возвращает в текст модели.
    let tx = guard::begin(
        st,
        &o.connection_id,
        Some(&member),
        &o.database,
        EXPLAIN_TIMEOUT_S,
    )
    .await?;
    let checked = async {
        tx.prepare_one(&rewritten_sql.text).await?;
        let n = count_params(&normalize(&rewritten_sql.text));
        if let Some(params) = probe_params(n, &o.values, tx.version_num()) {
            tx.probe(&rewritten_sql.text, &params).await?;
        }
        Ok::<_, ToolError>(())
    }
    .await;
    tx.finish().await;
    checked.map_err(|e| rewritten_sql.restore(e))?;
    let new = NewProposal {
        source: ProposalSource::Mcp,
        client,
        request_id: o.request_id,
        connection_id: o.connection_id,
        database: o.database,
        member: Some(member),
        original_sql: normalize(&original),
        original_query_id: o.query_id,
        rewritten_sql: normalize(&rewritten_sql.text),
        indexes,
        rationale,
        expected_effect: p
            .expected_effect
            .as_deref()
            .map(plain_text)
            .filter(|e| !e.is_empty()),
    };
    Ok(PgOutput::Proposed(store_proposal(st, new).await?))
}

/// Сохранение проверенного предложения: такое же (§8.2: тот же исходный запрос, SQL и индексы) — возвращается
/// существующее, без событий; новое прикрепляется к запросу (статус `answered`) и даёт события запроса и предложения.
/// Запрос могли отменить, пока шла проверка на сервере, — тогда предложение не сохраняется (`request_closed`).
async fn store_proposal(st: &AppState, new: NewProposal) -> Result<ProposedOut, ToolError> {
    if let Some(rid) = &new.request_id
        && st
            .llm
            .requests
            .get(rid)
            .is_some_and(|r| r.status == RequestStatus::Cancelled)
    {
        return Err(request_closed(rid));
    }
    let stores = st.llm.clone();
    let (proposal, created) = off_thread(move || stores.proposals.add_or_existing(new))
        .await
        .map_err(store_error)?;
    if created {
        if let Some(rid) = proposal.request_id.clone() {
            let (stores, pid, id) = (st.llm.clone(), proposal.id.clone(), rid.clone());
            if let Err(e) = off_thread(move || stores.requests.attach_proposal(&id, &pid)).await {
                // Отмена между проверкой и прикреплением: предложение не остаётся без запроса — хранилища согласованы,
                // событий нет (интерфейс о нём не узнает), модель получает `request_closed`.
                let (stores, pid) = (st.llm.clone(), proposal.id.clone());
                let _ = off_thread(move || stores.proposals.remove(&pid)).await;
                return Err(store_error(e));
            }
            if let Some(r) = st.llm.requests.get(&rid) {
                st.llm.emit(ProposalsEvent::request(&r));
            }
        }
        st.llm.emit(ProposalsEvent::proposal(&proposal));
    }
    Ok(ProposedOut {
        proposal_id: proposal.id,
        status: proposal.status.as_str().to_string(),
    })
}

/// `list_proposals` (§8.2): предложения подключений, открытых для модели, сначала новые; фильтр по подключению и
/// запросу (закрытое для модели — `not_allowed`, как у остальных инструментов).
fn list(st: &AppState, p: ListProposalsParams) -> Result<PgOutput, ToolError> {
    let mut filter = ProposalFilter::default();
    if let Some(id) = p.connection_id.as_deref() {
        check_connection(st, id)?;
        filter.connection_id = Some(id.to_string());
    }
    // Запрос могли вытеснить из последних 50, а его предложения остаются: фильтр по `request_id` напрямую, доступ —
    // по подключению каждого предложения.
    if let Some(rid) = p.request_id.as_deref().map(str::trim) {
        filter.request_ids = Some(vec![rid.to_string()]);
    }
    let items = st
        .llm
        .proposals
        .list(&filter)
        .iter()
        .filter(|x| check_connection(st, &x.connection_id).is_ok())
        .map(proposal_for_model)
        .collect();
    Ok(PgOutput::Proposals(items))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::*;
    use crate::conn::profile::{LlmAccess, sample_profile};
    use crate::mcp::llm::proposals::{Comparison, IndexUse, ProposalStatus};
    use crate::mcp::llm::{ProposalsEvent, ProposalsEventKind, ProposalsSink};
    use crate::secrets::MemoryStore;

    fn state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        for (id, level) in [("c1", LlmAccess::Diagnostics), ("c2", LlmAccess::Off)] {
            let mut p = sample_profile(id);
            p.llm_access = level;
            st.connections.upsert(p).unwrap();
        }
        (dir, st)
    }

    fn propose_params(request_id: &str, sql: &str) -> ProposeParams {
        ProposeParams {
            request_id: Some(request_id.into()),
            connection_id: None,
            database: None,
            sql: None,
            query_id: None,
            rewritten_sql: sql.into(),
            indexes: None,
            rationale: "faster".into(),
            expected_effect: None,
        }
    }

    fn direct_params(connection_id: &str, sql: &str, rewritten: &str) -> ProposeParams {
        ProposeParams {
            request_id: None,
            connection_id: Some(connection_id.into()),
            sql: Some(sql.into()),
            ..propose_params("r", rewritten)
        }
    }

    fn new_request(conn: &str) -> crate::mcp::llm::requests::NewRequest {
        crate::mcp::llm::requests::tests::new(conn)
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<ProposalsEvent>>);

    impl ProposalsSink for Recorder {
        fn proposals_changed(&self, e: &ProposalsEvent) {
            self.0.lock().unwrap().push(e.clone());
        }
    }

    #[tokio::test]
    async fn get_request_checks_access_and_status() {
        let (_d, st) = state();
        let a = st.llm.requests.create(new_request("c1")).unwrap();
        let hidden = st.llm.requests.create(new_request("c2")).unwrap();
        let get = |id: Option<&str>| {
            handle(
                &st,
                LlmOp::GetRequest {
                    params: GetRequestParams {
                        request_id: id.map(String::from),
                    },
                    run: None,
                },
            )
        };
        let PgOutput::Request(v) = get(Some(&a.id)).await.unwrap().body else {
            panic!()
        };
        assert_eq!(v["id"], a.id.as_str());
        assert_eq!(v["status"], "waiting");
        assert_eq!(v["connectionName"], "conn-c1");
        assert_eq!(
            v["bundle"]["planText"], "Seq Scan on orders",
            "снимок как есть, без маскирования"
        );
        assert_eq!(v["sql"], "SELECT * FROM orders WHERE status = $1");
        assert_eq!(v["params"], json!(["new"]));
        let next = v["next"].as_str().unwrap();
        assert!(
            next.contains(&a.id) && next.contains("propose_query_rewrite") && next.contains("pg-1"),
            "{next}"
        );
        assert!(v.get("hint").is_none(), "маленький снимок — целиком");
        // Без id (и с пустым id — E2) — последний ожидающий среди открытых для модели (c2 закрыто).
        for id in [None, Some(" ")] {
            let PgOutput::Request(v) = get(id).await.unwrap().body else {
                panic!()
            };
            assert_eq!(v["id"], a.id.as_str());
        }
        assert_eq!(get(Some(&hidden.id)).await.unwrap_err().code, "not_allowed");
        let e = get(Some("r000000")).await.unwrap_err();
        assert_eq!(e.code, "not_found");
        assert!(e.message.contains("last 50"), "{}", e.message);
        st.llm.requests.cancel(&a.id).unwrap();
        let e = get(Some(&a.id)).await.unwrap_err();
        assert_eq!(e.code, "request_closed");
        assert!(e.message.contains("cancelled by the user"), "{}", e.message);
        let e = get(None).await.unwrap_err();
        assert_eq!(e.code, "not_found");
        assert!(e.message.contains("Send to LLM assistant"), "{}", e.message);
    }

    /// Вытесненные предложения остаются в `proposalIds` запроса (Task 3) — модели уходят только существующие.
    #[tokio::test]
    async fn get_request_skips_evicted_proposal_ids() {
        let (_d, st) = state();
        let r = st.llm.requests.create(new_request("c1")).unwrap();
        let (p, _) = st
            .llm
            .proposals
            .add_or_existing(crate::mcp::llm::proposals::tests::new(
                Some(&r.id),
                "SELECT 1",
                &[],
            ))
            .unwrap();
        st.llm.requests.attach_proposal(&r.id, "p999999").unwrap();
        st.llm.requests.attach_proposal(&r.id, &p.id).unwrap();
        let out = handle(
            &st,
            LlmOp::GetRequest {
                params: GetRequestParams {
                    request_id: Some(r.id.clone()),
                },
                run: None,
            },
        )
        .await
        .unwrap();
        let PgOutput::Request(v) = out.body else {
            panic!()
        };
        assert_eq!(v["status"], "answered");
        assert_eq!(v["proposalIds"], json!([p.id]));
    }

    /// Решение владельца Q3: снимок помещается в ответ 100 КБ — `bundle.text`, равный `sql`, не повторяется; DDL таблиц
    /// уходят с конца (`tablesOmitted` растёт), затем режется конец текста плана; `hint` говорит, что опущено.
    #[test]
    fn request_for_the_model_fits_the_response() {
        let mut r = st_request();
        r.sql = "SELECT * FROM orders WHERE status = $1".into();
        let tables: Vec<Value> = (0..8)
            .map(
                |i| json!({ "schema": "shop", "name": format!("t{i}"), "ddl": "x".repeat(12_000) }),
            )
            .collect();
        r.bundle = json!({
            "text": " SELECT * FROM orders WHERE status = $1; ",
            "planText": "Seq Scan on orders\n".repeat(1_000),
            "tables": tables,
            "tablesOmitted": 2,
            "warnings": []
        });
        let v = request_for_model(&r, "prod-db").unwrap();
        let size = serde_json::to_vec(&v).unwrap().len();
        assert!(size <= HARD_LIMIT_BYTES, "{size}");
        assert!(
            v["bundle"].get("text").is_none(),
            "текст запроса — уже в sql"
        );
        let kept = v["bundle"]["tables"].as_array().unwrap().len();
        assert!(kept < 8, "{kept}");
        assert_eq!(v["bundle"]["tablesOmitted"], json!(2 + 8 - kept));
        assert_eq!(
            v["bundle"]["planText"],
            json!("Seq Scan on orders\n".repeat(1_000)),
            "таблиц хватило — план целиком"
        );
        let hint = v["hint"].as_str().unwrap();
        assert!(
            hint.contains("bundle.tables") && hint.contains("get_table_details"),
            "{hint}"
        );
        assert!(
            hint.contains(&format!("shop.t{kept}")) && hint.contains("shop.t7"),
            "имена опущенных таблиц: {hint}"
        );
        assert!(!hint.contains("planText"), "{hint}");
        assert_eq!(v["connectionName"], "prod-db");

        // План больше ответа: таблицы уходят все, план режется с пометкой.
        r.bundle["planText"] = json!("Index Scan using orders_pkey on orders\n".repeat(4_000));
        r.bundle["text"] = json!("SELECT other");
        let v = request_for_model(&r, "prod-db").unwrap();
        assert!(serde_json::to_vec(&v).unwrap().len() <= HARD_LIMIT_BYTES);
        assert_eq!(v["bundle"]["text"], "SELECT other", "другой текст остаётся");
        assert_eq!(v["bundle"]["tables"], json!([]));
        assert_eq!(v["bundle"]["tablesOmitted"], json!(10));
        let plan = v["bundle"]["planText"].as_str().unwrap();
        assert!(
            plan.ends_with("…[truncated]"),
            "{}",
            &plan[plan.len() - 40..]
        );
        assert!(plan.starts_with("Index Scan using orders_pkey on orders\n"));
        let hint = v["hint"].as_str().unwrap();
        assert!(
            hint.contains("planText") && hint.contains("explain_query"),
            "{hint}"
        );
    }

    /// Отмена между проверкой и прикреплением (здесь — запроса уже нет в хранилище): предложение не остаётся без
    /// запроса, событий нет, модель получает ошибку.
    #[tokio::test]
    async fn a_failed_attach_removes_the_just_added_proposal() {
        let (_d, st) = state();
        let rec = Arc::new(Recorder::default());
        st.llm.set_sink(rec.clone());
        let e = store_proposal(
            &st,
            crate::mcp::llm::proposals::tests::new(Some("r0000aa"), "SELECT 1", &[]),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, "not_found");
        assert!(st.llm.proposals.list(&ProposalFilter::default()).is_empty());
        assert!(rec.0.lock().unwrap().is_empty(), "событий нет");
    }

    /// Узел снимка пропал: нет запасного «лидера», модели советуют прямую форму; без запроса текст не меняется.
    #[test]
    fn a_missing_snapshot_member_points_to_the_direct_form() {
        let e = direct_form_hint(
            invalid_argument("Unknown member pg-1"),
            Some("r3a9f1c"),
            Some("pg-1"),
        );
        assert_eq!(e.code, "invalid_argument");
        assert!(
            e.message.starts_with("Unknown member pg-1")
                && e.message.contains("direct form with connectionId and sql"),
            "{}",
            e.message
        );
        let e = direct_form_hint(invalid_argument("Unknown member pg-1"), None, None);
        assert_eq!(e.message, "Unknown member pg-1");
    }

    fn size_of(v: &Value) -> usize {
        serde_json::to_vec(v).unwrap().len()
    }

    /// Q3, подсказка в бюджете: длинные квалифицированные имена опущенных таблиц и план чуть больше предела — итоговый
    /// ответ (вместе с `hint`) не больше предела ядра; в подсказке имена не перечисляются целиком.
    #[test]
    fn snapshot_with_the_hint_stays_within_the_limit() {
        for (tables, ddl, plan_lines) in [
            (30, 10_000, 100),
            (12, 9_000, 2_500),
            (40, 4_000, 3_000),
            (3, 40_000, 4_000),
        ] {
            let mut r = st_request();
            r.bundle = json!({
                "planText": "Index Scan using orders_pkey on orders\n".repeat(plan_lines),
                "tables": (0..tables).map(|i| json!({
                    "schema": "public", "name": format!("order_line_items_2024_{i:02}"), "ddl": "x".repeat(ddl)
                })).collect::<Vec<_>>(),
                "warnings": []
            });
            let v = request_for_model(&r, "prod-db").unwrap();
            let size = size_of(&v);
            assert!(
                size <= HARD_LIMIT_BYTES,
                "{tables}/{ddl}/{plan_lines}: {size}"
            );
            let hint = v["hint"].as_str().unwrap();
            assert!(hint.len() < 700, "подсказка короткая: {hint}");
        }
        // Таблиц много — перечислены первые три и «and N more».
        let mut r = st_request();
        r.bundle = json!({
            "tables": (0..30).map(|i| json!({
                "schema": "public", "name": format!("order_line_items_2024_{i:02}"), "ddl": "x".repeat(10_000)
            })).collect::<Vec<_>>(),
        });
        let v = request_for_model(&r, "c").unwrap();
        let hint = v["hint"].as_str().unwrap();
        assert!(hint.contains("and ") && hint.contains(" more"), "{hint}");
    }

    /// Q3: `params` тоже в бюджете — длинные значения режутся с пометкой, об этом сказано в подсказке.
    #[test]
    fn huge_params_are_truncated_to_fit() {
        let mut r = st_request();
        r.params = (0..50).map(|_| Some("v".repeat(5_000))).collect();
        r.bundle = json!({ "planText": "Seq Scan on orders\n".repeat(5_000), "tables": [] });
        let v = request_for_model(&r, "c").unwrap();
        assert!(size_of(&v) <= HARD_LIMIT_BYTES, "{}", size_of(&v));
        let params = v["params"].as_array().unwrap();
        assert_eq!(params.len(), 50, "число значений то же");
        assert!(
            params[0].as_str().unwrap().ends_with("…[truncated]"),
            "{}",
            params[0]
        );
        assert!(v["hint"].as_str().unwrap().contains("params"), "{v}");
        // Совсем много значений: итог всё равно в пределе.
        r.params = (0..30_000).map(|_| Some("v".repeat(100))).collect();
        let v = request_for_model(&r, "c").unwrap();
        assert!(size_of(&v) <= HARD_LIMIT_BYTES, "{}", size_of(&v));
        assert!(v["hint"].as_str().unwrap().contains("params"), "{v}");
    }

    fn st_request() -> OptimizationRequest {
        OptimizationRequest {
            id: "r3a9f1c".into(),
            connection_id: "c1".into(),
            member: "pg-1".into(),
            database: "shop".into(),
            source: RequestSource::Console,
            sql: "SELECT 1".into(),
            params: vec![],
            bundle: json!({}),
            created_at_ms: 1,
            status: RequestStatus::Waiting,
            proposal_ids: vec![],
        }
    }

    #[tokio::test]
    async fn propose_rejects_before_the_server() {
        let (_d, st) = state();
        let r = st.llm.requests.create(new_request("c1")).unwrap();
        let propose = |p: ProposeParams| {
            handle(
                &st,
                LlmOp::Propose {
                    params: p,
                    client: None,
                    run: None,
                },
            )
        };
        assert_eq!(
            propose(propose_params(&r.id, "DROP TABLE orders"))
                .await
                .unwrap_err()
                .code,
            "invalid_sql"
        );
        let e = propose(propose_params(&r.id, "  SELECT pg_sleep(10)"))
            .await
            .unwrap_err();
        assert_eq!(e.code, "forbidden_function");
        assert!(
            e.message.ends_with("(position 10)"),
            "позиция — в тексте модели: {}",
            e.message
        );
        // Вид выражения — тот же, что у исходного (SELECT снимка).
        let e = propose(propose_params(&r.id, "UPDATE orders SET status = 'x'"))
            .await
            .unwrap_err();
        assert_eq!(e.code, "invalid_sql");
        assert!(e.message.contains("select"), "{}", e.message);
        let mut p = propose_params(&r.id, "SELECT id FROM orders");
        p.indexes = Some(vec![
            "CREATE INDEX ON orders (id)".into(),
            "CREATE UNIQUE INDEX ON orders (id)".into(),
        ]);
        let e = propose(p).await.unwrap_err();
        assert_eq!(e.code, "invalid_sql");
        assert!(e.message.starts_with("indexes[1]: "), "{}", e.message);
        // Прямая форма: закрытое для модели подключение, SQL модели — те же проверки.
        assert_eq!(
            propose(direct_params("c2", "SELECT 1", "SELECT 1"))
                .await
                .unwrap_err()
                .code,
            "not_allowed"
        );
        assert_eq!(
            propose(direct_params("c1", "SELECT pg_sleep(1)", "SELECT 1"))
                .await
                .unwrap_err()
                .code,
            "forbidden_function"
        );
        let e = propose(direct_params("c1", "DELETE FROM orders", "SELECT 1"))
            .await
            .unwrap_err();
        assert_eq!(e.code, "invalid_sql");
        assert!(e.message.contains("delete"), "{}", e.message);
        // Параметры проверяются и без шлюза (`validate`): с requestId поле прямой формы, которое не совпадает с запросом,
        // — `invalid_argument` (M6).
        let mut p = direct_params("c1", "SELECT 1", "SELECT 1");
        p.request_id = Some(r.id.clone());
        assert_eq!(propose(p).await.unwrap_err().code, "invalid_argument");
        assert_eq!(
            propose(propose_params("r000000", "SELECT 1"))
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
        st.llm.requests.cancel(&r.id).unwrap();
        assert_eq!(
            propose(propose_params(&r.id, "SELECT id FROM orders"))
                .await
                .unwrap_err()
                .code,
            "request_closed"
        );
        assert!(st.llm.proposals.list(&ProposalFilter::default()).is_empty());
    }

    /// Смоук P4b, M6: рядом с requestId модели присылают поля прямой формы. Совпадающие с запросом (подключение, база,
    /// тот же SQL после снятия краевых пробелов и `;`, queryId запроса карточки) принимаются и ничего не меняют; не
    /// совпадающие — `invalid_argument` с названием поля и подсказкой «only rewrittenSql…».
    #[tokio::test]
    async fn propose_with_a_request_accepts_matching_fields_and_names_conflicts() {
        let (_d, st) = state();
        let r = st.llm.requests.create(new_request("c1")).unwrap();
        let mut q = new_request("c1");
        q.source = RequestSource::Query {
            query_id: "-42".into(),
        };
        let card = st.llm.requests.create(q).unwrap();
        // Ревью правки B, #2: снимок карточки с типизированным литералом; `sql` модели с приведением — тот же запрос.
        let mut t = new_request("c1");
        t.sql = "SELECT * FROM orders WHERE created > date $1".into();
        let typed = st.llm.requests.create(t).unwrap();
        let propose = |p: ProposeParams| {
            handle(
                &st,
                LlmOp::Propose {
                    params: p,
                    client: None,
                    run: None,
                },
            )
        };
        let with = |id: &str, f: &dyn Fn(&mut ProposeParams)| {
            // UPDATE — не тот вид, что у SELECT запроса: отказ после сверки с запросом, до сервера.
            let mut p = propose_params(id, "UPDATE orders SET status = 'x'");
            f(&mut p);
            p
        };
        for p in [
            with(&r.id, &|p| {
                p.connection_id = Some("c1".into());
                p.database = Some("shop".into());
                p.sql = Some("  SELECT * FROM orders WHERE status = $1 ;".into());
            }),
            with(&card.id, &|p| p.query_id = Some(" -42 ".into())),
            with(&r.id, &|p| p.query_id = Some(r.id.clone())),
            with(&typed.id, &|p| {
                p.sql = Some("SELECT * FROM orders WHERE created > $1::date;".into())
            }),
        ] {
            let e = propose(p).await.unwrap_err();
            assert_eq!(
                (e.code.as_str(), e.message.contains("same kind")),
                ("invalid_sql", true),
                "совпадающие поля приняты: {}",
                e.message
            );
        }
        for (p, field) in [
            (
                with(&r.id, &|p| p.connection_id = Some("c2".into())),
                "connectionId c2",
            ),
            (
                with(&r.id, &|p| p.database = Some("other".into())),
                "database other",
            ),
            (with(&r.id, &|p| p.sql = Some("SELECT 1".into())), "sql"),
            (
                with(&r.id, &|p| p.query_id = Some("-42".into())),
                "queryId -42",
            ),
            (
                with(&card.id, &|p| p.query_id = Some("7".into())),
                "queryId 7",
            ),
        ] {
            let e = propose(p).await.unwrap_err();
            assert_eq!(e.code, "invalid_argument", "{}", e.message);
            assert!(
                e.message.starts_with(&format!("{field} "))
                    && e.message.ends_with(WITH_REQUEST_ONLY),
                "{}",
                e.message
            );
        }
        assert!(st.llm.proposals.list(&ProposalFilter::default()).is_empty());
    }

    /// Снимок, который не проходит проверку шлюза (консоль принимает и другое), — предложение в прямой форме.
    #[tokio::test]
    async fn an_unchecked_original_points_to_the_direct_form() {
        let (_d, st) = state();
        let mut n = new_request("c1");
        n.sql = "SELECT * FROM orders WHERE status =".into();
        let r = st.llm.requests.create(n).unwrap();
        let e = handle(
            &st,
            LlmOp::Propose {
                params: propose_params(&r.id, "SELECT 1"),
                client: None,
                run: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, "invalid_argument");
        assert!(
            e.message.contains(&r.id) && e.message.contains("connectionId and sql"),
            "{}",
            e.message
        );
    }

    #[test]
    fn rewrite_must_keep_the_statement_kind() {
        let kind = |sql: &str| check_model_sql(sql, Purpose::Explain).unwrap().kind;
        assert!(
            same_kind(
                kind("SELECT 1"),
                kind("WITH x AS (SELECT 1) SELECT * FROM x")
            )
            .is_ok()
        );
        let e = same_kind(kind("SELECT * FROM t"), kind("UPDATE t SET a = 1")).unwrap_err();
        assert_eq!(e.code, "invalid_sql");
        assert_eq!(
            e.message,
            "rewrittenSql must be the same kind of statement as the original (select), got update"
        );
    }

    /// Строки индексов — проверки `hypothetical_index` (E4: имя снимается) с номером строки; хранится проверенный
    /// нормализованный текст (без имени и `;`) — он же идёт в hypopg при сравнении.
    #[test]
    fn proposal_indexes_drop_names_and_name_the_failed_line() {
        let ddls = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(proposal_indexes(&[]).unwrap(), Vec::<String>::new());
        assert_eq!(
            proposal_indexes(&ddls(&[
                "CREATE INDEX ON shop.orders (status)",
                " CREATE INDEX idx_orders_id ON shop.orders (id); ",
            ]))
            .unwrap(),
            ddls(&[
                "CREATE INDEX ON shop.orders (status)",
                "CREATE INDEX ON shop.orders (id)"
            ])
        );
        let e = proposal_indexes(&ddls(&["CREATE INDEX ON t (a)"; 6])).unwrap_err();
        assert_eq!(e.code, "invalid_argument");
        let e = proposal_indexes(&ddls(&[
            "CREATE INDEX ON t (a)",
            "CREATE INDEX ON t ((pg_sleep(1)))",
        ]))
        .unwrap_err();
        assert_eq!(e.code, "forbidden_function");
        assert!(e.message.starts_with("indexes[1]: "), "{}", e.message);
        let e = proposal_indexes(&ddls(&["CREATE INDEX ON t (a) /* x */"])).unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message.starts_with("indexes[0]: ")),
            ("invalid_sql", true),
            "{}",
            e.message
        );
    }

    #[test]
    fn probe_params_follow_the_values_and_the_version() {
        assert_eq!(probe_params(0, &[], 130_000), Some(Params::None));
        let values = vec![Some("new".to_string())];
        assert_eq!(
            probe_params(1, &values, 130_000),
            Some(Params::Values {
                values: values.clone()
            })
        );
        assert_eq!(
            probe_params(2, &values, 170_000),
            Some(Params::Generic),
            "16+ — обобщённый план"
        );
        assert_eq!(
            probe_params(2, &values, 150_000),
            None,
            "значений не хватает, GENERIC_PLAN нет — проба не делается"
        );
    }

    /// Сохранение после проверок на сервере: новое предложение прикрепляется к запросу (статус `answered`), события
    /// запроса и предложения; повтор — то же предложение без событий; шестое на запрос — `invalid_argument`.
    #[tokio::test]
    async fn stored_proposal_answers_the_request_and_a_repeat_is_the_same() {
        let (_d, st) = state();
        let rec = Arc::new(Recorder::default());
        st.llm.set_sink(rec.clone());
        let r = st.llm.requests.create(new_request("c1")).unwrap();
        let new = |sql: &str| NewProposal {
            request_id: Some(r.id.clone()),
            ..crate::mcp::llm::proposals::tests::new(Some(&r.id), sql, &[])
        };
        let out = store_proposal(&st, new("SELECT 1")).await.unwrap();
        assert_eq!(out.status, "new");
        let got = st.llm.requests.get(&r.id).unwrap();
        assert_eq!(got.status, RequestStatus::Answered);
        assert_eq!(got.proposal_ids, vec![out.proposal_id.clone()]);
        let events = rec.0.lock().unwrap().clone();
        assert_eq!(
            events.iter().map(|e| e.kind).collect::<Vec<_>>(),
            [ProposalsEventKind::Request, ProposalsEventKind::Proposal]
        );
        assert_eq!(
            events[1].proposal_id.as_deref(),
            Some(out.proposal_id.as_str())
        );
        st.llm
            .proposals
            .answer(&out.proposal_id, ProposalStatus::Accepted, None)
            .unwrap();
        let again = store_proposal(&st, new(" SELECT 1 ")).await.unwrap();
        assert_eq!(
            (again.proposal_id.as_str(), again.status.as_str()),
            (out.proposal_id.as_str(), "accepted")
        );
        assert_eq!(rec.0.lock().unwrap().len(), 2, "повтор — без событий");
        for i in 2..=5 {
            store_proposal(&st, new(&format!("SELECT {i}")))
                .await
                .unwrap();
        }
        let e = store_proposal(&st, new("SELECT 6")).await.unwrap_err();
        assert_eq!(e.code, "invalid_argument");
        assert!(
            e.message.contains("review them in Glafira"),
            "{}",
            e.message
        );
        // Запрос отменили, пока шла проверка на сервере: предложение не сохраняется.
        let w = st.llm.requests.create(new_request("c1")).unwrap();
        st.llm.requests.cancel(&w.id).unwrap();
        let e = store_proposal(
            &st,
            NewProposal {
                request_id: Some(w.id.clone()),
                ..crate::mcp::llm::proposals::tests::new(Some(&w.id), "SELECT 1", &[])
            },
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, "request_closed");
        assert!(
            st.llm
                .proposals
                .list(&ProposalFilter {
                    request_ids: Some(vec![w.id.clone()]),
                    ..Default::default()
                })
                .is_empty()
        );
    }

    #[tokio::test]
    async fn list_shows_only_open_connections_and_strips_plans() {
        let (_d, st) = state();
        let mut n = crate::mcp::llm::proposals::tests::new(None, "SELECT 1", &[]);
        let (p1, _) = st.llm.proposals.add_or_existing(n.clone()).unwrap();
        n.connection_id = "c2".into();
        st.llm.proposals.add_or_existing(n).unwrap();
        st.llm
            .proposals
            .answer(&p1.id, ProposalStatus::Rejected, Some("slower".into()))
            .unwrap();
        let list = |p: ListProposalsParams| {
            handle(
                &st,
                LlmOp::ListProposals {
                    params: p,
                    run: None,
                },
            )
        };
        let PgOutput::Proposals(items) = list(ListProposalsParams {
            connection_id: None,
            request_id: None,
        })
        .await
        .unwrap()
        .body
        else {
            panic!()
        };
        assert_eq!(items.len(), 1, "предложение подключения c2 (off) не видно");
        assert_eq!(items[0]["proposalId"], p1.id.as_str());
        assert_eq!(items[0]["status"], "rejected");
        assert_eq!(items[0]["reason"], "slower");
        assert!(items[0].get("requestId").is_none());
        assert!(items[0].get("rewrittenSql").is_none());
        assert_eq!(
            list(ListProposalsParams {
                connection_id: Some("c2".into()),
                request_id: None
            })
            .await
            .unwrap_err()
            .code,
            "not_allowed"
        );
        // Фильтр по запросу идёт по `request_id` предложений: запрос закрытого подключения и неизвестный запрос — пусто
        // (доступ проверяется по подключению каждого предложения), вытесненный запрос с живыми предложениями — они.
        let hidden = st.llm.requests.create(new_request("c2")).unwrap();
        let mut in_hidden =
            crate::mcp::llm::proposals::tests::new(Some(&hidden.id), "SELECT 3", &[]);
        in_hidden.connection_id = "c2".into();
        st.llm.proposals.add_or_existing(in_hidden).unwrap();
        let by_request = |id: &str| {
            let id = id.to_string();
            async {
                let PgOutput::Proposals(items) = list(ListProposalsParams {
                    connection_id: None,
                    request_id: Some(id),
                })
                .await
                .unwrap()
                .body
                else {
                    panic!()
                };
                items
            }
        };
        assert!(by_request(&hidden.id).await.is_empty());
        assert!(by_request("r000000").await.is_empty());
        let mut evicted = crate::mcp::llm::proposals::tests::new(Some("r0e41c7"), "SELECT 4", &[]);
        evicted.connection_id = "c1".into();
        let (pe, _) = st.llm.proposals.add_or_existing(evicted).unwrap();
        assert!(st.llm.requests.get("r0e41c7").is_none());
        let items = by_request("r0e41c7").await;
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0]["proposalId"], pe.id.as_str());
        let r = st.llm.requests.create(new_request("c1")).unwrap();
        let (p2, _) = st
            .llm
            .proposals
            .add_or_existing(crate::mcp::llm::proposals::tests::new(
                Some(&r.id),
                "SELECT 2",
                &[],
            ))
            .unwrap();
        let PgOutput::Proposals(items) = list(ListProposalsParams {
            connection_id: Some(" ".into()),
            request_id: Some(r.id.clone()),
        })
        .await
        .unwrap()
        .body
        else {
            panic!()
        };
        assert_eq!(items.len(), 1);
        assert_eq!(
            (
                items[0]["proposalId"].as_str(),
                items[0]["requestId"].as_str()
            ),
            (Some(p2.id.as_str()), Some(r.id.as_str()))
        );
    }

    #[test]
    fn proposal_for_the_model_carries_the_comparison_summary() {
        let mut p = LlmProposal {
            id: "p1".into(),
            source: ProposalSource::Mcp,
            client: Some("qwen-code".into()),
            request_id: Some("r1".into()),
            connection_id: "c1".into(),
            database: "shop".into(),
            member: Some("pg-1".into()),
            original_sql: "SELECT 1".into(),
            original_query_id: None,
            rewritten_sql: "SELECT 1".into(),
            indexes: vec![],
            rationale: "r".into(),
            expected_effect: None,
            status: ProposalStatus::Accepted,
            reason: None,
            comparison: None,
            created_at_ms: 3,
            answered_at_ms: Some(4),
        };
        assert_eq!(
            proposal_for_model(&p),
            json!({ "proposalId": "p1", "requestId": "r1", "connectionId": "c1", "database": "shop",
                    "status": "accepted", "createdAtMs": 3, "answeredAtMs": 4 })
        );
        let plan = pg_advisor::plan::json::from_json(&json!([{ "Plan": {
            "Node Type": "Result", "Startup Cost": 0, "Total Cost": 1.5, "Plan Rows": 1, "Plan Width": 4
        } }]))
        .unwrap();
        p.comparison = Some(Comparison {
            cost_before: 12.0,
            cost_after: 1.5,
            indexes_used: vec![IndexUse {
                ddl: "CREATE INDEX ON t (a)".into(),
                used: true,
            }],
            member: "pg-1".into(),
            at_ms: 9,
            plan_before: plan.clone(),
            plan_after: plan,
            recommendations_before: vec![],
            recommendations_after: vec![],
            without_hypopg: false,
        });
        let v = proposal_for_model(&p);
        assert_eq!(
            v["comparison"],
            json!({ "costBefore": 12.0, "costAfter": 1.5,
                    "indexesUsed": [{ "ddl": "CREATE INDEX ON t (a)", "used": true }],
                    "member": "pg-1", "atMs": 9, "withoutHypopg": false })
        );
    }

    #[test]
    fn store_errors_reach_the_model_with_their_codes() {
        let e = store_error(crate::mcp::llm::requests::request_closed("r1"));
        assert_eq!(e.code, "request_closed");
        let e = store_error(crate::error::AppError::internal_with("disk"));
        assert_eq!(e.code, "internal");
    }
}
