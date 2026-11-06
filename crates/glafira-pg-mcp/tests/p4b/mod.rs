//! Обвязка тестов P4b: фальшивый `PgBackend` (подтверждает любого клиента, запоминает операции) и клиент rmcp поверх
//! `tokio::io::duplex` (сервер одной сессии — `serve_stream` ядра).
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use glafira_mcp_core::audit::AuditLog;
use glafira_mcp_core::error::{BoxFuture, ToolError};
use glafira_mcp_core::gate::{Gate, SessionAccess, SessionView};
use glafira_mcp_core::server::{GatewayOptions, serve_stream};
use glafira_pg_mcp::model::{ObserveCtx, PgAnswer, PgBackend, PgObserve, PgRead, SelectOp};
use glafira_pg_mcp::server::{INSTRUCTIONS, PgFactory};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;

type Reply<Op> = Box<dyn Fn(&Op) -> Result<PgAnswer, ToolError> + Send + Sync>;

pub struct Fake {
    /// Операции `select` и срок, переданный в `ObserveCtx`.
    pub selects: Mutex<Vec<(SelectOp, Duration)>>,
    pub select_reply: Reply<SelectOp>,
}

impl Fake {
    pub fn new() -> Self {
        Self {
            selects: Mutex::default(),
            select_reply: Box::new(|_| Err(ToolError::internal("no select reply in this test"))),
        }
    }

    pub fn on_select(
        mut self,
        f: impl Fn(&SelectOp) -> Result<PgAnswer, ToolError> + Send + Sync + 'static,
    ) -> Self {
        self.select_reply = Box::new(f);
        self
    }
}

impl Gate for Fake {
    fn access(&self, _s: &SessionView) -> SessionAccess {
        SessionAccess::Approved
    }
    fn connection_name(&self, id: &str) -> Option<String> {
        Some(format!("conn-{id}"))
    }
    fn session_closed(&self, _session_id: &str) {}
    fn sessions_changed(&self) {}
}

impl PgBackend for Fake {
    fn read(&self, _op: PgRead) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        Box::pin(async { Err(ToolError::internal("read is not used in P4b tests")) })
    }
    fn observe(
        &self,
        _op: PgObserve,
        _ctx: ObserveCtx,
    ) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        Box::pin(async { Err(ToolError::internal("observe is not used in P4b tests")) })
    }
    fn select(&self, op: SelectOp, ctx: ObserveCtx) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        let reply = (self.select_reply)(&op);
        self.selects.lock().unwrap().push((op, ctx.duration));
        Box::pin(async move { reply })
    }
}

/// Клиент rmcp, подключённый к серверу одной сессии с бэкендом `fake`. Каталог журнала живёт, пока работает сервер
/// сессии (он внутри его задачи), и удаляется вместе с ней в конце теста.
pub async fn client(fake: Arc<Fake>) -> RunningService<RoleClient, ()> {
    let dir = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(dir.path()));
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    let options = GatewayOptions::new("glafira-pg", "0.0.0-test", INSTRUCTIONS);
    let server = serve_stream(
        Box::new(server_io),
        fake.clone(),
        audit,
        options,
        Arc::new(PgFactory::new(fake)),
    );
    tokio::spawn(async move {
        let _dir = dir;
        server.await;
    });
    ().serve(client_io).await.unwrap()
}

pub async fn call(c: &RunningService<RoleClient, ()>, tool: &str, args: Value) -> CallToolResult {
    let mut p = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(m) = args {
        p = p.with_arguments(m);
    }
    c.call_tool(p).await.unwrap()
}

pub fn text(r: &CallToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

pub fn ok(r: CallToolResult) -> Value {
    assert_ne!(r.is_error, Some(true), "{}", text(&r));
    r.structured_content.unwrap()
}

pub fn err(r: &CallToolResult) -> String {
    assert_eq!(r.is_error, Some(true), "{}", text(r));
    text(r)
}
