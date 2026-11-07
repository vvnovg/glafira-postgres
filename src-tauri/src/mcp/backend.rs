//! `AppBackend` — реализация `Gate` и `PgBackend` приложения (спец. P4 §3.2, §4.3): право сессии по политике,
//! выключателю и подтверждению клиента; данные инструментов — существующие функции приложения.
//! Сессия фонового запуска (P4c §5) — подтверждение билетом, девять инструментов, подключение и запрос запуска
//! (`agent::tickets`).

use std::sync::Arc;

use glafira_mcp_core::audit::AuditEvent;
use glafira_mcp_core::error::{BoxFuture, ToolError};
use glafira_mcp_core::gate::{Gate, SessionAccess, SessionView};
use glafira_pg_mcp::model::{LlmOp, ObserveCtx, PgAnswer, PgBackend, PgObserve, PgRead, SelectOp};
use glafira_pg_mcp::server::RUN_TOOL_NAMES;
use glafira_pg_mcp::server::glossary::UiGlossary;

use super::approvals::Approvals;
use crate::agent::tickets::{Attach, not_in_run};
use crate::state::AppState;
use crate::topology::model::now_ms;

pub struct AppBackend {
    pub(crate) st: AppState,
    pub(crate) approvals: Arc<Approvals>,
}

impl AppBackend {
    pub fn new(st: AppState, approvals: Arc<Approvals>) -> Self {
        Self { st, approvals }
    }

    /// MCP разрешён политикой и включён в «Настройках» (спец. §4.3, шаг 1). Выключение останавливает сервер, но
    /// вызов, пришедший между записью настройки и остановкой, тоже отказывает.
    fn mcp_on(&self) -> bool {
        self.st.policy.status.mcp_allowed() && self.st.settings.mcp().enabled
    }

    /// Сессия фонового запуска (P4c §5.1): билет уже проверен в рукопожатии. Поэтому подтверждение — без диалога и без
    /// записи клиента в `approvedClients` (`Approvals` её не видит); в журнале — `approved` с `run` и `by: "run"`, один
    /// раз на сессию. Живая сессия на запуск одна: новая сменяет прежнюю, прежняя закрывается и назад не
    /// возвращается. Запуск закончился — отказ, сессия закрывается.
    fn run_access(&self, s: &SessionView, run: &str) -> SessionAccess {
        match self.st.tickets.attach_session(run, &s.id) {
            Attach::Gone => SessionAccess::Rejected,
            Attach::Known => SessionAccess::Approved,
            Attach::New { replaced } => {
                if let Some(old) = replaced {
                    self.st.mcp.disconnect(Some(&old));
                }
                self.st.mcp.record_audit(AuditEvent::Approved {
                    session: s.id.clone(),
                    client: s.client.as_ref().map(|c| c.name.clone()),
                    run: Some(run.to_string()),
                    by: Some("run".to_string()),
                });
                SessionAccess::Approved
            }
        }
    }
}

impl Gate for AppBackend {
    fn access(&self, s: &SessionView) -> SessionAccess {
        if !self.mcp_on() {
            return SessionAccess::Disabled;
        }
        if let Some(run) = s.run.as_deref() {
            return self.run_access(s, run);
        }
        let (access, new_pending) = self.approvals.access(s, now_ms());
        if new_pending {
            // Диалог подтверждения: окно привлекает внимание, интерфейс получает ожидающий запрос.
            self.st.mcp.request_attention();
            self.st.mcp.emit_state();
        }
        access
    }

    fn connection_name(&self, id: &str) -> Option<String> {
        self.st.connections.get(id).ok().map(|p| p.name)
    }

    fn session_closed(&self, session_id: &str) {
        self.approvals.closed(session_id);
        self.st.tickets.session_closed(session_id);
    }

    fn sessions_changed(&self) {
        self.st.mcp.emit_state();
    }

    /// Билет сравнивается за постоянное время (`Tickets::resolve`).
    fn run_for_ticket(&self, ticket: &str) -> Option<String> {
        self.st.tickets.resolve(ticket)
    }

