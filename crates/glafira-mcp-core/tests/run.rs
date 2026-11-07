//! Сессия фонового запуска в ядре (спец. P4c §3.3, §5.1–§5.2, §9.2):
//! - билет в рукопожатии через настоящий сокет (pipe на Windows);
//! - `SessionView.run` и `SessionCtx::run`;
//! - `tools/list` и `prompts/list` через `Gate`;
//! - отказ `check_call` — модели и в журнал;
//! - `run` в журнале и в списке сессий;
//! - подсказка кеша списка;
//! - «Отключить клиента» не трогает сессию запуска.
//!
//! Сервер — игрушечный, как в `tests/server.rs`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use glafira_mcp_core::endpoint::{self, Endpoint};
use glafira_mcp_core::handshake::{self, BAD_TICKET_MESSAGE, HandshakeError, TICKET_LEN};
use glafira_mcp_core::{
    AuditLog, CallMeta, CallOk, Gate, GatewayOptions, McpServerHandle, SessionAccess, SessionCtx,
    SessionFactory, SessionView, ToolError,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig, Implementation,
    InitializeRequestParams, InitializeResult, ListPromptsResult, ListToolsResult,
    PaginatedRequestParams, Prompt, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{Peer, RequestContext, RunningService};
use rmcp::{
    ErrorData, RoleClient, RoleServer, ServerHandler, ServiceExt, tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};

const TOKEN: &str = "test-token";
const RUN: &str = "a1b2c3d";
const OUTSIDE: &str = "not_allowed: Not available to a background run";
const WAIT: Duration = Duration::from_secs(5);

fn ticket() -> String {
    "5a".repeat(TICKET_LEN / 2)
}

// ---- Право ----

/// Билет `ticket()` даёт запуск `RUN`, пока он жив. Сессии запуска видны только `work` и только подключение `c1`;
/// prompts им не видны. Остальные сессии видят всё.
struct RunGate {
    live: AtomicBool,
    seen: Mutex<Vec<SessionView>>,
    checks: Mutex<Vec<(String, Option<String>)>>,
}

impl RunGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            live: AtomicBool::new(true),
            seen: Mutex::default(),
            checks: Mutex::default(),
        })
    }
}

impl Gate for RunGate {
    fn access(&self, s: &SessionView) -> SessionAccess {
        self.seen.lock().unwrap().push(s.clone());
        SessionAccess::Approved
    }

    fn connection_name(&self, id: &str) -> Option<String> {
        Some(format!("conn-{id}"))
    }

    fn session_closed(&self, _session_id: &str) {}

    fn sessions_changed(&self) {}

    fn run_for_ticket(&self, t: &str) -> Option<String> {
        (self.live.load(Ordering::SeqCst) && t == ticket()).then(|| RUN.to_string())
    }

    fn check_call(
        &self,
        s: &SessionView,
        tool: &str,
        connection_id: Option<&str>,
    ) -> Result<(), ToolError> {
        self.checks
            .lock()
            .unwrap()
            .push((tool.to_string(), connection_id.map(str::to_string)));
        let outside = !self.tool_visible(s, tool)
            || (s.run.is_some() && connection_id.is_some_and(|c| c != "c1"));
        if outside {
            return Err(ToolError::new(
                "not_allowed",
                "Not available to a background run",
            ));
        }
        Ok(())
    }

    fn tool_visible(&self, s: &SessionView, tool: &str) -> bool {
        s.run.is_none() || tool == "work"
    }

    fn prompts_visible(&self, s: &SessionView) -> bool {
        s.run.is_none()
    }
}

/// Скрывает `secret` от всех сессий и не переопределяет `check_call`: отказ даёт реализация по умолчанию.
struct Hiding;

impl Gate for Hiding {
    fn access(&self, _s: &SessionView) -> SessionAccess {
        SessionAccess::Approved
    }

    fn connection_name(&self, _id: &str) -> Option<String> {
        None
    }

    fn session_closed(&self, _session_id: &str) {}

    fn sessions_changed(&self) {}

    fn tool_visible(&self, _s: &SessionView, tool: &str) -> bool {
        tool != "secret"
    }
}

