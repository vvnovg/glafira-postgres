//! Жизненный цикл MCP-сервера (спец. P4 §3.3, §4.4, §11): запуск и остановка по настройке и политике, `mcp.json`,
//! «другой экземпляр Glafira», статус для интерфейса (`mcp://state`), ответы на подтверждение, отзыв клиентов.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock, Weak};

use glafira_mcp_core::audit::{AuditEvent, AuditLog};
use glafira_mcp_core::endpoint::{self, Endpoint};
use glafira_mcp_core::server::{GatewayOptions, McpServerHandle, SessionSnapshot};
use glafira_pg_mcp::server::{INSTRUCTIONS, PgFactory};
use serde::Serialize;
use serde_json::Value;

use super::approvals::{Approvals, PendingApproval};
use super::backend::AppBackend;
use super::files::{self, McpFile};
use crate::agent::queue::{AgentQueue, AgentStatus};
use crate::error::{AppResult, ErrorCode};
use crate::policy::{McpMaxAccess, Policy};
use crate::product::MCP_SERVER_NAME;
use crate::settings::{ApprovedClient, SettingsStore};
use crate::state::AppState;
use crate::storage::connections::ConnectionStore;
use crate::sync::{MutexExt, RwLockExt};
use crate::topology::model::now_ms;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum McpServerState {
    Stopped,
    Running,
    /// MCP обслуживает другой запущенный экземпляр Glafira (его `mcp.json` не трогаем).
    OtherInstance,
    Error,
}

/// Статус MCP для «Настроек», диалога подтверждения и индикатора — полезная нагрузка `mcp://state`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub server: McpServerState,
    /// Почему сервер не запустился (`server == error`), английский текст.
    pub error: Option<String>,
    /// Выключатель «Разрешить MCP-сервер» в `settings.json`.
    pub enabled: bool,
    pub sessions: Vec<SessionSnapshot>,
    pub pending: Vec<PendingApproval>,
    pub approved_clients: Vec<ApprovedClient>,
    /// Подключений с действующим уровнем доступа LLM, отличным от off (раздел MCP, текст диалога подтверждения).
    pub open_connections: u32,
    /// Из них — с действующим уровнем `data`.
    pub data_connections: u32,
    /// Политика разрешает MCP (`mcpEnabled`).
    pub policy_enabled: bool,
    /// Потолок уровня доступа по политике (`mcpMaxAccess`).
    pub max_access: McpMaxAccess,
    /// Фоновый ассистент (P4c §8.4): политика, выключатель, настроен ли, идущий запуск и очередь.
    pub agent: AgentStatus,
}

/// Связь с окном: событие статуса и привлечение внимания. В приложении — окно Tauri, в тестах — запись вызовов.
pub trait McpUi: Send + Sync {
    fn status_changed(&self, status: &McpStatus);
    fn request_attention(&self);
}

struct TauriUi(tauri::AppHandle);

impl McpUi for TauriUi {
    fn status_changed(&self, status: &McpStatus) {
        use tauri::Emitter;
        if let Err(e) = self.0.emit(super::STATE_EVENT, status) {
            tracing::warn!("emit {}: {e}", super::STATE_EVENT);
        }
    }

    fn request_attention(&self) {
        use tauri::Manager;
        if let Some(w) = self.0.get_webview_window("main")
            && let Err(e) = w.request_user_attention(Some(tauri::UserAttentionType::Informational))
        {
            tracing::warn!("request_user_attention: {e}");
        }
    }
}

struct Inner {
    handle: Option<McpServerHandle>,
    /// `mcp.json`, записанный этим сервером: удаляется только он.
    file: Option<McpFile>,
    server: McpServerState,
    error: Option<String>,
}

pub struct McpController {
    data_dir: PathBuf,
    settings: Arc<SettingsStore>,
    policy: Arc<Policy>,
    connections: Arc<ConnectionStore>,
    audit: Arc<AuditLog>,
    approvals: Arc<Approvals>,
    ui: OnceLock<Box<dyn McpUi>>,
    inner: Mutex<Inner>,
    /// Запуск и остановка не пересекаются.
    ops: tokio::sync::Mutex<()>,
    /// Очередь фонового ассистента — источник `McpStatus.agent` (без цикла `Arc`: очередь держит контроллер).
    agent: RwLock<Weak<AgentQueue>>,
}

