//! `AppBackend` — реализация `Gate` и `PgBackend` приложения (спец. P4 §3.2, §4.3): право сессии по политике,
//! выключателю и подтверждению клиента; данные инструментов — существующие функции приложения.

use std::sync::Arc;

use glafira_mcp_core::error::{BoxFuture, ToolError};
use glafira_mcp_core::gate::{Gate, SessionAccess, SessionView};
use glafira_pg_mcp::model::{ObserveCtx, PgAnswer, PgBackend, PgObserve, PgRead};

use super::approvals::Approvals;
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
}

impl Gate for AppBackend {
    fn access(&self, s: &SessionView) -> SessionAccess {
        if !self.mcp_on() {
            return SessionAccess::Disabled;
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
    }

    fn sessions_changed(&self) {
        self.st.mcp.emit_state();
    }
}

/// Ответ инструмента, пока его данные не подключены к функциям приложения.
const NOT_CONNECTED: &str = "not implemented yet";

impl PgBackend for AppBackend {
    fn read(&self, _op: PgRead) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        Box::pin(async { Err(ToolError::new("internal", NOT_CONNECTED)) })
    }

    fn observe(
        &self,
        _op: PgObserve,
        _ctx: ObserveCtx,
    ) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        Box::pin(async { Err(ToolError::new("internal", NOT_CONNECTED)) })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};

    use glafira_mcp_core::gate::ClientIdent;

    use super::*;
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

    #[test]
    fn connection_names_come_from_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let (b, _) = backend(dir.path(), true, true);
        b.st.connections.upsert(sample_profile("c1")).unwrap();
        assert_eq!(b.connection_name("c1").as_deref(), Some("conn-c1"));
        assert_eq!(b.connection_name("nope"), None);
    }

    #[tokio::test]
    async fn data_operations_answer_internal_until_tools_are_connected() {
        let dir = tempfile::tempdir().unwrap();
        let (b, _) = backend(dir.path(), true, true);
        let e = b.read(PgRead::ListConnections).await.unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            ("internal", "not implemented yet")
        );
    }
}
