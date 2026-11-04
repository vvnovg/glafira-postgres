//! Ядро целиком (спец. P4 §13): настоящий сокет (pipe на Windows), рукопожатие, клиент rmcp и игрушечный сервер с
//! двумя инструментами поверх `SessionCtx` — так же устроен `PgServer` в `glafira-pg-mcp`. Сервер без сокета
//! (`serve_stream` поверх `tokio::io::duplex`) — в тестах протокола 2026-07-28 и старого `initialize`.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use glafira_mcp_core::endpoint::{self, Endpoint};
use glafira_mcp_core::{
    AuditLog, CallMeta, CallOk, Gate, GatewayOptions, McpServerHandle, SessionAccess, SessionCtx,
    SessionFactory, SessionView, ToolError, handshake,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig,
    ClientRequest, Implementation, InitializeRequestParams, InitializeResult, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig,
};
use rmcp::service::{PeerRequestOptions, RequestContext, RunningService};
use rmcp::{
    ErrorData, RoleClient, RoleServer, ServerHandler, ServiceExt, tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};

const TOKEN: &str = "test-token";
const INSTRUCTIONS: &str = "Toy gateway for the core tests.";

// ---- Право: фальшивый Gate ----

struct FakeGate {
    access: Mutex<SessionAccess>,
    /// Каждый запрос права: при `initialize`, списке инструментов и вызове.
    seen: Mutex<Vec<SessionView>>,
    closed: Mutex<Vec<String>>,
    changes: AtomicUsize,
}

impl FakeGate {
    fn new(access: SessionAccess) -> Arc<Self> {
        Arc::new(Self {
            access: Mutex::new(access),
            seen: Mutex::new(Vec::new()),
            closed: Mutex::new(Vec::new()),
            changes: AtomicUsize::new(0),
        })
    }
}

impl Gate for FakeGate {
    fn access(&self, session: &SessionView) -> SessionAccess {
        self.seen.lock().unwrap().push(session.clone());
        *self.access.lock().unwrap()
    }

    fn connection_name(&self, id: &str) -> Option<String> {
        (id == "c1").then(|| "lab".to_string())
    }

    fn session_closed(&self, session_id: &str) {
        self.closed.lock().unwrap().push(session_id.to_string());
    }

    fn sessions_changed(&self) {
        self.changes.fetch_add(1, Ordering::SeqCst);
    }
}

// ---- Игрушечный сервер сессии ----

/// Наблюдение за инструментами: задержка `work`, одновременных операций сейчас и наибольшее, отмена `long_wait`.
#[derive(Default)]
struct Probe {
    delay_ms: AtomicU64,
    active: AtomicUsize,
    peak: AtomicUsize,
    long_stopped: AtomicBool,
}

/// Считает одновременные операции «с базой» на любом выходе.
struct Busy<'a>(&'a Probe);