// ---- Игрушечный сервер: два инструмента и один prompt ----

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct WorkParams {
    /// Connection id.
    connection_id: String,
}

#[derive(Clone)]
struct Toy {
    ctx: SessionCtx,
    #[allow(dead_code)] // поле читает код макроса tool_handler
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Toy {
    #[tool(
        description = "Toy operation on a connection",
        annotations(read_only_hint = true)
    )]
    async fn work(
        &self,
        Parameters(p): Parameters<WorkParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "work",
            args: json!({ "connectionId": p.connection_id }),
            connection_id: Some(&p.connection_id),
            member: None,
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                Ok(CallOk::new(json!({ "ok": true, "run": self.ctx.run() })))
            })
            .await)
    }

    #[tool(
        description = "Toy operation hidden from background runs",
        annotations(read_only_hint = true)
    )]
    async fn secret(&self, ctx: RequestContext<RoleServer>) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "secret",
            args: json!({}),
            connection_id: None,
            member: None,
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                Ok(CallOk::new(json!({ "secret": true })))
            })
            .await)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Toy {
    fn get_info(&self) -> ServerConfig {
        let o = self.ctx.options();
        let mut c = ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        );
        c.server_info = Implementation::new(&o.name, &o.version);
        c.instructions = Some(o.instructions.clone());
        c
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        context.peer.set_peer_info(request.clone());
        let view = self.ctx.identify(Some(request.client_info.clone()));
        self.ctx.request_approval(&view);
        self.negotiate_initialize(&request)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(self
            .ctx
            .list_tools_result(self.tool_router.list_all(), &context))
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(self
            .ctx
            .list_prompts_result(vec![Prompt::new("toy", Some("Toy prompt"), None)], &context))
    }
}

struct ToyFactory;

impl SessionFactory for ToyFactory {
    type Server = Toy;

    fn create(&self, ctx: SessionCtx) -> Toy {
        Toy {
            ctx,
            tool_router: Toy::tool_router(),
        }
    }
}

fn options() -> GatewayOptions {
    GatewayOptions::new("glafira-pg", "0.0.0-test", "Toy gateway")
}

// ---- Окружение и клиент ----

struct Env {
    _dir: tempfile::TempDir,
    ep: Endpoint,
    handle: McpServerHandle,
    audit: Arc<AuditLog>,
    gate: Arc<RunGate>,
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let ep = Endpoint::for_app(dir.path(), "glafira-pg-run");
    let audit = Arc::new(AuditLog::new(dir.path()));
    let gate = RunGate::new();
    let handle = glafira_mcp_core::start(
        &ep,
        TOKEN.into(),
        gate.clone(),
        audit.clone(),
        options(),
        Arc::new(ToyFactory),
    )
    .await
    .unwrap();
    Env {
        _dir: dir,
        ep,
        handle,
        audit,
        gate,
    }
}

/// Клиент с именем `name`; `ticket` — шим фонового запуска.
async fn client(
    ep: &Endpoint,
    name: &str,
    ticket: Option<&str>,
) -> RunningService<RoleClient, ClientConfig> {
    let mut s = endpoint::connect(ep).await.unwrap();
    handshake::client_with_run(&mut s, TOKEN, ticket)
        .await
        .unwrap();
    ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new(name, "1.0"),
    )
    .serve(s)
    .await
    .unwrap()
}

async fn call(c: &Peer<RoleClient>, tool: &str, args: Value) -> CallToolResult {
    let mut p = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(m) = args {
        p = p.with_arguments(m);
    }
    c.call_tool(p).await.unwrap()
}

fn text(r: &CallToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

/// Успешный ответ: один текстовый блок с JSON-объектом, без `structuredContent` (смоук P4b, G3), как `ok` в
/// `tests/server.rs`.
fn ok(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(false), "{}", text(&r));
    assert_eq!(r.structured_content, None, "{}", text(&r));
    assert_eq!(r.content.len(), 1, "{}", text(&r));
    serde_json::from_str(&text(&r)).unwrap()
}

fn err(r: &CallToolResult) -> String {
    assert_eq!(r.is_error, Some(true), "{}", text(r));
    text(r)
}

