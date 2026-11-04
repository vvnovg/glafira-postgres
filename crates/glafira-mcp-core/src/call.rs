//! Обвязка вызова инструмента (спец. P4 §4.3, §10): перенос `GatewayServer::{identify, admit, begin_long, slots, call}`
//! и тела `list_tools` из `server/mod.rs` Glafira for Kafka в [`SessionCtx`] — его получает сервер сессии крейта
//! приложения. Порядок вызова: опознание клиента → право (`Gate::access`) → лимит частоты → отметка «идёт вызов» →
//! тело → JSON-объект не больше 100 КБ → ответ или ошибка → запись в журнал.

use std::future::Future;
use std::sync::Arc;

use rmcp::RoleServer;
use rmcp::model::{
    CacheScope, CallToolResult, ContentBlock, Implementation, ListToolsResult, ProtocolVersion,
    ResultType, Tool,
};
use rmcp::service::RequestContext;
use serde_json::{Map, Value};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::audit::{AuditConnection, AuditEvent, CallRecord, Outcome, truncate_args};
use crate::error::ToolError;
use crate::gate::{ClientIdent, SessionAccess, SessionView};
use crate::limits::{CALLS_PER_MINUTE, HARD_LIMIT_BYTES, SESSION_CONCURRENCY};
use crate::server::GatewayOptions;
use crate::session::Shared;

/// Поля аргументов, которые журнал хранит целиком до 16 КБ (спец. §10.3).
pub const AUDIT_FULL_FIELDS: [&str; 2] = ["sql", "rewrittenSql"];
/// Имя и версия клиента приходят от него самого: длина ограничена (диалог, `settings.json`).
const MAX_CLIENT_FIELD_CHARS: usize = 128;

/// Что пишется в журнал о вызове: инструмент, аргументы (до усечения), подключение и узел.
pub struct CallMeta<'a> {
    pub tool: &'a str,
    pub args: Value,
    pub connection_id: Option<&'a str>,
    pub member: Option<&'a str>,
}

/// Успешный ответ: JSON-объект для `structuredContent` и счётчики приложения для журнала (`masked`, `rows`).
#[derive(Debug, Clone, PartialEq)]
pub struct CallOk {
    pub value: Value,
    pub extra: Map<String, Value>,
}

impl CallOk {
    pub fn new(value: Value) -> Self {
        Self {
            value,
            extra: Map::new(),
        }
    }

    pub fn with_extra(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.extra.insert(key.to_string(), value.into());
        self
    }
}

/// Контекст сессии для сервера rmcp крейта приложения: обвязка каждого вызова, опознание клиента и подтверждение,
/// слоты параллельности и длительные операции.
#[derive(Clone)]
pub struct SessionCtx {
    shared: Arc<Shared>,
    id: String,
    /// Слоты операций с базой этой сессии.
    local: Arc<Semaphore>,
}

/// Отметка «идёт длительная операция»; снимается при сбросе.
pub struct LongGuard {
    ctx: SessionCtx,
}

impl Drop for LongGuard {
    fn drop(&mut self) {
        if let Some(e) = self.ctx.shared.sessions().get_mut(&self.ctx.id) {
            e.long_running = false;
        }
    }
}

/// Слоты параллельности операции с базой: сессии и всех сессий; освобождаются при сбросе.
pub struct DbSlots {
    _local: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
}

/// Отметка «идёт вызов» (индикатор в интерфейсе); снимается при любом выходе.
struct CallFlag {
    ctx: SessionCtx,
}

impl Drop for CallFlag {
    fn drop(&mut self) {
        if let Some(e) = self.ctx.shared.sessions().get_mut(&self.ctx.id) {
            e.in_flight = e.in_flight.saturating_sub(1);
        }
        self.ctx.shared.gate.sessions_changed();
    }
}

fn capped(s: &str) -> String {
    s.trim().chars().take(MAX_CLIENT_FIELD_CHARS).collect()
}

fn normalize_client(c: Implementation) -> Option<ClientIdent> {
    let name = capped(&c.name);
    (!name.is_empty()).then(|| ClientIdent {
        name,
        version: capped(&c.version),
    })
}

fn closed() -> ToolError {
    ToolError::new("mcp_disabled", "The MCP session is closed")
}

fn tool_error(e: &ToolError) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(format!(
        "{}: {}",
        e.code, e.message
    ))])
}