impl<'a> Busy<'a> {
    fn new(p: &'a Probe) -> Self {
        let now = p.active.fetch_add(1, Ordering::SeqCst) + 1;
        p.peak.fetch_max(now, Ordering::SeqCst);
        Self(p)
    }
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct WorkParams {
    /// Connection id.
    connection_id: String,
    /// Member name.
    #[serde(default)]
    member: Option<String>,
    /// Size of the filler string in the response, bytes.
    #[serde(default)]
    size: Option<usize>,
    /// SQL text, kept whole in the audit log.
    #[serde(default)]
    sql: Option<String>,
    /// Any long text, cut in the audit log.
    #[serde(default)]
    note: Option<String>,
    /// Answer with a JSON array instead of an object.
    #[serde(default)]
    array: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct WaitParams {
    /// How long to wait, milliseconds.
    #[serde(default)]
    millis: Option<u64>,
}

#[derive(Clone)]
struct ToyServer {
    ctx: SessionCtx,
    probe: Arc<Probe>,
    #[allow(dead_code)] // поле читает код макроса tool_handler
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl ToyServer {
    #[tool(
        description = "Toy database operation under the concurrency slots",
        annotations(read_only_hint = true)
    )]
    async fn work(
        &self,
        Parameters(p): Parameters<WorkParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "work",
            args: serde_json::to_value(&p).unwrap_or(Value::Null),
            connection_id: Some(&p.connection_id),
            member: p.member.as_deref(),
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let _slots = self.ctx.db_slots().await?;
                let _busy = Busy::new(&self.probe);
                let delay = self.probe.delay_ms.load(Ordering::SeqCst);
                if delay > 0 {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                if p.connection_id == "broken" {
                    return Err(ToolError::new(
                        "NODE_UNAVAILABLE",
                        "Node pg-2 is not reachable",
                    ));
                }
                if p.array == Some(true) {
                    return Ok(CallOk::new(json!([1, 2])));
                }
                let filler = "x".repeat(p.size.unwrap_or(0));
                Ok(CallOk::new(json!({ "ok": true, "filler": filler })).with_extra("masked", 3))
            })
            .await)
    }

    #[tool(
        description = "Toy long operation: waits for millis or until cancelled",
        annotations(read_only_hint = true)
    )]
    async fn long_wait(
        &self,
        Parameters(p): Parameters<WaitParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "long_wait",
            args: serde_json::to_value(&p).unwrap_or(Value::Null),
            connection_id: None,
            member: None,
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let _long = self
                    .ctx
                    .begin_long("Another long operation is running in this session")?;
                tokio::select! {
                    _ = ctx.ct.cancelled() => self.probe.long_stopped.store(true, Ordering::SeqCst),
                    _ = tokio::time::sleep(Duration::from_millis(p.millis.unwrap_or(5_000))) => {}
                }
                Ok(CallOk::new(json!({ "done": true })))
            })
            .await)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ToyServer {
    fn get_info(&self) -> ServerConfig {
        let o = self.ctx.options();
        let mut c = ServerConfig::new(ServerCapabilities::builder().enable_tools().build());
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
}

struct ToyFactory {
    probe: Arc<Probe>,
}

impl SessionFactory for ToyFactory {
    type Server = ToyServer;

    fn create(&self, ctx: SessionCtx) -> ToyServer {
        ToyServer {
            ctx,
            probe: self.probe.clone(),
            tool_router: ToyServer::tool_router(),
        }
    }
}

fn options() -> GatewayOptions {
    GatewayOptions::new("glafira-pg", "0.6.0", INSTRUCTIONS)
}

// ---- Окружение и клиент ----

struct Env {
    _dir: tempfile::TempDir,
    ep: Endpoint,
    handle: McpServerHandle,
    audit: Arc<AuditLog>,
    gate: Arc<FakeGate>,
    probe: Arc<Probe>,
}

async fn env(access: SessionAccess) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let ep = Endpoint::for_app(dir.path(), "glafira-pg-test");
    let audit = Arc::new(AuditLog::new(dir.path()));
    let gate = FakeGate::new(access);
    let probe = Arc::new(Probe::default());
    let factory = Arc::new(ToyFactory {
        probe: probe.clone(),
    });
    let handle = glafira_mcp_core::start(
        &ep,
        TOKEN.into(),
        gate.clone(),
        audit.clone(),
        options(),
        factory,
    )
    .await
    .unwrap();
    Env {
        _dir: dir,
        ep,
        handle,
        audit,
        gate,
        probe,
    }
}

async fn client(ep: &Endpoint) -> RunningService<RoleClient, ()> {
    let mut s = endpoint::connect(ep).await.unwrap();
    handshake::client(&mut s, TOKEN).await.unwrap();
    ().serve(s).await.unwrap()
}

/// Клиент с заданными именем и версией в `initialize`.
async fn named_client(
    ep: &Endpoint,
    name: &str,
    version: &str,
) -> RunningService<RoleClient, ClientConfig> {
    let mut s = endpoint::connect(ep).await.unwrap();
    handshake::client(&mut s, TOKEN).await.unwrap();
    ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new(name, version),
    )
    .serve(s)
    .await
    .unwrap()
}

async fn call(c: &RunningService<RoleClient, ()>, tool: &str, args: Value) -> CallToolResult {
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

fn ok(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(false), "{}", text(&r));
    r.structured_content.unwrap()
}