    /// Сессия запуска (P4c §5.2). Вызов считается всегда (`run_limit` после 40), отклонённый тоже. Затем
    /// проверяются инструмент из `RUN_TOOL_NAMES` и подключение запуска (без краевых пробелов: так его сверяет с
    /// запросом `propose_query_rewrite`, M6). Запрос проверяют сами операции (`mcp::llm::tools`, `list_connections`).
    fn check_call(
        &self,
        s: &SessionView,
        tool: &str,
        connection_id: Option<&str>,
    ) -> Result<(), ToolError> {
        let Some(scope) = self.st.tickets.scope_of(s.run.as_deref())? else {
            return Ok(());
        };
        self.st.tickets.note_call(&scope.run_id, tool)?;
        if !RUN_TOOL_NAMES.contains(&tool) {
            return Err(not_in_run());
        }
        if let Some(c) = connection_id
            && scope.connection_id.as_deref() != Some(c.trim())
        {
            return Err(not_in_run());
        }
        Ok(())
    }

    fn tool_visible(&self, s: &SessionView, tool: &str) -> bool {
        s.run.is_none() || RUN_TOOL_NAMES.contains(&tool)
    }

    fn prompts_visible(&self, s: &SessionView) -> bool {
        s.run.is_none()
    }
}

impl PgBackend for AppBackend {
    fn read(&self, op: PgRead) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        Box::pin(super::read::read_op(&self.st, op))
    }

    fn observe(
        &self,
        op: PgObserve,
        ctx: ObserveCtx,
    ) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        Box::pin(super::read::observe_op(&self.st, op, ctx))
    }

    /// `run_select` (P4b, §7.4) — `mcp::select`.
    fn select(&self, op: SelectOp, ctx: ObserveCtx) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        Box::pin(super::select::run_select(&self.st, op.0, ctx))
    }

    /// Запросы на оптимизацию и предложения (P4b, §8.2) — `mcp::llm::tools`.
    fn llm(&self, op: LlmOp) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        Box::pin(super::llm::tools::handle(&self.st, op))
    }

    /// Подписи интерфейса для `instructions` и prompts (смоук 2026-11-07, R8) — по языку интерфейса сейчас.
    fn ui_glossary(&self) -> Option<UiGlossary> {
        super::glossary::glossary(&crate::settings::ui_language(
            &self.st.settings,
            &self.st.policy.status,
        ))
    }
}

