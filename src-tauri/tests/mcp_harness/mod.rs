//! Сервер MCP в процессе для интеграционных тестов P4b (спец. P4 §13 «Интеграция»): `serve_stream` ядра поверх
//! `tokio::io::duplex`, настоящий `AppBackend` приложения за `Gate`, который подтверждает любую сессию (подтверждение
//! клиентов проверяют unit-тесты ядра и контроллера), и клиент rmcp. Уровень доступа подключения тест меняет правкой
//! профиля — как форма: действует со следующего вызова. Журнал MCP ведёт тот же файл, что и `st.mcp` (каталог данных
//! приложения): записи шлюза и записи приложения (`requestCreated`, `proposalAnswered`) читает `audit_events`.
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use glafira_mcp_core::audit::AuditLog;
use glafira_mcp_core::endpoint::BoxStream;
use glafira_mcp_core::error::{BoxFuture, ToolError};
use glafira_mcp_core::gate::{Gate, SessionAccess, SessionView};
use glafira_mcp_core::server::{GatewayOptions, serve_stream};
use glafira_pg_lib::conn::profile::LlmAccess;
use glafira_pg_lib::mcp::approvals::Approvals;
use glafira_pg_lib::mcp::backend::AppBackend;
use glafira_pg_lib::state::AppState;
use glafira_pg_mcp::model::{LlmOp, ObserveCtx, PgAnswer, PgBackend, PgObserve, PgRead, SelectOp};
use glafira_pg_mcp::server::{INSTRUCTIONS, PgFactory};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;
use tokio::task::JoinHandle;

/// Бэкенд приложения с подтверждённой сессией: проверки уровня подключения (§4.3, шаги 3–4) остаются в `AppBackend`.
pub struct Approving(pub AppBackend);

impl Gate for Approving {
    fn access(&self, _s: &SessionView) -> SessionAccess {
        SessionAccess::Approved
    }

    fn connection_name(&self, id: &str) -> Option<String> {
        self.0.connection_name(id)
    }

    fn session_closed(&self, _session_id: &str) {}

    fn sessions_changed(&self) {}
}

impl PgBackend for Approving {
    fn read(&self, op: PgRead) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        self.0.read(op)
    }

    fn observe(
        &self,
        op: PgObserve,
        ctx: ObserveCtx,
    ) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        self.0.observe(op, ctx)
    }

    fn select(&self, op: SelectOp, ctx: ObserveCtx) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        self.0.select(op, ctx)
    }

    fn llm(&self, op: LlmOp) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        self.0.llm(op)
    }
}

/// Одна сессия MCP: клиент rmcp и задача сервера (снимается при сбросе).
pub struct Gateway {
    pub client: RunningService<RoleClient, ()>,
    server: JoinHandle<()>,
}

impl Gateway {
    /// Сессия к приложению `st`; журнал MCP — в `dir` (каталог данных теста). MCP включается в настройках, как
    /// переключатель в «Настройках»: без него `AppBackend` отказывает, а `create_request` не создаёт запрос.
    pub async fn start(st: &AppState, dir: &Path) -> Self {
        st.settings.set_mcp_enabled(true).unwrap();
        let (server_io, client_io) = tokio::io::duplex(1 << 20);
        // `Approvals::new(settings)` — конструктор P4a задачи 8 (`Default` у него нет).
        let backend = Arc::new(Approving(AppBackend::new(
            st.clone(),
            Arc::new(Approvals::new(st.settings.clone())),
        )));
        let gate: Arc<dyn Gate> = backend.clone();
        let audit = Arc::new(AuditLog::new(dir));
        let options = GatewayOptions::new(
            String::from("glafira-pg"),
            String::from(env!("CARGO_PKG_VERSION")),
            String::from(INSTRUCTIONS),
        );
        let stream: BoxStream = Box::new(server_io);
        let factory = Arc::new(PgFactory::new(backend));
        let server = tokio::spawn(async move {
            serve_stream(stream, gate, audit, options, factory).await;
        });
        let client = ().serve(client_io).await.expect("initialize MCP");
        Self { client, server }
    }

    pub async fn call(&self, tool: &str, args: Value) -> CallToolResult {
        let mut p = CallToolRequestParams::new(tool.to_string());
        if let Value::Object(m) = args {
            p = p.with_arguments(m);
        }
        self.client.call_tool(p).await.expect("tools/call")
    }

    /// Успешный ответ инструмента — `structuredContent`.
    pub async fn ok(&self, tool: &str, args: Value) -> Value {
        let r = self.call(tool, args).await;
        assert_eq!(r.is_error, Some(false), "{tool}: {}", text(&r));
        r.structured_content.expect("structuredContent")
    }

    /// Текст ошибки инструмента: `<code>: <message>`.
    pub async fn err(&self, tool: &str, args: Value) -> String {
        let r = self.call(tool, args).await;
        assert_eq!(r.is_error, Some(true), "{tool}: {}", text(&r));
        text(&r)
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.server.abort();
    }
}

pub fn text(r: &CallToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

/// Уровень доступа для LLM у подключения `id` — как сохранение формы (без переоткрытия соединения).
pub fn set_access(st: &AppState, id: &str, access: LlmAccess) {
    let mut p = st.connections.get(id).unwrap();
    p.llm_access = access;
    st.connections.upsert(p).unwrap();
}

/// Узел подключения-сервера `it` (единственный участник топологии).
pub fn member(st: &AppState) -> String {
    st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone()
}

/// Записи журнала MCP с `event` = `kind`, старые первыми.
pub fn audit_events(st: &AppState, kind: &str) -> Vec<Value> {
    let mut v: Vec<Value> = st
        .mcp
        .audit_tail(1000)
        .into_iter()
        .filter(|e| e["event"] == kind)
        .collect();
    v.sort_by_key(|e| e["ts"].as_u64());
    v
}