impl McpController {
    pub fn new(
        data_dir: PathBuf,
        settings: Arc<SettingsStore>,
        policy: Arc<Policy>,
        connections: Arc<ConnectionStore>,
    ) -> Self {
        Self {
            audit: Arc::new(AuditLog::new(&data_dir)),
            approvals: Arc::new(Approvals::new(settings.clone())),
            data_dir,
            settings,
            policy,
            connections,
            ui: OnceLock::new(),
            inner: Mutex::new(Inner {
                handle: None,
                file: None,
                server: McpServerState::Stopped,
                error: None,
            }),
            ops: tokio::sync::Mutex::new(()),
            agent: RwLock::new(Weak::new()),
        }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Окно приложения — получатель `mcp://state` и запросов внимания (вызывается один раз в `setup`).
    pub fn set_app_handle(&self, h: tauri::AppHandle) {
        self.set_ui(Box::new(TauriUi(h)));
    }

    pub(crate) fn set_ui(&self, ui: Box<dyn McpUi>) {
        if self.ui.set(ui).is_err() {
            tracing::warn!("mcp ui is already set");
        }
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock_safe()
    }

    fn policy_allows(&self) -> bool {
        self.policy.status.mcp_allowed()
    }

    fn set_server(&self, server: McpServerState, error: Option<String>) {
        let mut inner = self.inner();
        inner.server = server;
        inner.error = error;
    }

    pub fn status(&self) -> McpStatus {
        let profiles = self.connections.list();
        let mcp = self.settings.mcp();
        let (server, error, sessions) = {
            let inner = self.inner();
            (
                inner.server,
                inner.error.clone(),
                inner
                    .handle
                    .as_ref()
                    .map(|h| h.sessions())
                    .unwrap_or_default(),
            )
        };
        let policy = &self.policy.status;
        let (open_connections, data_connections) =
            crate::mcp::access::open_counts(&profiles, policy);
        McpStatus {
            server,
            error,
            enabled: mcp.enabled,
            sessions,
            pending: self.approvals.pending(),
            approved_clients: mcp.approved_clients,
            open_connections,
            data_connections,
            policy_enabled: policy.mcp_allowed(),
            max_access: policy.mcp_max_access,
            agent: self.agent_status(),
        }
    }

    /// Отправляет статус в интерфейс (`mcp://state`); без окна (тесты, шим) — ничего.
    pub fn emit_state(&self) {
        if let Some(ui) = self.ui.get() {
            ui.status_changed(&self.status());
        }
    }

    /// Очередь фонового ассистента — источник `McpStatus.agent`; при пересоздании очереди заменяется.
    pub(crate) fn set_agent(&self, q: Weak<AgentQueue>) {
        *self.agent.write_safe() = q;
    }

    fn agent_status(&self) -> AgentStatus {
        let queue = self.agent.read_safe().upgrade();
        match queue {
            Some(q) => q.status(),
            None => AgentStatus::idle(&self.settings.agent(), &self.policy.status),
        }
    }

    /// Сервер MCP этого окна работает: фоновый запуск возможен (§3.2).
    pub fn is_running(&self) -> bool {
        self.inner().server == McpServerState::Running
    }

    /// Тесты очереди: состояние сервера без настоящего сокета.
    #[cfg(test)]
    pub(crate) fn force_server(&self, server: McpServerState) {
        self.set_server(server, None);
    }

    pub(crate) fn request_attention(&self) {
        if let Some(ui) = self.ui.get() {
            ui.request_attention();
        }
    }

    /// Приводит сервер в соответствие с настройкой и политикой (при запуске приложения и после выключателя).
    pub async fn start_if_enabled(&self, st: AppState) {
        let _op = self.ops.lock().await;
        let wanted = self.settings.mcp().enabled && self.policy_allows();
        let running = self.inner().handle.is_some();
        if wanted && !running {
            self.start(&st).await;
        } else if !wanted && running {
            self.stop().await;
        } else if !wanted {
            self.set_server(McpServerState::Stopped, None);
        }
        self.emit_state();
    }

    async fn start(&self, st: &AppState) {
        if let Some(f) = files::read_mcp_file(&self.data_dir) {
            if endpoint::connect(&f.endpoint).await.is_ok() {
                self.set_server(McpServerState::OtherInstance, None);
                return;
            }
            // Сервер из файла не отвечает (сбой прежнего запуска): файл удаляется до bind, чтобы шим не ходил по
            // мёртвому адресу, если новый сервер не поднимется.
            files::remove_mcp_file(&self.data_dir, &f);
        }
        let token = match files::new_token() {
            Ok(t) => t,
            Err(e) => return self.failed(e),
        };
        let endpoint = Endpoint::for_app(&self.data_dir, MCP_SERVER_NAME);
        let backend = Arc::new(AppBackend::new(st.clone(), self.approvals.clone()));
        let factory = Arc::new(PgFactory::new(backend.clone()));
        let options = GatewayOptions::new(MCP_SERVER_NAME, env!("CARGO_PKG_VERSION"), INSTRUCTIONS);
        let started = glafira_mcp_core::server::start(
            &endpoint,
            token.clone(),
            backend,
            self.audit.clone(),
            options,
            factory,
        )
        .await;
        let handle = match started {
            Ok(h) => h,
            Err(e) => return self.failed(e),
        };
        let file = McpFile {
            version: files::VERSION,
            endpoint,
            token,
            pid: std::process::id(),
        };
        if let Err(e) = files::write_mcp_file(&self.data_dir, &file) {
            handle.shutdown().await;
            self.failed(e.message);
            return;
        }
        tracing::info!("mcp server started");
        let mut inner = self.inner();
        inner.handle = Some(handle);
        inner.file = Some(file);
        inner.server = McpServerState::Running;
        inner.error = None;
    }

    /// Сервер не поднялся: состояние «ошибка» с текстом для раздела «Настроек» (английский, как в скелете
    /// `McpStatus.error`; раздел показывает его внутри переведённой строки).
    fn failed(&self, e: impl std::fmt::Display) {
        tracing::warn!("mcp server not started: {e}");
        self.set_server(
            McpServerState::Error,
            Some(format!("The MCP server did not start: {e}")),
        );
    }

    async fn stop(&self) {
        let (handle, file) = {
            let mut inner = self.inner();
            (inner.handle.take(), inner.file.take())
        };
        if let Some(f) = &file {
            files::remove_mcp_file(&self.data_dir, f);
        }
        if let Some(h) = handle {
            h.shutdown().await;
        }
        self.set_server(McpServerState::Stopped, None);
        tracing::info!("mcp server stopped");
    }

    /// Выключатель «Разрешить MCP-сервер». Политика с `mcpEnabled = false` не даёт включить.
    pub async fn set_enabled(&self, st: AppState, on: bool) -> AppResult<McpStatus> {
        if on && !self.policy_allows() {
            return Err(app_err!(
                ErrorCode::InvalidInput,
                "errors.mcp.disabledByPolicy",
                "MCP is disabled by your organization"
            ));
        }
        let settings = self.settings.clone();
        crate::commands::blocking(move || settings.set_mcp_enabled(on)).await?;
        self.start_if_enabled(st).await;
        Ok(self.status())
    }

    /// Ответ в диалоге подтверждения: «Разрешить» запоминает клиента, «Отклонить» закрывает сессию.
    /// Блокирующий вызов (запись `settings.json`).
    pub fn answer_approval(&self, session_id: &str, approve: bool) -> AppResult<()> {
        if let Some(answered) = self.approvals.answer(session_id, approve, now_ms())? {
            let client = answered.client.map(|c| c.name);
            if approve {
                for session in std::iter::once(session_id.to_string()).chain(answered.also_approved)
                {
                    self.audit.record(AuditEvent::Approved {
                        session,
                        client: client.clone(),
                        run: None,
                        by: None,
                    });
                }
            } else {
                self.audit.record(AuditEvent::Rejected {
                    session: session_id.to_string(),
                    client,
                });
                if let Some(h) = &self.inner().handle {
                    h.disconnect(session_id);
                }
            }
        }
        self.emit_state();
        Ok(())
    }

    /// «Отозвать» подтверждённого клиента: его сессии закрываются. Блокирующий вызов.
    pub fn revoke_client(&self, name: &str) -> AppResult<()> {
        if self.approvals.revoke(name)? {
            self.audit.record(AuditEvent::Revoked {
                client: name.to_string(),
            });
        }
        if let Some(h) = &self.inner().handle {
            h.disconnect_client(name);
        }
        self.emit_state();
        Ok(())
    }

    /// «Отключить» одну сессию или всех (`None`); статус уходит в интерфейс, когда сессии закроются.
    pub fn disconnect(&self, session_id: Option<&str>) {
        if let Some(h) = &self.inner().handle {
            match session_id {
                Some(id) => {
                    h.disconnect(id);
                }
                None => h.disconnect_all(),
            }
        }
    }

    /// Конец фонового запуска (P4c §3.2): его сессии закрываются. Билет к этому времени уже отозван — новых сессий нет.
    pub fn disconnect_run(&self, run_id: &str) {
        if let Some(h) = &self.inner().handle {
            for s in h
                .sessions()
                .into_iter()
                .filter(|s| s.run_id.as_deref() == Some(run_id))
            {
                h.disconnect(&s.id);
            }
        }
    }

    /// Запись приложения в журнал MCP (P4b: `requestCreated`, `proposalAnswered`); журнал ведётся и при выключенном
    /// сервере, как у записей P4a.
    pub fn record_audit(&self, event: glafira_mcp_core::audit::AuditEvent) {
        self.audit.record(event);
    }

    pub fn audit_tail(&self, limit: usize) -> Vec<Value> {
        self.audit.tail(limit)
    }

    pub fn audit_path(&self) -> PathBuf {
        self.audit.path().to_path_buf()
    }

    /// При выходе из приложения: сервер останавливается, `mcp.json` удаляется.
    pub async fn shutdown(&self) {
        let _op = self.ops.lock().await;
        let running = self.inner().handle.is_some();
        if running {
            self.stop().await;
        }
    }

    /// Выход мимо `RunEvent::Exit` (установщик обновления на Windows вызывает `process::exit` из синхронного хука)
    /// или без runtime: свой `mcp.json` удаляется сразу, сессии закрываются; слушатель уходит вместе с процессом.
    pub fn exit_now(&self) {
        let inner = self.inner();
        if let Some(f) = &inner.file {
            files::remove_mcp_file(&self.data_dir, f);
        }
        if let Some(h) = &inner.handle {
            h.disconnect_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use glafira_mcp_core::handshake;
    use rmcp::model::{CallToolRequestParams, CallToolResult};
    use serde_json::json;

    use super::*;
    use crate::agent::tickets::RunScope;
    use crate::conn::profile::{LlmAccess, sample_profile};
    use crate::secrets::MemoryStore;

    #[derive(Default)]
    struct Recorder {
        emitted: AtomicU32,
        attention: AtomicU32,
    }

    impl McpUi for Arc<Recorder> {
        fn status_changed(&self, _: &McpStatus) {
            self.emitted.fetch_add(1, Ordering::SeqCst);
        }
        fn request_attention(&self) {
            self.attention.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn state(dir: &Path, policy_allows: bool) -> (AppState, Arc<Recorder>) {
        let mut policy = Policy::default();
        policy.status.mcp_enabled = policy_allows;
        let st = AppState::new(dir, Arc::new(MemoryStore::default()))
            .unwrap()
            .with_policy(policy);
        let rec = Arc::new(Recorder::default());
        st.mcp.set_ui(Box::new(rec.clone()));
        (st, rec)
    }

    /// Ждёт условия до 5 с (сессии открываются и закрываются в фоновых задачах сервера).
    async fn until(what: &str, mut f: impl FnMut() -> bool) {
        for _ in 0..500 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("не дождались: {what}");
    }

    async fn client_of(dir: &Path) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
        let f = files::read_mcp_file(dir).expect("mcp.json");
        let mut s = endpoint::connect(&f.endpoint).await.unwrap();
        handshake::client(&mut s, &f.token).await.unwrap();
        rmcp::ServiceExt::serve((), s).await.unwrap()
    }

    fn text(r: &CallToolResult) -> String {
        r.content
            .iter()
            .filter_map(|c| c.as_text())
            .map(|t| t.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Запись журнала с событием `event` (имя поля-тега не важно: ищем значение).
    fn logged(c: &McpController, event: &str) -> bool {
        c.audit_tail(50).iter().any(|e| {
            e.as_object()
                .is_some_and(|o| o.values().any(|v| v == event))
        })
    }

    #[tokio::test]
    async fn enabling_starts_the_server_and_disabling_removes_mcp_json() {
        let dir = tempfile::tempdir().unwrap();
        let (st, rec) = state(dir.path(), true);
        st.mcp.start_if_enabled(st.clone()).await;
        assert_eq!(st.mcp.status().server, McpServerState::Stopped);
        assert_eq!(files::read_mcp_file(dir.path()), None);

        let s = st.mcp.set_enabled(st.clone(), true).await.unwrap();
        assert_eq!((s.server, s.enabled), (McpServerState::Running, true));
        assert!(st.settings.mcp().enabled, "выключатель сохранён");
        let f = files::read_mcp_file(dir.path()).expect("mcp.json");
        assert_eq!(f.pid, std::process::id());
        assert_eq!(f.token.len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(files::FILE_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        drop(
            endpoint::connect(&f.endpoint)
                .await
                .expect("сервер слушает"),
        );

        let s = st.mcp.set_enabled(st.clone(), false).await.unwrap();
        assert_eq!((s.server, s.enabled), (McpServerState::Stopped, false));
        assert_eq!(files::read_mcp_file(dir.path()), None);
        assert!(
            rec.emitted.load(Ordering::SeqCst) >= 3,
            "статус уходит в интерфейс"
        );
    }

    #[tokio::test]
    async fn policy_forbids_the_server_even_when_the_setting_is_on() {
        let dir = tempfile::tempdir().unwrap();
        SettingsStore::open(dir.path())
            .set_mcp_enabled(true)
            .unwrap();
        let (st, _) = state(dir.path(), false);
        st.mcp.start_if_enabled(st.clone()).await;
        let s = st.mcp.status();
        assert_eq!(
            (s.server, s.enabled, s.policy_enabled),
            (McpServerState::Stopped, true, false)
        );
        assert_eq!(files::read_mcp_file(dir.path()), None);
        let e = st.mcp.set_enabled(st.clone(), true).await.unwrap_err();
        assert_eq!(e.key, "errors.mcp.disabledByPolicy");
    }

    #[test]
    fn status_counts_connections_open_for_llm_and_shows_the_policy_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        let mut policy = Policy::default();
        policy.status.mcp_max_access = McpMaxAccess::Diagnostics;
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default()))
            .unwrap()
            .with_policy(policy);
        let mut open = sample_profile("c1");
        open.llm_access = LlmAccess::Diagnostics;
        st.connections.upsert(open).unwrap();
        st.connections.upsert(sample_profile("c2")).unwrap();
        let s = st.mcp.status();
        assert_eq!((s.open_connections, s.data_connections), (1, 0));
        assert_eq!(
            (s.server, s.enabled, s.policy_enabled, s.max_access),
            (
                McpServerState::Stopped,
                false,
                true,
                McpMaxAccess::Diagnostics
            )
        );
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["server"], "stopped");
        assert_eq!(v["maxAccess"], "diagnostics");
        for key in [
            "error",
            "enabled",
            "sessions",
            "pending",
            "approvedClients",
            "openConnections",
            "dataConnections",
            "policyEnabled",
            "agent",
        ] {
            assert!(v.get(key).is_some(), "{key}: {v}");
        }
        assert_eq!(v["agent"]["queued"], 0);
        assert_eq!(v["agent"]["policyEnabled"], true);
        assert_eq!(v["agent"]["preset"], "qwen");
    }

    #[tokio::test]
    async fn second_instance_reports_other_instance_and_keeps_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (first, _) = state(dir.path(), true);
        first.mcp.set_enabled(first.clone(), true).await.unwrap();
        let own = files::read_mcp_file(dir.path()).unwrap();

        let (second, _) = state(dir.path(), true);
        second.mcp.start_if_enabled(second.clone()).await;
        assert_eq!(second.mcp.status().server, McpServerState::OtherInstance);
        second.mcp.shutdown().await;
        second.mcp.exit_now();
        assert_eq!(
            files::read_mcp_file(dir.path()),
            Some(own),
            "второй экземпляр чужой файл не трогает"
        );

        first.mcp.shutdown().await;
        assert_eq!(files::read_mcp_file(dir.path()), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_file_of_a_crashed_instance_is_removed_before_bind() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = state(dir.path(), true);
        files::write_mcp_file(
            dir.path(),
            &McpFile {
                version: files::VERSION,
                endpoint: Endpoint::Unix(dir.path().join("dead.sock")),
                token: files::new_token().unwrap(),
                pid: 1,
            },
        )
        .unwrap();
        // На месте сокета — каталог: сервер не поднимется, но мёртвый файл уже не нужен.
        std::fs::create_dir(dir.path().join("mcp.sock")).unwrap();
        let s = st.mcp.set_enabled(st.clone(), true).await.unwrap();
        assert_eq!(s.server, McpServerState::Error);
        assert!(
            s.error
                .as_deref()
                .is_some_and(|e| e.starts_with("The MCP server did not start: ")),
            "{:?}",
            s.error
        );
        assert_eq!(files::read_mcp_file(dir.path()), None);
    }

    #[tokio::test]
    async fn mcp_json_write_failure_gets_the_start_failure_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = state(dir.path(), true);
        // На месте файла — каталог: сервер поднимется, а запись mcp.json не удастся.
        std::fs::create_dir(dir.path().join(files::FILE_NAME)).unwrap();
        let s = st.mcp.set_enabled(st.clone(), true).await.unwrap();
        assert_eq!(s.server, McpServerState::Error);
        assert!(
            s.error
                .as_deref()
                .is_some_and(|e| e.starts_with("The MCP server did not start: ")),
            "{:?}",
            s.error
        );
    }

    #[tokio::test]
    async fn only_own_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = state(dir.path(), true);
        st.mcp.set_enabled(st.clone(), true).await.unwrap();
        let own = files::read_mcp_file(dir.path()).unwrap();
        let foreign = McpFile {
            token: files::new_token().unwrap(),
            pid: own.pid + 1,
            ..own
        };
        files::write_mcp_file(dir.path(), &foreign).unwrap();
        st.mcp.set_enabled(st.clone(), false).await.unwrap();
        assert_eq!(
            files::read_mcp_file(dir.path()),
            Some(foreign),
            "файл другого экземпляра остаётся"
        );
    }

    #[tokio::test]
    async fn exit_without_runtime_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = state(dir.path(), true);
        st.mcp.set_enabled(st.clone(), true).await.unwrap();
        assert!(files::read_mcp_file(dir.path()).is_some());
        st.mcp.exit_now();
        assert_eq!(files::read_mcp_file(dir.path()), None);
        st.mcp.shutdown().await;
    }

    #[tokio::test]
    async fn new_client_waits_for_approval_then_passes_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let (st, rec) = state(dir.path(), true);
        let c = st.mcp.clone();
        c.set_enabled(st.clone(), true).await.unwrap();
        let client = client_of(dir.path()).await;
        let list = || CallToolRequestParams::new("list_connections");

        let r = client.call_tool(list()).await.unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(text(&r).starts_with("client_not_approved"), "{}", text(&r));
        let pending = c.status().pending;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].client_name.as_deref(), Some("rmcp"));
        assert_eq!(
            rec.attention.load(Ordering::SeqCst),
            1,
            "окно привлекает внимание"
        );

        let (c2, id) = (c.clone(), pending[0].session_id.clone());
        tokio::task::spawn_blocking(move || c2.answer_approval(&id, true))
            .await
            .unwrap()
            .unwrap();
        let s = c.status();
        assert!(s.pending.is_empty());
        assert_eq!(s.approved_clients[0].name, "rmcp");
        let r = client.call_tool(list()).await.unwrap();
        // Право пройдено: отвечает AppBackend (подключений, открытых для LLM, нет).
        assert_eq!(text(&r), r#"{"connections":[]}"#);
        assert!(logged(&c, "approved"));
        drop(client);
        c.shutdown().await;
    }

    #[tokio::test]
    async fn rejected_session_is_closed_and_not_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = state(dir.path(), true);
        let c = st.mcp.clone();
        c.set_enabled(st.clone(), true).await.unwrap();
        let client = client_of(dir.path()).await;
        let _ = client
            .call_tool(CallToolRequestParams::new("list_connections"))
            .await;
        until("ожидающий запрос", || {
            c.status().pending.len() == 1
        })
        .await;
        let (c2, id) = (c.clone(), c.status().pending[0].session_id.clone());
        tokio::task::spawn_blocking(move || c2.answer_approval(&id, false))
            .await
            .unwrap()
            .unwrap();
        until("сессия закрыта", || {
            c.status().sessions.is_empty()
        })
        .await;
        let s = c.status();
        assert!(s.pending.is_empty() && s.approved_clients.is_empty());
        assert!(logged(&c, "rejected"));
        drop(client);
        c.shutdown().await;
    }

    #[tokio::test]
    async fn revoking_a_client_closes_its_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = state(dir.path(), true);
        st.settings.approve_client("rmcp", "1", 1).unwrap();
        let c = st.mcp.clone();
        c.set_enabled(st.clone(), true).await.unwrap();
        let client = client_of(dir.path()).await;
        let r = client
            .call_tool(CallToolRequestParams::new("list_connections"))
            .await
            .unwrap();
        assert_eq!(text(&r), r#"{"connections":[]}"#, "клиент уже подтверждён");
        assert_eq!(c.status().sessions.len(), 1);

        let c2 = c.clone();
        tokio::task::spawn_blocking(move || c2.revoke_client("rmcp"))
            .await
            .unwrap()
            .unwrap();
        until(
            "сессии отозванного клиента закрыты",
            || c.status().sessions.is_empty(),
        )
        .await;
        assert!(c.status().approved_clients.is_empty());
        assert!(logged(&c, "revoked"));
        drop(client);
        c.shutdown().await;
    }

    /// Сессия фонового запуска через настоящий сервер (P4c §3.3, §5):
    /// - билет в рукопожатии, подтверждение без диалога, `approved` с `run`;
    /// - девять инструментов, подключение запуска, `runId` в статусе;
    /// - `propose_query_rewrite` с `requestId` запуска и `connectionId` его подключения — форма запроса (M6);
    /// - «Отозвать» клиента её не закрывает;
    /// - после конца запуска билет не действует.
    #[tokio::test]
    async fn background_run_session_goes_through_the_real_server() {
        let dir = tempfile::tempdir().unwrap();
        let (st, rec) = state(dir.path(), true);
        for id in ["c1", "c2"] {
            let mut p = sample_profile(id);
            p.llm_access = LlmAccess::Diagnostics;
            st.connections.upsert(p).unwrap();
        }
        let request = st
            .llm
            .requests
            .create(crate::mcp::llm::requests::tests::new("c1"))
            .unwrap();
        let c = st.mcp.clone();
        c.set_enabled(st.clone(), true).await.unwrap();
        let ticket = st.tickets.issue(RunScope {
            run_id: "a1b2c3d".into(),
            connection_id: Some("c1".into()),
            request_id: Some(request.id.clone()),
        });
        let f = files::read_mcp_file(dir.path()).expect("mcp.json");
        let mut s = endpoint::connect(&f.endpoint).await.unwrap();
        handshake::client_with_run(&mut s, &f.token, Some(&ticket))
            .await
            .unwrap();
        let client = rmcp::ServiceExt::serve((), s).await.unwrap();

        let r = client
            .call_tool(CallToolRequestParams::new("list_connections"))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&text(&r)).unwrap();
        assert_eq!(v["connections"].as_array().unwrap().len(), 1, "{v}");
        assert_eq!(
            v["connections"][0]["id"], "c1",
            "только подключение запуска"
        );
        let s = c.status();
        assert!(
            s.pending.is_empty() && s.approved_clients.is_empty(),
            "без диалога и без запоминания клиента"
        );
        assert_eq!(rec.attention.load(Ordering::SeqCst), 0);
        assert_eq!(s.sessions[0].run_id.as_deref(), Some("a1b2c3d"));
        assert_eq!(
            serde_json::to_value(&s).unwrap()["sessions"][0]["runId"],
            "a1b2c3d"
        );
        let log = c.audit_tail(50);
        let approved: Vec<&Value> = log.iter().filter(|e| e["event"] == "approved").collect();
        assert_eq!(approved.len(), 1);
        assert_eq!(
            (approved[0]["run"].as_str(), approved[0]["by"].as_str()),
            (Some("a1b2c3d"), Some("run"))
        );
        assert!(
            log.iter()
                .any(|e| e["event"] == "sessionStarted" && e["run"] == "a1b2c3d")
        );

        assert_eq!(client.list_all_tools().await.unwrap().len(), 9);
        assert!(client.list_all_prompts().await.unwrap().is_empty());
        assert_eq!(
            client.peer_info().unwrap().instructions.as_deref(),
            Some(glafira_pg_mcp::server::INSTRUCTIONS_RUN)
        );
        for (tool, args) in [
            ("get_health_summary", json!({"connectionId": "c1"})),
            (
                "explain_query",
                json!({"connectionId": "c2", "sql": "SELECT 1"}),
            ),
            (
                "propose_query_rewrite",
                json!({"connectionId": "c1", "sql": "SELECT 1", "rewrittenSql": "SELECT 1",
                       "rationale": "faster"}),
            ),
        ] {
            let p =
                CallToolRequestParams::new(tool).with_arguments(args.as_object().unwrap().clone());
            let r = client.call_tool(p).await.unwrap();
            assert_eq!(
                text(&r),
                "not_allowed: Not available to a background run",
                "{tool}"
            );
        }
        // Сверка с feat/p4b (M6): requestId запуска и connectionId его подключения — форма запроса, не отказ области.
        // UPDATE — не тот вид, что у SELECT запроса: отказ до сервера (стенд не нужен).
        let p = CallToolRequestParams::new("propose_query_rewrite").with_arguments(
            json!({"requestId": request.id, "connectionId": "c1",
                   "rewrittenSql": "UPDATE orders SET status = 'x'", "rationale": "faster"})
            .as_object()
            .unwrap()
            .clone(),
        );
        let r = client.call_tool(p).await.unwrap();
        assert!(
            text(&r).starts_with("invalid_sql: ") && text(&r).contains("same kind"),
            "{}",
            text(&r)
        );
        let r = client
            .call_tool(CallToolRequestParams::new("get_optimization_request"))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&text(&r)).unwrap();
        assert_eq!(v["id"], request.id.as_str(), "запрос запуска без id");

        let c2 = c.clone();
        tokio::task::spawn_blocking(move || c2.revoke_client("rmcp"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            c.status().sessions.len(),
            1,
            "«Отозвать» клиента фоновый запуск не останавливает"
        );

        st.tickets.revoke("a1b2c3d");
        let mut again = endpoint::connect(&f.endpoint).await.unwrap();
        let e = handshake::client_with_run(&mut again, &f.token, Some(&ticket))
            .await
            .unwrap_err();
        assert!(
            matches!(&e, handshake::HandshakeError::Rejected { code, .. } if code == "bad_ticket"),
            "{e:?}"
        );
        let r = client
            .call_tool(CallToolRequestParams::new("list_connections"))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true), "запуск закончился");
        until("сессия запуска закрыта", || {
            c.status().sessions.is_empty()
        })
        .await;
        drop(client);
        c.shutdown().await;
    }

    /// Конец фонового запуска (P4c §3.2): очередь закрывает его сессии; сессии других запусков остаются.
    #[tokio::test]
    async fn the_end_of_a_run_closes_its_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _rec) = state(dir.path(), true);
        let c = st.mcp.clone();
        c.set_enabled(st.clone(), true).await.unwrap();
        let f = files::read_mcp_file(dir.path()).expect("mcp.json");
        let mut clients = Vec::new();
        for run in ["a000001", "a000002"] {
            let ticket = st.tickets.issue(RunScope {
                run_id: run.into(),
                connection_id: None,
                request_id: None,
            });
            let mut s = endpoint::connect(&f.endpoint).await.unwrap();
            handshake::client_with_run(&mut s, &f.token, Some(&ticket))
                .await
                .unwrap();
            let client = rmcp::ServiceExt::serve((), s).await.unwrap();
            client
                .call_tool(CallToolRequestParams::new("list_connections"))
                .await
                .unwrap();
            clients.push(client);
        }
        until("две сессии запусков", || {
            c.status().sessions.len() == 2
        })
        .await;
        c.disconnect_run("a000009");
        st.tickets.revoke("a000001");
        c.disconnect_run("a000001");
        until("сессия запуска a000001 закрыта", || {
            c.status()
                .sessions
                .iter()
                .all(|s| s.run_id.as_deref() != Some("a000001"))
        })
        .await;
        let left: Vec<Option<String>> = c.status().sessions.into_iter().map(|s| s.run_id).collect();
        assert_eq!(left, [Some("a000002".to_string())]);
        drop(clients);
        c.shutdown().await;
    }
}
