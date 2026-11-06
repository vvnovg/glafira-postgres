//! Запросы на оптимизацию и предложения LLM (спец. P4 §8): хранилища `llm-requests.json` / `llm-proposals.json`,
//! событие `mcp://proposals`, записи журнала MCP `requestCreated` и `proposalAnswered`.

mod list_file;
pub mod proposals;
pub mod requests;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use glafira_mcp_core::audit::AuditEvent;
use pg_advisor::plan::PlanTree;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tauri::{AppHandle, Emitter};

use crate::conn::profile::LlmAccess;
use crate::diag::query_detail::parse_query_id;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::explain::MAX_SQL_BYTES;
use crate::explain::llm::LlmSource;
use crate::explain::params::normalize;
use crate::mcp::access::effective_access;
use crate::state::AppState;
use proposals::{LlmProposal, ProposalStatus, ProposalStore};
use requests::{NewRequest, OptimizationRequest, RequestSource, RequestStore};

/// Предел SQL снимка (решение владельца Q3): снимок целиком должен поместиться в ответ MCP в 100 КБ. SQL не
/// обрезается молча — модель должна видеть настоящий запрос.
pub const MAX_SNAPSHOT_SQL_BYTES: usize = 65_536;

/// Событие для консоли и карточки: новое предложение, ответ или сравнение; смена статуса запроса.
pub const PROPOSALS_EVENT: &str = "mcp://proposals";

/// Получатель `mcp://proposals`. В приложении — окно Tauri, в тестах — запись событий.
pub trait ProposalsSink: Send + Sync {
    fn proposals_changed(&self, e: &ProposalsEvent);
}

struct TauriSink(AppHandle);

impl ProposalsSink for TauriSink {
    fn proposals_changed(&self, e: &ProposalsEvent) {
        emit_proposals_event(&self.0, e.clone());
    }
}

pub struct LlmStores {
    pub requests: RequestStore,
    pub proposals: ProposalStore,
    /// Получатель `mcp://proposals`; в тестах его нет — событие не отправляется.
    sink: OnceLock<Arc<dyn ProposalsSink>>,
}

impl LlmStores {
    pub fn open(dir: &Path) -> Self {
        Self {
            requests: RequestStore::open(dir),
            proposals: ProposalStore::open(dir),
            sink: OnceLock::new(),
        }
    }

    /// Окно приложения — получатель `mcp://proposals` (вызывается один раз в `setup`).
    pub fn set_app_handle(&self, h: AppHandle) {
        self.set_sink(Arc::new(TauriSink(h)));
    }

    pub(crate) fn set_sink(&self, sink: Arc<dyn ProposalsSink>) {
        let _ = self.sink.set(sink);
    }