impl SessionCtx {
    pub(crate) fn new(shared: Arc<Shared>, id: String) -> Self {
        Self {
            shared,
            id,
            local: Arc::new(Semaphore::new(SESSION_CONCURRENCY)),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.id
    }

    pub fn options(&self) -> &GatewayOptions {
        &self.shared.options
    }

    /// Клиент запоминается при первом запросе, где он известен (`initialize` или `_meta` протокола 2026-07-28), и
    /// дальше не меняется; имя без пробелов по краям, пустое — как отсутствующее (спец. §4.4).
    pub fn identify(&self, client: Option<Implementation>) -> SessionView {
        let client = client.and_then(normalize_client);
        let mut changed = false;
        let known = {
            let mut sessions = self.shared.sessions();
            match sessions.get_mut(&self.id) {
                Some(e) => {
                    if e.client.is_none()
                        && let Some(c) = client
                    {
                        e.client = Some(c);
                        changed = true;
                    }
                    e.client.clone()
                }
                None => None,
            }
        };
        if changed {
            self.shared.gate.sessions_changed();
        }
        SessionView {
            id: self.id.clone(),
            client: known,
        }
    }

    /// Подтверждение запрашивается сразу при подключении (`initialize`, `tools/list`), а не при первом вызове:
    /// приложение ставит новую сессию в очередь в `Gate::access`.
    pub fn request_approval(&self, view: &SessionView) {
        let _ = self.shared.gate.access(view);
    }

    fn cancel_session(&self) {
        if let Some(e) = self.shared.sessions().get(&self.id) {
            e.cancel.cancel();
        }
    }

    /// Право, лимит частоты и отметка «идёт вызов».
    fn admit(&self, view: &SessionView) -> Result<CallFlag, ToolError> {
        match self.shared.gate.access(view) {
            SessionAccess::Approved => {}
            SessionAccess::Pending => {
                return Err(ToolError::new(
                    "client_not_approved",
                    "The user has not approved this MCP client in Glafira yet. Ask them to approve it in the Glafira window, then retry.",
                ));
            }
            SessionAccess::Rejected => {
                self.cancel_session();
                return Err(ToolError::new(
                    "client_rejected",
                    "The user rejected this MCP client in Glafira",
                ));
            }
            SessionAccess::Disabled => {
                return Err(ToolError::new("mcp_disabled", "MCP is disabled in Glafira"));
            }
        }
        {
            let mut sessions = self.shared.sessions();
            let Some(e) = sessions.get_mut(&self.id) else {
                return Err(closed());
            };
            if !e.limiter.try_take() {
                return Err(ToolError::new(
                    "rate_limited",
                    format!("Too many calls: at most {CALLS_PER_MINUTE} per minute; retry later"),
                ));
            }
            e.calls += 1;
            e.in_flight += 1;
        }
        self.shared.gate.sessions_changed();
        Ok(CallFlag { ctx: self.clone() })
    }

    /// Одна длительная операция на сессию (`observe_*`); вторая — `busy` с текстом `busy_message`.
    pub fn begin_long(&self, busy_message: &str) -> Result<LongGuard, ToolError> {
        {
            let mut sessions = self.shared.sessions();
            let e = sessions.get_mut(&self.id).ok_or_else(closed)?;
            if e.long_running {
                return Err(ToolError::new("busy", busy_message));
            }
            e.long_running = true;
        }
        Ok(LongGuard { ctx: self.clone() })
    }

    /// Слоты параллельности операции с базой: сначала сессии (2), затем всех сессий (4).
    pub async fn db_slots(&self) -> Result<DbSlots, ToolError> {
        let local = self
            .local
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ToolError::internal("The MCP session is closing"))?;
        let global = self
            .shared
            .global
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ToolError::internal("The MCP server is stopping"))?;
        Ok(DbSlots {
            _local: local,
            _global: global,
        })
    }

    /// Общая обвязка инструмента: право, лимит, предел размера, журнал, ответ или ошибка для LLM.
    pub async fn call<Fut>(
        &self,
        rctx: &RequestContext<RoleServer>,
        meta: CallMeta<'_>,
        body: Fut,
    ) -> CallToolResult
    where
        Fut: Future<Output = Result<CallOk, ToolError>>,
    {
        let started = tokio::time::Instant::now();
        let view = self.identify(rctx.client_info());
        let result = match self.admit(&view) {
            Ok(_flag) => body.await,
            Err(e) => Err(e),
        };
        let result = result.and_then(|ok| {
            if !ok.value.is_object() {
                return Err(ToolError::internal("The tool response is not a JSON object"));
            }
            if serde_json::to_vec(&ok.value).map_or(usize::MAX, |b| b.len()) > HARD_LIMIT_BYTES {
                return Err(ToolError::new(
                    "response_too_large",
                    "The response is larger than 100 KB; narrow the request: a member, filters or a smaller limit",
                ));
            }
            Ok(ok)
        });
        let (out, outcome, code, bytes, extra) = match result {
            Ok(CallOk { value, extra }) => {
                let r = CallToolResult::structured(value);
                let bytes = r
                    .content
                    .iter()
                    .map(|c| c.as_text().map_or(0, |t| t.text.len()))
                    .sum();
                (r, Outcome::Ok, None, bytes, extra)
            }
            Err(e) => (tool_error(&e), Outcome::Error, Some(e.code), 0, Map::new()),
        };
        self.shared.audit.record(AuditEvent::Call(CallRecord {
            session: self.id.clone(),
            client: view.client.map(|c| format!("{} {}", c.name, c.version)),
            tool: meta.tool.to_string(),
            connection: meta.connection_id.map(|id| AuditConnection {
                id: id.to_string(),
                name: self.shared.gate.connection_name(id).unwrap_or_default(),
            }),
            member: meta.member.map(str::to_string),
            args: truncate_args(meta.args, &AUDIT_FULL_FIELDS),
            outcome,
            code,
            bytes,
            ms: started.elapsed().as_millis() as u64,
            extra,
        }));
        out
    }

    /// Ответ `tools/list`. Клиент протокола 2026-07-28 не шлёт `initialize`, а список инструментов просит сразу после
    /// подключения: клиент — из `_meta` запроса, подтверждение — здесь, как при `initialize`. Подсказки кеша — только
    /// для 2026-07-28 (тело — как у `#[tool_handler]` rmcp 3.5).
    pub fn list_tools_result(
        &self,
        tools: Vec<Tool>,
        rctx: &RequestContext<RoleServer>,
    ) -> ListToolsResult {
        let view = self.identify(rctx.client_info());
        self.request_approval(&view);
        let cache_hints = rctx
            .protocol_version()
            .is_some_and(|v| v >= ProtocolVersion::V_2026_07_28);
        ListToolsResult {
            result_type: Some(ResultType::COMPLETE),
            tools,
            meta: None,
            next_cursor: None,
            ttl_ms: cache_hints.then_some(0),
            cache_scope: cache_hints.then_some(CacheScope::Public),
        }
    }
}