fn names(tools: Vec<Tool>) -> Vec<String> {
    let mut v: Vec<String> = tools.into_iter().map(|t| t.name.to_string()).collect();
    v.sort();
    v
}

async fn wait_until(mut cond: impl FnMut() -> bool, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if cond() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Ответ JSON-RPC с номером `id` (уведомления сервера пропускаются).
async fn reply<R: AsyncBufRead + Unpin>(r: &mut R, id: u64) -> Value {
    loop {
        let mut line = String::new();
        assert!(
            r.read_line(&mut line).await.unwrap() > 0,
            "сервер закрыл поток"
        );
        let v: Value = serde_json::from_str(&line).unwrap();
        if v["id"] == id {
            return v;
        }
    }
}

// ---- Тесты ----

#[tokio::test]
async fn run_session_sees_only_its_tools_and_no_prompts() {
    let e = env().await;
    let run = client(&e.ep, "qwen-code", Some(&ticket())).await;
    let chat = client(&e.ep, "qwen-code", None).await;
    assert_eq!(names(run.list_all_tools().await.unwrap()), ["work"]);
    assert_eq!(
        names(chat.list_all_tools().await.unwrap()),
        ["secret", "work"]
    );
    assert!(run.list_all_prompts().await.unwrap().is_empty());
    assert_eq!(chat.list_all_prompts().await.unwrap().len(), 1);

    let v = ok(call(&run, "work", json!({"connectionId": "c1"})).await);
    assert_eq!(v["run"], RUN, "SessionCtx::run");
    let v = ok(call(&chat, "work", json!({"connectionId": "c1"})).await);
    assert!(
        v.get("run").is_none(),
        "null в ответе модели не пишется: {v}"
    );

    let seen = e.gate.seen.lock().unwrap().clone();
    assert!(
        seen.iter().any(|v| v.run.as_deref() == Some(RUN)
            && v.client.as_ref().is_some_and(|c| c.name == "qwen-code")),
        "{seen:?}"
    );
    assert!(seen.iter().any(|v| v.run.is_none()));

    let mut sessions = e.handle.sessions();
    sessions.sort_by_key(|s| s.run_id.is_some());
    assert_eq!(
        (sessions[0].run_id.as_deref(), sessions[1].run_id.as_deref()),
        (None, Some(RUN))
    );
    let json = serde_json::to_value(&sessions).unwrap();
    assert!(json[0].get("runId").is_none(), "{json}");
    assert_eq!(json[1]["runId"], RUN);

    let log = e.audit.tail(20);
    let started: Vec<&Value> = log
        .iter()
        .filter(|r| r["event"] == "sessionStarted")
        .collect();
    assert_eq!(started.len(), 2);
    assert_eq!(started.iter().filter(|r| r["run"] == RUN).count(), 1);
    assert_eq!(started.iter().filter(|r| r.get("run").is_none()).count(), 1);
    let calls: Vec<&Value> = log.iter().filter(|r| r["event"] == "call").collect();
    assert_eq!(calls.iter().filter(|r| r["run"] == RUN).count(), 1);
    assert_eq!(calls.iter().filter(|r| r.get("run").is_none()).count(), 1);
}

#[tokio::test]
async fn scope_refusals_reach_the_model_and_the_audit() {
    let e = env().await;
    let run = client(&e.ep, "claude-code", Some(&ticket())).await;
    assert_eq!(
        err(&call(&run, "work", json!({"connectionId": "c2"})).await),
        OUTSIDE
    );
    assert_eq!(err(&call(&run, "secret", json!({})).await), OUTSIDE);
    ok(call(&run, "work", json!({"connectionId": "c1"})).await);
    assert_eq!(
        *e.gate.checks.lock().unwrap(),
        [
            ("work".to_string(), Some("c2".to_string())),
            ("secret".to_string(), None),
            ("work".to_string(), Some("c1".to_string())),
        ]
    );
    let log = e.audit.tail(3);
    assert_eq!(
        (
            log[2]["tool"].as_str(),
            log[2]["code"].as_str(),
            log[2]["run"].as_str()
        ),
        (Some("work"), Some("not_allowed"), Some(RUN))
    );
    assert_eq!(log[2]["connection"], json!({"id": "c2", "name": "conn-c2"}));
    assert_eq!(
        e.handle.sessions()[0].calls,
        1,
        "отказ области — вне счёта вызовов сессии"
    );
}

#[tokio::test]
async fn hidden_tool_is_not_allowed_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    tokio::spawn(glafira_mcp_core::serve_stream(
        Box::new(server_io),
        Arc::new(Hiding),
        Arc::new(AuditLog::new(dir.path())),
        options(),
        Arc::new(ToyFactory),
    ));
    let c = ().serve(client_io).await.unwrap();
    assert_eq!(names(c.list_all_tools().await.unwrap()), ["work"]);
    assert_eq!(
        err(&call(&c, "secret", json!({})).await),
        "not_allowed: This tool is not available in this session"
    );
    ok(call(&c, "work", json!({"connectionId": "c2"})).await);
}