    pub fn emit(&self, e: ProposalsEvent) {
        if let Some(sink) = self.sink.get() {
            sink.proposals_changed(&e);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalsEventKind {
    Request,
    Proposal,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalsEvent {
    pub kind: ProposalsEventKind,
    pub connection_id: String,
    pub request_id: Option<String>,
    pub proposal_id: Option<String>,
}

impl ProposalsEvent {
    pub fn request(r: &OptimizationRequest) -> Self {
        Self {
            kind: ProposalsEventKind::Request,
            connection_id: r.connection_id.clone(),
            request_id: Some(r.id.clone()),
            proposal_id: None,
        }
    }

    pub fn proposal(p: &LlmProposal) -> Self {
        Self {
            kind: ProposalsEventKind::Proposal,
            connection_id: p.connection_id.clone(),
            request_id: p.request_id.clone(),
            proposal_id: Some(p.id.clone()),
        }
    }
}

pub fn emit_proposals_event(app: &AppHandle, e: ProposalsEvent) {
    if let Err(err) = app.emit(PROPOSALS_EVENT, e) {
        tracing::warn!("emit {PROPOSALS_EVENT}: {err}");
    }
}

/// Новый id: буква и 6 hex (`r3a9f1c`, `p0b12de`); занятый — следующая попытка.
pub(crate) fn new_id(prefix: char, taken: impl Fn(&str) -> bool) -> String {
    loop {
        let u = uuid::Uuid::new_v4().simple().to_string();
        let id = format!("{prefix}{}", &u[..6]);
        if !taken(&id) {
            return id;
        }
    }
}

/// Текст модели или пользователя «как текст» (обоснование, эффект, причина): без управляющих символов, кроме перевода
/// строки и табуляции, без пробелов по краям.
pub fn plain_text(s: &str) -> String {
    s.chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

/// Запись файлов хранилищ — вне потоков async: файл запросов со снимками бывает до нескольких мегабайт.
pub(crate) async fn off_thread<T: Send + 'static>(
    f: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::internal_with(format!("storage task failed: {e}")))?
}

/// Аргументы `create_optimization_request`: что открыто в консоли или карточке.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequestArgs {
    pub member: String,
    pub database: String,
    pub source: RequestSource,
    /// Текст консоли или текст pgss карточки.
    pub sql: String,
    #[serde(default)]
    pub params: Vec<Option<String>>,
    /// Последний построенный план (как у «Скопировать для LLM»); `None` — пробный план без выполнения.
    #[serde(default)]
    pub plan: Option<PlanTree>,
}

fn connection_field(id: &str, name: &str) -> Value {
    json!({ "id": id, "name": name })
}

/// Запрос на оптимизацию (спец. §8.1): доступен, если действующий уровень подключения не `off` и MCP включён. Снимок —
/// `LlmBundle` P3 без маскирования: его отправляет сам пользователь (решение владельца 8). Запись `requestCreated`.
pub async fn create_request(
    st: &AppState,
    connection_id: &str,
    args: CreateRequestArgs,
) -> AppResult<OptimizationRequest> {
    let profile = st.profile(connection_id)?;
    if effective_access(&profile, &st.policy.status) == LlmAccess::Off {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.llm.accessOff",
            "LLM access is off for this connection: turn it on in the connection settings"
        ));
    }
    if !(st.settings.mcp().enabled && st.policy.status.mcp_allowed()) {
        return Err(app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.llm.mcpOff",
            "The MCP server is off: turn it on in Settings → LLM assistants (MCP)"
        ));
    }
    let sql = normalize(&args.sql);
    if sql.is_empty() {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.emptySql",
            "Enter a query"
        ));
    }
    if sql.len() > MAX_SQL_BYTES {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.sqlTooLong",
            "The query is longer than {{max}} bytes",
            max = MAX_SQL_BYTES
        ));
    }
    if sql.len() > MAX_SNAPSHOT_SQL_BYTES {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.llm.sqlTooLong",
            "The query is longer than {{max}} bytes: it is too long to use “Send to LLM assistant”",
            max = MAX_SNAPSHOT_SQL_BYTES
        ));
    }
    let source = match &args.source {
        RequestSource::Console => LlmSource::Sql { sql: sql.clone() },
        RequestSource::Query { query_id } => LlmSource::Query {
            query_id: parse_query_id(query_id)?,
        },
    };
    let bundle = crate::explain::llm::bundle(
        st,
        connection_id,
        &args.member,
        &args.database,
        source,
        args.plan,
    )
    .await?;
    let new = NewRequest {
        connection_id: connection_id.to_string(),
        member: args.member,
        database: args.database,
        source: args.source,
        sql,
        params: args.params,
        bundle: serde_json::to_value(&bundle)?,
    };
    let stores = st.llm.clone();
    let r = off_thread(move || stores.requests.create(new)).await?;
    let mut fields = Map::new();
    fields.insert("requestId".into(), r.id.clone().into());
    fields.insert(
        "connection".into(),
        connection_field(&r.connection_id, &profile.name),
    );
    st.mcp.record_audit(AuditEvent::App {
        kind: "requestCreated".into(),
        fields,
    });
    st.llm.emit(ProposalsEvent::request(&r));
    Ok(r)
}

/// «Отменить» запрос: модель получит `request_closed`. Меняет только `Waiting`; у `Answered` и
/// `Cancelled` ничего не меняется, события нет.
pub async fn cancel_request(st: &AppState, request_id: &str) -> AppResult<OptimizationRequest> {
    let (stores, id) = (st.llm.clone(), request_id.to_string());
    let (r, changed) = off_thread(move || stores.requests.cancel_changed(&id)).await?;
    if changed {
        st.llm.emit(ProposalsEvent::request(&r));
    }
    Ok(r)
}