/// Отчёт инструмента для модели (`mcp::read::report_out`): имя из плана P4a.
pub use super::read::report_out;

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};

    use glafira_mcp_core::gate::ClientIdent;
    use glafira_pg_mcp::model::PgOutput;
    use glafira_pg_mcp::server::TOOL_NAMES;
    use serde_json::Value;

    use super::*;
    use crate::agent::tickets::{MAX_RUN_CALLS, RunObserver, RunScope, not_in_run};
    use crate::conn::profile::sample_profile;
    use crate::mcp::{McpStatus, McpUi};
    use crate::secrets::MemoryStore;

    #[derive(Default)]
    struct Attention(AtomicU32);

    impl McpUi for Arc<Attention> {
        fn status_changed(&self, _: &McpStatus) {}
        fn request_attention(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn backend(dir: &Path, policy_allows: bool, enabled: bool) -> (AppBackend, Arc<Attention>) {
        let mut policy = crate::policy::Policy::default();
        policy.status.mcp_enabled = policy_allows;
        let st = AppState::new(dir, Arc::new(MemoryStore::default()))
            .unwrap()
            .with_policy(policy);
        st.settings.set_mcp_enabled(enabled).unwrap();
        let rec = Arc::new(Attention::default());
        st.mcp.set_ui(Box::new(rec.clone()));
        let approvals = Arc::new(Approvals::new(st.settings.clone()));
        (AppBackend::new(st, approvals), rec)
    }

    fn view(id: &str, name: &str) -> SessionView {
        SessionView {
            id: id.into(),
            client: Some(ClientIdent {
                name: name.into(),
                version: "0.26.7".into(),
            }),
            run: None,
        }
    }

    #[test]
    fn mcp_off_by_policy_or_setting_disables_every_session() {
        for (policy, enabled) in [(false, true), (true, false), (false, false)] {
            let dir = tempfile::tempdir().unwrap();
            let (b, rec) = backend(dir.path(), policy, enabled);
            assert_eq!(
                b.access(&view("s1", "qwen-code")),
                SessionAccess::Disabled,
                "политика {policy}, настройка {enabled}"
            );
            assert_eq!(rec.0.load(Ordering::SeqCst), 0, "диалога нет");
            assert!(b.approvals.pending().is_empty());
        }
    }

    /// Смоук 2026-11-07, R8: подписи интерфейса для текстов модели — по действующему языку интерфейса (политика →
    /// выбор → ОС) на момент вопроса; у английского интерфейса их нет.
    #[test]
    fn the_glossary_follows_the_language_of_the_interface() {
        let dir = tempfile::tempdir().unwrap();
        let (b, _) = backend(dir.path(), true, true);
        b.st.settings.set_language(Some("ru")).unwrap();
        assert_eq!(b.ui_glossary(), crate::mcp::glossary::glossary("ru"));
        assert!(b.ui_glossary().is_some());
        b.st.settings.set_language(Some("en")).unwrap();
        assert_eq!(b.ui_glossary(), None);
        let mut policy = crate::policy::Policy::default();
        policy.status.mcp_enabled = true;
        policy.status.language = Some("ru".into());
        let st = b.st.clone().with_policy(policy);
        let b = AppBackend::new(st, b.approvals.clone());
        assert!(b.ui_glossary().is_some(), "язык политики важнее выбора");
    }

    #[test]
    fn new_client_asks_once_and_is_let_in_after_approval() {
        let dir = tempfile::tempdir().unwrap();
        let (b, rec) = backend(dir.path(), true, true);
        let v = view("s1", "qwen-code");
        assert_eq!(b.access(&v), SessionAccess::Pending);
        assert_eq!(b.access(&v), SessionAccess::Pending);
        assert_eq!(
            rec.0.load(Ordering::SeqCst),
            1,
            "внимание — один раз на сессию"
        );
        b.approvals.answer("s1", true, 1).unwrap();
        assert_eq!(b.access(&v), SessionAccess::Approved);
        b.session_closed("s1");
        assert!(!b.approvals.is_approved("s1"), "закрытая сессия забыта");
        // Новая сессия того же клиента — без вопроса.
        assert_eq!(b.access(&view("s2", "qwen-code")), SessionAccess::Approved);
        assert_eq!(rec.0.load(Ordering::SeqCst), 1);
    }

    fn scope(run: &str) -> RunScope {
        RunScope {
            run_id: run.into(),
            connection_id: Some("c1".into()),
            request_id: Some("r1".into()),
        }
    }

    fn run_view(id: &str, run: &str) -> SessionView {
        SessionView {
            run: Some(run.into()),
            ..view(id, "qwen-code")
        }
    }

    fn approved_records(b: &AppBackend) -> Vec<Value> {
        b.st.mcp
            .audit_tail(50)
            .into_iter()
            .filter(|e| e["event"] == "approved")
            .collect()
    }

    #[test]
    fn run_session_is_approved_by_its_ticket_without_the_dialog() {
        let dir = tempfile::tempdir().unwrap();
        let (b, rec) = backend(dir.path(), true, true);
        let ticket = b.st.tickets.issue(scope("a1"));
        assert_eq!(b.run_for_ticket(&ticket).as_deref(), Some("a1"));
        assert_eq!(b.run_for_ticket(&"0".repeat(64)), None);
        let v = run_view("s1", "a1");
        assert_eq!(b.access(&v), SessionAccess::Approved);
        assert_eq!(b.access(&v), SessionAccess::Approved);
        assert_eq!(rec.0.load(Ordering::SeqCst), 0, "диалога нет");
        assert!(b.approvals.pending().is_empty());
        assert!(!b.approvals.is_approved("s1"), "решение — не в Approvals");
        assert!(
            b.st.settings.mcp().approved_clients.is_empty(),
            "клиент не запомнен"
        );
        let approved = approved_records(&b);
        assert_eq!(approved.len(), 1, "approved — один раз на сессию");
        assert_eq!(
            (
                approved[0]["session"].as_str(),
                approved[0]["client"].as_str(),
                approved[0]["run"].as_str(),
                approved[0]["by"].as_str()
            ),
            (Some("s1"), Some("qwen-code"), Some("a1"), Some("run"))
        );
        // Обычная сессия того же клиента — с диалогом, как прежде.
        assert_eq!(b.access(&view("s2", "qwen-code")), SessionAccess::Pending);
        // Запуск закончился: билет не действует, сессия запуска закрывается.
        b.st.tickets.revoke("a1");
        assert_eq!(b.run_for_ticket(&ticket), None);
        assert_eq!(b.access(&v), SessionAccess::Rejected);
    }

    #[test]
    fn mcp_off_disables_run_sessions_too() {
        let dir = tempfile::tempdir().unwrap();
        let (b, _) = backend(dir.path(), false, true);
        b.st.tickets.issue(scope("a1"));
        assert_eq!(b.access(&run_view("s1", "a1")), SessionAccess::Disabled);
        assert!(approved_records(&b).is_empty());
    }

    #[test]
    fn run_sees_nine_tools_no_prompts_and_its_connection_only() {
        let dir = tempfile::tempdir().unwrap();
        let (b, _) = backend(dir.path(), true, true);
        b.st.tickets.issue(scope("a1"));
        let v = run_view("s1", "a1");
        let chat = view("s2", "qwen-code");
        assert_eq!(
            TOOL_NAMES.iter().filter(|t| b.tool_visible(&v, t)).count(),
            9
        );
        assert_eq!(
            TOOL_NAMES
                .iter()
                .filter(|t| b.tool_visible(&chat, t))
                .count(),
            18
        );
        assert!(!b.prompts_visible(&v) && b.prompts_visible(&chat));
        assert_eq!(b.check_call(&v, "explain_query", Some("c1")), Ok(()));
        assert_eq!(b.check_call(&v, "get_optimization_request", None), Ok(()));
        // Сверка с feat/p4b (M6): requestId своего запуска рядом с connectionId своего подключения — не отказ области.
        assert_eq!(
            b.check_call(&v, "propose_query_rewrite", Some("c1")),
            Ok(())
        );
        assert_eq!(
            b.check_call(&v, "propose_query_rewrite", Some(" c1 ")),
            Ok(()),
            "краевые пробелы — как в сверке с запросом"
        );
        assert_eq!(
            b.check_call(&v, "explain_query", Some("c2")),
            Err(not_in_run())
        );
        let e = b.check_call(&v, "run_select", Some("c1")).unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            ("not_allowed", "Not available to a background run")
        );
        assert_eq!(
            b.check_call(&chat, "run_select", Some("c2")),
            Ok(()),
            "обычная сессия — без области запуска"
        );
        // «Проверить»: запуск без подключения — инструменты с подключением вне области.
        b.st.tickets.issue(RunScope {
            run_id: "a2".into(),
            connection_id: None,
            request_id: None,
        });
        let check = run_view("s3", "a2");
        assert_eq!(b.check_call(&check, "list_connections", None), Ok(()));
        assert_eq!(
            b.check_call(&check, "explain_query", Some("c1")),
            Err(not_in_run())
        );
    }

    #[test]
    fn forty_calls_per_run_then_run_limit() {
        let dir = tempfile::tempdir().unwrap();
        let (b, _) = backend(dir.path(), true, true);
        b.st.tickets.issue(scope("a1"));
        let v = run_view("s1", "a1");
        // Отклонённые вызовы — тоже в счёте.
        for _ in 0..3 {
            assert_eq!(b.check_call(&v, "run_select", None), Err(not_in_run()));
        }
        for _ in 3..MAX_RUN_CALLS {
            b.check_call(&v, "explain_query", Some("c1")).unwrap();
        }
        let e = b.check_call(&v, "explain_query", Some("c1")).unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "run_limit",
                "The background run reached 40 tool calls: stop and write the summary"
            )
        );
        // Предел — на запуск, а не на сессию.
        assert_eq!(
            b.check_call(&run_view("s2", "a1"), "list_connections", None)
                .unwrap_err()
                .code,
            "run_limit"
        );
        b.st.tickets.revoke("a1");
        assert_eq!(
            b.check_call(&v, "list_connections", None)
                .unwrap_err()
                .message,
            "The background run is over"
        );
    }

    #[test]
    fn a_new_session_of_the_run_replaces_the_old_one() {
        #[derive(Default)]
        struct Sessions(std::sync::Mutex<Vec<(String, bool)>>);

        impl RunObserver for Sessions {
            fn run_session(&self, run_id: &str, connected: bool) {
                self.0.lock().unwrap().push((run_id.into(), connected));
            }
            fn run_call(&self, _run_id: &str, _tool: &str, _calls: u32) {}
        }

        let dir = tempfile::tempdir().unwrap();
        let (b, _) = backend(dir.path(), true, true);
        let seen = Arc::new(Sessions::default());
        b.st.tickets.set_observer(seen.clone());
        b.st.tickets.issue(scope("a1"));
        assert_eq!(b.access(&run_view("s1", "a1")), SessionAccess::Approved);
        assert_eq!(
            b.access(&run_view("s2", "a1")),
            SessionAccess::Approved,
            "агент перезапустил MCP-сервер"
        );
        // Вызов, который сменённая сессия успела послать до закрытия, новую не сменяет.
        assert_eq!(b.access(&run_view("s1", "a1")), SessionAccess::Rejected);
        assert_eq!(b.access(&run_view("s2", "a1")), SessionAccess::Approved);
        b.session_closed("s1");
        b.session_closed("s2");
        assert_eq!(
            *seen.0.lock().unwrap(),
            [
                ("a1".to_string(), true),
                ("a1".to_string(), true),
                ("a1".to_string(), false)
            ]
        );
        assert_eq!(approved_records(&b).len(), 2);
    }

    #[test]
    fn connection_names_come_from_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let (b, _) = backend(dir.path(), true, true);
        b.st.connections.upsert(sample_profile("c1")).unwrap();
        assert_eq!(b.connection_name("c1").as_deref(), Some("conn-c1"));
        assert_eq!(b.connection_name("nope"), None);
    }

    #[tokio::test]
    async fn data_operations_go_to_mcp_read() {
        let dir = tempfile::tempdir().unwrap();
        let (b, _) = backend(dir.path(), true, true);
        let a = b.read(PgRead::ListConnections { run: None }).await.unwrap();
        assert_eq!(a.body, PgOutput::Connections(vec![]));
        let e = b
            .read(PgRead::Topology(
                serde_json::from_value(serde_json::json!({ "connectionId": "nope" })).unwrap(),
            ))
            .await
            .unwrap_err();
        assert_eq!(e.code, "not_allowed");
        // Наблюдение — тоже через проверку доступа к подключению (`mcp::observe`).
        for op in [
            PgObserve::Activity(
                serde_json::from_value(serde_json::json!({ "connectionId": "nope" })).unwrap(),
            ),
            PgObserve::TopQueries(
                serde_json::from_value(serde_json::json!({ "connectionId": "nope" })).unwrap(),
            ),
        ] {
            let e = b
                .observe(op, ObserveCtx::new(std::time::Duration::from_secs(10)))
                .await
                .unwrap_err();
            assert_eq!(e.code, "not_allowed");
        }
        // `run_select` — через `mcp::select`: та же проверка доступа.
        let op = SelectOp(
            serde_json::from_value(
                serde_json::json!({ "connectionId": "nope", "sql": "SELECT 1" }),
            )
            .unwrap(),
        );
        let e = b
            .select(op, ObserveCtx::new(std::time::Duration::from_secs(10)))
            .await
            .unwrap_err();
        assert_eq!(e.code, "not_allowed");
    }
}