fn err(r: &CallToolResult) -> String {
    assert_eq!(r.is_error, Some(true), "{}", text(r));
    text(r)
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

// ---- Право и опознание клиента ----

#[tokio::test]
async fn pending_client_gets_a_hint_then_works_after_approval() {
    let e = env(SessionAccess::Pending).await;
    let c = client(&e.ep).await;
    let first = e.gate.seen.lock().unwrap()[0].clone();
    assert_eq!(
        first.client.as_ref().map(|c| c.name.as_str()),
        Some("rmcp"),
        "подтверждение запрошено при initialize, клиент — из clientInfo"
    );
    let msg = err(&call(&c, "work", json!({"connectionId": "c1"})).await);
    assert_eq!(
        msg,
        "client_not_approved: The user has not approved this MCP client in Glafira yet. Ask them to approve it in the Glafira window, then retry."
    );
    *e.gate.access.lock().unwrap() = SessionAccess::Approved;
    assert_eq!(
        ok(call(&c, "work", json!({"connectionId": "c1"})).await)["ok"],
        true
    );
    let sessions = e.handle.sessions();
    assert_eq!(
        (sessions.len(), sessions[0].calls),
        (1, 1),
        "отклонённый вызов не считается"
    );
    assert_eq!(sessions[0].client_name.as_deref(), Some("rmcp"));
}

#[tokio::test]
async fn rejected_client_is_disconnected() {
    let e = env(SessionAccess::Rejected).await;
    let c = client(&e.ep).await;
    let msg = err(&call(&c, "work", json!({"connectionId": "c1"})).await);
    assert_eq!(
        msg,
        "client_rejected: The user rejected this MCP client in Glafira"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(e.handle.sessions().is_empty());
    assert_eq!(e.gate.closed.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn disabled_gateway_refuses_calls_but_keeps_the_session() {
    let e = env(SessionAccess::Disabled).await;
    let c = client(&e.ep).await;
    let msg = err(&call(&c, "work", json!({"connectionId": "c1"})).await);
    assert_eq!(msg, "mcp_disabled: MCP is disabled in Glafira");
    assert_eq!(e.handle.sessions().len(), 1);
    assert_eq!(e.audit.tail(1)[0]["code"], "mcp_disabled");
}

#[tokio::test]
async fn client_name_is_trimmed_and_capped() {
    let e = env(SessionAccess::Approved).await;
    let _blank = named_client(&e.ep, " \t ", "1.0").await;
    let _long = named_client(&e.ep, &format!("  {}", "ж".repeat(300)), &"v".repeat(300)).await;
    let seen = e.gate.seen.lock().unwrap().clone();
    assert_eq!(seen[0].client, None, "пустое имя — как отсутствующее");
    let c = seen.last().unwrap().client.clone().unwrap();
    assert_eq!(c.name, "ж".repeat(128));
    assert_eq!(c.version, "v".repeat(128));
    let mut names: Vec<Option<String>> = e
        .handle
        .sessions()
        .into_iter()
        .map(|s| s.client_name)
        .collect();
    names.sort();
    assert_eq!(names, [None, Some("ж".repeat(128))]);
}

#[tokio::test]
async fn client_without_initialize_is_identified_on_the_tool_list() {
    let gate = FakeGate::new(SessionAccess::Pending);
    let dir = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(dir.path()));
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let server = tokio::spawn(glafira_mcp_core::serve_stream(
        Box::new(server_io),
        gate.clone(),
        audit.clone(),
        options(),
        Arc::new(ToyFactory {
            probe: Arc::new(Probe::default()),
        }),
    ));
    let (rd, mut wr) = tokio::io::split(client_io);
    let mut rd = BufReader::new(rd);
    // Протокол 2026-07-28: без initialize, клиент — в _meta каждого запроса.
    let list = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {"_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "claude-code", "version": "2.1.310"},
        }},
    });
    wr.write_all(format!("{list}\n").as_bytes()).await.unwrap();
    wr.flush().await.unwrap();
    let r = reply(&mut rd, 1).await;
    let mut names: Vec<&str> = r["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["long_wait", "work"]);
    assert_eq!(r["result"]["ttlMs"], 0, "подсказки кеша для 2026-07-28");
    assert_eq!(r["result"]["cacheScope"], "public");
    let seen = gate.seen.lock().unwrap().clone();
    assert_eq!(
        seen.first()
            .and_then(|v| v.client.as_ref())
            .map(|c| (c.name.as_str(), c.version.as_str())),
        Some(("claude-code", "2.1.310")),
        "подтверждение запрошено при списке инструментов, с клиентом из _meta"
    );
    drop(wr);
    drop(rd);
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("serve_stream возвращается, когда клиент закрыл поток")
        .unwrap();
    assert_eq!(gate.closed.lock().unwrap().len(), 1);
    let events: Vec<String> = audit
        .tail(5)
        .iter()
        .map(|r| r["event"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(events, ["sessionEnded", "sessionStarted"]);
}

#[tokio::test]
async fn legacy_tool_list_has_no_cache_hints() {
    let gate = FakeGate::new(SessionAccess::Approved);
    let dir = tempfile::tempdir().unwrap();
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    tokio::spawn(glafira_mcp_core::serve_stream(
        Box::new(server_io),
        gate.clone(),
        Arc::new(AuditLog::new(dir.path())),
        options(),
        Arc::new(ToyFactory {
            probe: Arc::new(Probe::default()),
        }),
    ));
    let (rd, mut wr) = tokio::io::split(client_io);
    let mut rd = BufReader::new(rd);
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "qwen-code", "version": "0.26.7"},
        },
    });
    wr.write_all(format!("{init}\n").as_bytes()).await.unwrap();
    let r = reply(&mut rd, 1).await;
    assert_eq!(r["result"]["serverInfo"]["name"], "glafira-pg");
    assert_eq!(r["result"]["instructions"], INSTRUCTIONS);
    let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    wr.write_all(format!("{initialized}\n{list}\n").as_bytes())
        .await
        .unwrap();
    let r = reply(&mut rd, 2).await;
    assert_eq!(r["result"]["tools"].as_array().unwrap().len(), 2);
    assert!(r["result"].get("ttlMs").is_none(), "{r}");
    assert!(r["result"].get("cacheScope").is_none(), "{r}");
    assert_eq!(
        gate.seen.lock().unwrap()[0]
            .client
            .as_ref()
            .map(|c| c.name.clone()),
        Some("qwen-code".to_string())
    );
}