#[tokio::test]
async fn a_run_that_is_over_is_refused_before_mcp() {
    let e = env().await;
    e.gate.live.store(false, Ordering::SeqCst);
    let mut s = endpoint::connect(&e.ep).await.unwrap();
    let r = handshake::client_with_run(&mut s, TOKEN, Some(&ticket())).await;
    assert!(
        matches!(&r, Err(HandshakeError::Rejected { code, message })
            if code == "bad_ticket" && message == BAD_TICKET_MESSAGE),
        "{r:?}"
    );
    assert!(e.handle.sessions().is_empty(), "сессии нет");
    let _chat = client(&e.ep, "qwen-code", None).await;
    assert_eq!(
        e.handle.sessions().len(),
        1,
        "старая форма рукопожатия работает"
    );
}

#[tokio::test]
async fn disconnecting_a_client_keeps_its_background_run() {
    let e = env().await;
    let run = client(&e.ep, "qwen-code", Some(&ticket())).await;
    let _chat = client(&e.ep, "qwen-code", None).await;
    assert_eq!(
        e.handle.disconnect_client("qwen-code"),
        1,
        "только сессия чата"
    );
    assert!(wait_until(|| e.handle.sessions().len() == 1, WAIT).await);
    assert_eq!(e.handle.sessions()[0].run_id.as_deref(), Some(RUN));
    ok(call(&run, "work", json!({"connectionId": "c1"})).await);
}

/// Список сессии запуска свой — `cacheScope: private`, а не `public` (протокол 2026-07-28 требует подсказку у списка;
/// `serve_run_stream` без сокета).
#[tokio::test]
async fn run_lists_have_no_public_cache_hint() {
    let dir = tempfile::tempdir().unwrap();
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    tokio::spawn(glafira_mcp_core::serve_run_stream(
        Box::new(server_io),
        RUN.to_string(),
        RunGate::new(),
        Arc::new(AuditLog::new(dir.path())),
        options(),
        Arc::new(ToyFactory),
    ));
    let (rd, mut wr) = tokio::io::split(client_io);
    let mut rd = BufReader::new(rd);
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "claude-code", "version": "2.1.318"},
    });
    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": meta.clone()}});
    wr.write_all(format!("{list}\n").as_bytes()).await.unwrap();
    wr.flush().await.unwrap();
    let r = reply(&mut rd, 1).await;
    let tools: Vec<&str> = r["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(tools, ["work"]);
    assert_eq!(r["result"]["ttlMs"], 0);
    assert_eq!(r["result"]["cacheScope"], "private", "{r}");
    let prompts =
        json!({"jsonrpc": "2.0", "id": 2, "method": "prompts/list", "params": {"_meta": meta}});
    wr.write_all(format!("{prompts}\n").as_bytes())
        .await
        .unwrap();
    wr.flush().await.unwrap();
    let r = reply(&mut rd, 2).await;
    assert!(
        r["result"]["prompts"]
            .as_array()
            .is_none_or(|a| a.is_empty()),
        "{r}"
    );
    assert_eq!(r["result"]["cacheScope"], "private", "{r}");
}