/// «Принять» / «Отклонить» в панели (спец. §8.4). Повторный ответ разрешён (пользователь передумал:
/// принято ↔ отклонено); каждый ответ пишет `proposalAnswered` в журнал и шлёт событие.
pub async fn answer_proposal(
    st: &AppState,
    id: &str,
    status: &str,
    reason: Option<String>,
) -> AppResult<LlmProposal> {
    let status = match status {
        "accepted" => ProposalStatus::Accepted,
        "rejected" => ProposalStatus::Rejected,
        _ => return Err(proposals::bad_status()),
    };
    let (stores, pid) = (st.llm.clone(), id.to_string());
    let p = off_thread(move || stores.proposals.answer(&pid, status, reason)).await?;
    let name = st
        .profile(&p.connection_id)
        .map(|c| c.name)
        .unwrap_or_default();
    let mut fields = Map::new();
    fields.insert("proposalId".into(), p.id.clone().into());
    fields.insert("status".into(), p.status.as_str().into());
    fields.insert(
        "connection".into(),
        connection_field(&p.connection_id, &name),
    );
    st.mcp.record_audit(AuditEvent::App {
        kind: "proposalAnswered".into(),
        fields,
    });
    st.llm.emit(ProposalsEvent::proposal(&p));
    Ok(p)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::conn::profile::sample_profile;
    use crate::secrets::MemoryStore;

    fn state_with(level: LlmAccess) -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        let mut p = sample_profile("c1");
        p.llm_access = level;
        st.connections.upsert(p).unwrap();
        (dir, st)
    }

    fn args() -> CreateRequestArgs {
        CreateRequestArgs {
            member: "pg-1".into(),
            database: "postgres".into(),
            source: RequestSource::Console,
            sql: "SELECT 1".into(),
            params: vec![],
            plan: None,
        }
    }

    #[tokio::test]
    async fn a_request_needs_llm_access_and_the_mcp_server() {
        let (_d, st) = state_with(LlmAccess::Off);
        assert_eq!(
            create_request(&st, "c1", args()).await.unwrap_err().key,
            "errors.llm.accessOff"
        );
        // Уровень есть, MCP выключен в настройках (так по умолчанию).
        let (_d, st) = state_with(LlmAccess::Diagnostics);
        assert_eq!(
            create_request(&st, "c1", args()).await.unwrap_err().key,
            "errors.llm.mcpOff"
        );
    }

    #[tokio::test]
    async fn cancel_and_answer_go_to_the_store_and_the_journal() {
        let (_d, st) = state_with(LlmAccess::Diagnostics);
        let r = st.llm.requests.create(requests::tests::new("c1")).unwrap();
        assert_eq!(
            cancel_request(&st, &r.id).await.unwrap().status,
            requests::RequestStatus::Cancelled
        );
        let (p, _) = st
            .llm
            .proposals
            .add_or_existing(proposals::tests::new(None, "SELECT 1", &[]))
            .unwrap();
        let a = answer_proposal(&st, &p.id, "rejected", Some("slower on prod".into()))
            .await
            .unwrap();
        assert_eq!(
            (a.status, a.reason.as_deref()),
            (ProposalStatus::Rejected, Some("slower on prod"))
        );
        assert_eq!(
            answer_proposal(&st, &p.id, "maybe", None)
                .await
                .unwrap_err()
                .key,
            "errors.llm.badStatus"
        );
        let journal: Vec<String> = st
            .mcp
            .audit_tail(10)
            .iter()
            .map(|e| e.to_string())
            .collect();
        assert!(
            journal.iter().any(|e| e.contains("proposalAnswered")
                && e.contains(&p.id)
                && e.contains("rejected")),
            "{journal:?}"
        );
    }

    #[test]
    fn event_json_shape() {
        let e = ProposalsEvent {
            kind: ProposalsEventKind::Proposal,
            connection_id: "c1".into(),
            request_id: None,
            proposal_id: Some("p1a2b3c".into()),
        };
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({ "kind": "proposal", "connectionId": "c1", "requestId": null, "proposalId": "p1a2b3c" })
        );
        assert_eq!(plain_text(" a\u{0}b\nc\t "), "ab\nc");
    }

    #[derive(Default)]
    struct Recorder(std::sync::Mutex<Vec<ProposalsEvent>>);

    impl ProposalsSink for Recorder {
        fn proposals_changed(&self, e: &ProposalsEvent) {
            self.0.lock().unwrap().push(e.clone());
        }
    }

    #[tokio::test]
    async fn cancel_and_answer_emit_events() {
        let (_d, st) = state_with(LlmAccess::Diagnostics);
        let rec = Arc::new(Recorder::default());
        st.llm.set_sink(rec.clone());
        let r = st.llm.requests.create(requests::tests::new("c1")).unwrap();
        cancel_request(&st, &r.id).await.unwrap();
        let (p, _) = st
            .llm
            .proposals
            .add_or_existing(proposals::tests::new(Some(&r.id), "SELECT 1", &[]))
            .unwrap();
        answer_proposal(&st, &p.id, "accepted", None).await.unwrap();
        let got = rec.0.lock().unwrap().clone();
        assert_eq!(got.len(), 2);
        assert_eq!(
            got[0],
            ProposalsEvent::request(&st.llm.requests.get(&r.id).unwrap())
        );
        assert_eq!(got[1].kind, ProposalsEventKind::Proposal);
        assert_eq!(got[1].proposal_id.as_deref(), Some(p.id.as_str()));
        assert_eq!(got[1].request_id.as_deref(), Some(r.id.as_str()));
    }

    #[tokio::test]
    async fn cancelling_an_answered_request_is_a_silent_no_op() {
        let (_d, st) = state_with(LlmAccess::Diagnostics);
        let rec = Arc::new(Recorder::default());
        st.llm.set_sink(rec.clone());
        let r = st.llm.requests.create(requests::tests::new("c1")).unwrap();
        st.llm.requests.attach_proposal(&r.id, "p000001").unwrap();
        let got = cancel_request(&st, &r.id).await.unwrap();
        assert_eq!(got.status, requests::RequestStatus::Answered);
        assert!(rec.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_too_long_query_is_refused_not_cut() {
        let (_d, st) = state_with(LlmAccess::Diagnostics);
        st.settings.set_mcp_enabled(true).unwrap();
        let mut a = args();
        a.sql = format!("SELECT '{}'", "x".repeat(MAX_SNAPSHOT_SQL_BYTES));
        let e = create_request(&st, "c1", a).await.unwrap_err();
        assert_eq!(e.key, "errors.llm.sqlTooLong");
        assert!(st.llm.requests.list("c1").is_empty());
        let mut a = args();
        a.source = RequestSource::Query {
            query_id: "abc".into(),
        };
        assert_eq!(
            create_request(&st, "c1", a).await.unwrap_err().key,
            "errors.queries.badQueryId"
        );
    }
}