// ---- Лимиты ----

#[tokio::test]
async fn calls_are_rate_limited_and_audited() {
    let e = env(SessionAccess::Approved).await;
    let c = client(&e.ep).await;
    for _ in 0..60 {
        ok(call(&c, "work", json!({"connectionId": "c1"})).await);
    }
    let msg = err(&call(&c, "work", json!({"connectionId": "c1"})).await);
    assert_eq!(
        msg,
        "rate_limited: Too many calls: at most 60 per minute; retry later"
    );
    let other = client(&e.ep).await;
    ok(call(
        &other,
        "work",
        json!({"connectionId": "c1", "member": "pg-1"}),
    )
    .await);
    let recent = e.audit.tail(3);
    assert_eq!(recent[0]["event"], "call");
    assert_eq!(recent[0]["tool"], "work");
    assert_eq!(recent[0]["connection"], json!({"id": "c1", "name": "lab"}));
    assert_eq!(recent[0]["member"], "pg-1");
    assert_eq!(
        recent[0]["masked"], 3,
        "счётчик приложения из CallOk::extra"
    );
    assert!(
        recent[0]["client"].as_str().unwrap().starts_with("rmcp "),
        "клиент — «имя версия»: {}",
        recent[0]
    );
    assert!(
        recent.iter().any(|r| r["code"] == "rate_limited"),
        "{recent:?}"
    );
}

#[tokio::test]
async fn db_operations_are_limited_per_session_and_globally() {
    let e = env(SessionAccess::Approved).await;
    e.probe.delay_ms.store(150, Ordering::SeqCst);
    let work = |c: Arc<RunningService<RoleClient, ()>>| {
        tokio::spawn(async move { ok(call(&c, "work", json!({"connectionId": "c1"})).await) })
    };
    let one = Arc::new(client(&e.ep).await);
    let calls: Vec<_> = (0..5).map(|_| work(one.clone())).collect();
    for h in calls {
        h.await.unwrap();
    }
    assert_eq!(
        e.probe.peak.load(Ordering::SeqCst),
        2,
        "не больше 2 на сессию"
    );

    e.probe.peak.store(0, Ordering::SeqCst);
    let mut clients = vec![one];
    for _ in 0..2 {
        clients.push(Arc::new(client(&e.ep).await));
    }
    let calls: Vec<_> = clients
        .iter()
        .flat_map(|c| (0..3).map(|_| work(c.clone())).collect::<Vec<_>>())
        .collect();
    for h in calls {
        h.await.unwrap();
    }
    assert_eq!(
        e.probe.peak.load(Ordering::SeqCst),
        4,
        "не больше 4 на все сессии"
    );
}

#[tokio::test]
async fn one_long_operation_per_session_and_cancel_frees_it() {
    let e = env(SessionAccess::Approved).await;
    let c = client(&e.ep).await;
    let Value::Object(args) = json!({"millis": 5_000}) else {
        unreachable!()
    };
    let request = ClientRequest::CallToolRequest(CallToolRequest::new(
        CallToolRequestParams::new("long_wait").with_arguments(args),
    ));
    let handle = c
        .send_cancellable_request(request, PeerRequestOptions::no_options())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let msg = err(&call(&c, "long_wait", json!({"millis": 10})).await);
    assert_eq!(
        msg,
        "busy: Another long operation is running in this session"
    );
    handle.cancel(Some("test".into())).await.unwrap();
    let mut logged = None;
    for _ in 0..100 {
        logged = e
            .audit
            .tail(5)
            .into_iter()
            .find(|r| r["tool"] == "long_wait" && r["outcome"] == "ok");
        if logged.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let logged = logged.expect("операция закончилась после отмены");
    assert!(logged["ms"].as_u64().unwrap() < 1_000, "{logged}");
    assert!(e.probe.long_stopped.load(Ordering::SeqCst));
    // Отметка «идёт длительная операция» снята.
    ok(call(&c, "long_wait", json!({"millis": 10})).await);
}

#[tokio::test]
async fn too_large_response_is_refused() {
    let e = env(SessionAccess::Approved).await;
    let c = client(&e.ep).await;
    ok(call(&c, "work", json!({"connectionId": "c1", "size": 90_000})).await);
    let msg = err(&call(&c, "work", json!({"connectionId": "c1", "size": 150_000})).await);
    assert!(msg.starts_with("response_too_large: "), "{msg}");
    let last = e.audit.tail(1).remove(0);
    assert_eq!(
        (last["code"].as_str(), last["bytes"].as_u64()),
        (Some("response_too_large"), Some(0))
    );
}

#[tokio::test]
async fn non_object_response_is_an_internal_error() {
    let e = env(SessionAccess::Approved).await;
    let c = client(&e.ep).await;
    let msg = err(&call(&c, "work", json!({"connectionId": "c1", "array": true})).await);
    assert_eq!(msg, "internal: The tool response is not a JSON object");
}

#[tokio::test]
async fn tool_errors_reach_the_llm_as_code_and_message() {
    let e = env(SessionAccess::Approved).await;
    let c = client(&e.ep).await;
    let msg = err(&call(&c, "work", json!({"connectionId": "broken"})).await);
    assert_eq!(msg, "NODE_UNAVAILABLE: Node pg-2 is not reachable");
    let last = e.audit.tail(1).remove(0);
    assert_eq!(last["code"], "NODE_UNAVAILABLE");
    assert_eq!(last["outcome"], "error");
    assert_eq!(
        last["connection"],
        json!({"id": "broken", "name": ""}),
        "имени нет — пусто"
    );
    assert!(
        last.get("masked").is_none(),
        "у ошибки нет счётчиков приложения"
    );
}

#[tokio::test]
async fn audit_keeps_sql_whole_and_cuts_the_rest() {
    let e = env(SessionAccess::Approved).await;
    let c = client(&e.ep).await;
    let sql = format!("SELECT {}", "x".repeat(10_000));
    ok(call(
        &c,
        "work",
        json!({"connectionId": "c1", "sql": sql, "note": "n".repeat(2_000)}),
    )
    .await);
    let r = e.audit.tail(1).remove(0);
    assert_eq!(r["args"]["sql"], sql);
    assert_eq!(r["args"]["_truncated"], true);
    assert!(r["args"]["_rest"].as_str().unwrap().ends_with('…'));
    assert_eq!(
        r["connection"]["id"], "c1",
        "подключение — из CallMeta, не из аргументов"
    );
}

// ---- Сессии и остановка ----

#[tokio::test]
async fn sessions_show_active_calls() {
    let e = env(SessionAccess::Approved).await;
    e.probe.delay_ms.store(300, Ordering::SeqCst);
    let c = Arc::new(client(&e.ep).await);
    let h = {
        let c = c.clone();
        tokio::spawn(async move { call(&c, "work", json!({"connectionId": "c1"})).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let s = e.handle.sessions();
    assert_eq!((s.len(), s[0].active, s[0].calls), (1, true, 1));
    ok(h.await.unwrap());
    assert!(!e.handle.sessions()[0].active);
    assert!(
        e.gate.changes.load(Ordering::SeqCst) >= 3,
        "начало сессии, опознание клиента, начало и конец вызова"
    );
    let snap = serde_json::to_value(&e.handle.sessions()[0]).unwrap();
    assert!(
        snap.get("clientName").is_some() && snap.get("startedMs").is_some(),
        "{snap}"
    );
}

#[tokio::test]
async fn disconnect_and_shutdown_close_sessions() {
    let e = env(SessionAccess::Approved).await;
    let c = client(&e.ep).await;
    ok(call(&c, "work", json!({"connectionId": "c1"})).await);
    let id = e.handle.sessions()[0].id.clone();
    assert!(e.handle.disconnect(&id));
    assert!(!e.handle.disconnect("nope"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(e.handle.sessions().is_empty());
    assert_eq!(
        e.gate.closed.lock().unwrap().as_slice(),
        std::slice::from_ref(&id)
    );
    let ended = e
        .audit
        .tail(10)
        .into_iter()
        .find(|r| r["event"] == "sessionEnded")
        .unwrap();
    assert_eq!(
        (ended["session"].as_str(), ended["calls"].as_u64()),
        (Some(id.as_str()), Some(1))
    );

    let _a = client(&e.ep).await;
    let _b = named_client(&e.ep, "cursor", "1.0").await;
    assert_eq!(e.handle.disconnect_client("rmcp"), 1);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(e.handle.sessions().len(), 1);
    e.handle.disconnect_all();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(e.handle.sessions().is_empty());

    let c2 = client(&e.ep).await;
    e.handle.shutdown().await;
    assert!(
        c2.call_tool(CallToolRequestParams::new("work"))
            .await
            .is_err()
    );
    assert!(
        endpoint::connect(&e.ep).await.is_err(),
        "сервер больше не слушает"
    );
    let last = e.audit.tail(1).remove(0);
    assert_eq!(last["event"], "serverStopped");
}

#[tokio::test]
async fn shutdown_does_not_wait_for_a_silent_handshake() {
    let e = env(SessionAccess::Approved).await;
    // Соединение без строки рукопожатия: сервер ждал бы её до 5 с.
    let _silent = endpoint::connect(&e.ep).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let started = std::time::Instant::now();
    e.handle.shutdown().await;
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
}

#[tokio::test]
async fn wrong_token_is_rejected_before_mcp() {
    let e = env(SessionAccess::Approved).await;
    let mut s = endpoint::connect(&e.ep).await.unwrap();
    let r = handshake::client(&mut s, "wrong").await;
    assert!(matches!(
        r,
        Err(handshake::HandshakeError::Rejected { ref code, .. }) if code == "bad_token"
    ));
    assert!(e.handle.sessions().is_empty(), "без рукопожатия сессии нет");
    let started = e.audit.tail(5);
    assert_eq!(started.last().unwrap()["event"], "serverStarted");
    assert!(started.iter().all(|r| r["event"] != "sessionStarted"));
}
