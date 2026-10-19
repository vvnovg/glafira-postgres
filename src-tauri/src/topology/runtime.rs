//! Открытые подключения (спец. P1 §6–7): состояние, последняя топология, журнал, узел каталога,
//! фоновый опрос раз в 10 с. События уходят в интерфейс через `RuntimeSink`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde::Serialize;
use tokio::task::JoinHandle;

use crate::conn::pool::{IDLE_TIMEOUT, PoolRegistry};
use crate::conn::profile::{ConnectionKind, ConnectionProfile};
use crate::conn::run_blocking;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::secrets::{PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, SecretStore, secret_key};
use crate::sync::MutexExt;
use crate::topology::BoxFuture;
use crate::topology::connector::PooledConnector;
use crate::topology::journal::{Journal, TopologyEvent, diff};
use crate::topology::model::{Member, Topology, now_ms};
use crate::topology::observe::{default_member, observe};
use crate::topology::patroni::{PatroniHttp, ReqwestHttp};

pub const POLL_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
    Degraded,
    Failed,
}

/// Подключение глазами интерфейса (`get_topology`, событие `topology://changed`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyView {
    pub state: ConnState,
    pub topology: Option<Topology>,
    /// Узел, с которого читается каталог.
    pub catalog_member: Option<String>,
    /// Выбранный пользователем узел недоступен — используется узел по умолчанию.
    pub catalog_fallback: bool,
    pub error: Option<AppError>,
}

impl TopologyView {
    fn disconnected() -> Self {
        Self {
            state: ConnState::Disconnected,
            topology: None,
            catalog_member: None,
            catalog_fallback: false,
            error: None,
        }
    }
}

pub trait RuntimeSink: Send + Sync {
    fn state_changed(&self, connection_id: &str, state: ConnState, error: Option<&AppError>);
    fn topology_changed(
        &self,
        connection_id: &str,
        view: &TopologyView,
        new_events: &[TopologyEvent],
    );
}

/// Откуда берётся топология: в приложении — Patroni и опрос узлов, в тестах — подмена.
pub trait TopologySource: Send + Sync {
    fn observe<'a>(
        &'a self,
        profile: &'a ConnectionProfile,
        previous: Option<&'a Topology>,
    ) -> BoxFuture<'a, AppResult<Topology>>;
    /// Подключение закрыто: забыть кэш (клиент HTTP и т. п.).
    fn forget(&self, _connection_id: &str) {}
}

/// Patroni REST и опрос узлов через пулы приложения. Клиент HTTP строится один раз на подключение:
/// keychain не читается каждые 10 с.
pub struct LiveSource {
    secrets: Arc<dyn SecretStore>,
    pools: Arc<PoolRegistry>,
    http: Mutex<HashMap<String, Arc<ReqwestHttp>>>,
}

impl LiveSource {
    pub fn new(secrets: Arc<dyn SecretStore>, pools: Arc<PoolRegistry>) -> Self {
        Self {
            secrets,
            pools,
            http: Mutex::new(HashMap::new()),
        }
    }

    async fn http_for(&self, profile: &ConnectionProfile) -> AppResult<Arc<ReqwestHttp>> {
        if let Some(h) = self.http.lock_safe().get(&profile.id) {
            return Ok(h.clone());
        }
        let (p, s) = (profile.clone(), self.secrets.clone());
        let http = run_blocking(move || {
            let settings = p.patroni.as_ref().ok_or_else(|| {
                app_err!(
                    ErrorCode::PatroniError,
                    "errors.patroni.noUrls",
                    "No Patroni API addresses"
                )
            })?;
            let password = s.get(&secret_key(&p.id, PATRONI_PASSWORD))?;
            let key_password = s.get(&secret_key(&p.id, PATRONI_KEY_PASSWORD))?;
            ReqwestHttp::new(settings, password, key_password.as_deref())
        })
        .await?;
        let http = Arc::new(http);
        self.http
            .lock_safe()
            .insert(profile.id.clone(), http.clone());
        Ok(http)
    }
}

impl TopologySource for LiveSource {
    fn observe<'a>(
        &'a self,
        profile: &'a ConnectionProfile,
        previous: Option<&'a Topology>,
    ) -> BoxFuture<'a, AppResult<Topology>> {
        Box::pin(async move {
            let nodes = PooledConnector {
                pools: self.pools.clone(),
                profile: profile.clone(),
                secrets: self.secrets.clone(),
            };
            let http = match profile.kind {
                ConnectionKind::Patroni => Some(self.http_for(profile).await?),
                _ => None,
            };
            observe(
                profile,
                http.as_deref().map(|h| h as &dyn PatroniHttp),
                &nodes,
                previous,
                now_ms(),
            )
            .await
        })
    }

    fn forget(&self, connection_id: &str) {
        self.http.lock_safe().remove(connection_id);
    }
}

struct Entry {
    profile: ConnectionProfile,
    state: ConnState,
    topology: Option<Topology>,
    journal: Journal,
    chosen_member: Option<String>,
    error: Option<AppError>,
    poller: Option<JoinHandle<()>>,
}

impl Entry {
    fn view(&self) -> TopologyView {
        let (catalog_member, catalog_fallback) = match &self.topology {
            None => (None, false),
            Some(t) => {
                let chosen = self
                    .chosen_member
                    .as_deref()
                    .filter(|name| t.member(name).is_some_and(|m| m.reachable));
                match (chosen, &self.chosen_member) {
                    (Some(c), _) => (Some(c.to_string()), false),
                    (None, wanted) => (default_member(&self.profile, t), wanted.is_some()),
                }
            }
        };
        TopologyView {
            state: self.state,
            topology: self.topology.clone(),
            catalog_member,
            catalog_fallback,
            error: self.error.clone(),
        }
    }
}

fn state_for(t: &Topology) -> ConnState {
    if t.is_degraded() {
        ConnState::Degraded
    } else {
        ConnState::Connected
    }
}

pub struct Runtimes {
    source: Arc<dyn TopologySource>,
    pools: Arc<PoolRegistry>,
    interval: Duration,
    entries: Mutex<HashMap<String, Entry>>,
    connect_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    sink: Mutex<Option<Arc<dyn RuntimeSink>>>,
}

fn not_connected() -> AppError {
    app_err!(
        ErrorCode::NodeUnavailable,
        "errors.topology.notConnected",
        "The connection is not open"
    )
}

impl Runtimes {
    pub fn new(source: Arc<dyn TopologySource>, pools: Arc<PoolRegistry>) -> Arc<Self> {
        Self::with_interval(source, pools, POLL_INTERVAL)
    }

    pub fn with_interval(
        source: Arc<dyn TopologySource>,
        pools: Arc<PoolRegistry>,
        interval: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            source,
            pools,
            interval,
            entries: Mutex::new(HashMap::new()),
            connect_locks: Mutex::new(HashMap::new()),
            sink: Mutex::new(None),
        })
    }

    pub fn set_sink(&self, sink: Arc<dyn RuntimeSink>) {
        *self.sink.lock_safe() = Some(sink);
    }

    fn sink(&self) -> Option<Arc<dyn RuntimeSink>> {
        self.sink.lock_safe().clone()
    }

    fn emit_state(&self, id: &str, state: ConnState, error: Option<&AppError>) {
        if let Some(s) = self.sink() {
            s.state_changed(id, state, error);
        }
    }

    fn emit_topology(&self, id: &str, view: &TopologyView, events: &[TopologyEvent]) {
        if let Some(s) = self.sink() {
            s.topology_changed(id, view, events);
        }
    }

    /// Открывает подключение (или возвращает уже открытое с тем же профилем).
    pub async fn connect(self: &Arc<Self>, profile: ConnectionProfile) -> AppResult<TopologyView> {
        let id = profile.id.clone();
        let lock = self
            .connect_locks
            .lock_safe()
            .entry(id.clone())
            .or_default()
            .clone();
        let _guard = lock.lock().await;
        {
            let mut entries = self.entries.lock_safe();
            if let Some(e) = entries.get(&id)
                && matches!(e.state, ConnState::Connected | ConnState::Degraded)
                && e.profile == profile
            {
                return Ok(e.view());
            }
            if let Some(old) = entries.remove(&id)
                && let Some(h) = old.poller
            {
                h.abort();
            }
            entries.insert(
                id.clone(),
                Entry {
                    profile: profile.clone(),
                    state: ConnState::Connecting,
                    topology: None,
                    journal: Journal::default(),
                    chosen_member: None,
                    error: None,
                    poller: None,
                },
            );
        }
        self.pools.evict_connection(&id);
        self.source.forget(&id);
        self.emit_state(&id, ConnState::Connecting, None);
        match self.source.observe(&profile, None).await {
            Ok(t) => {
                let view = {
                    let mut entries = self.entries.lock_safe();
                    let e = entries.get_mut(&id).ok_or_else(not_connected)?;
                    // Таймлайн первого опроса — исходная точка: событие о нём не создаётся.
                    e.journal.observe_timeline(t.cluster_timeline());
                    e.state = state_for(&t);
                    e.topology = Some(t);
                    e.view()
                };
                self.emit_state(&id, view.state, None);
                self.emit_topology(&id, &view, &[]);
                self.start_poller(&id);
                Ok(view)
            }
            Err(err) => {
                if let Some(e) = self.entries.lock_safe().get_mut(&id) {
                    e.state = ConnState::Failed;
                    e.error = Some(err.clone());
                }
                self.emit_state(&id, ConnState::Failed, Some(&err));
                Err(err)
            }
        }
    }

    fn start_poller(self: &Arc<Self>, id: &str) {
        let weak: Weak<Self> = Arc::downgrade(self);
        let (key, interval) = (id.to_string(), self.interval);
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(rt) = weak.upgrade() else { break };
                if !rt.poll_once(&key).await {
                    break;
                }
            }
        });
        match self.entries.lock_safe().get_mut(id) {
            Some(e) => e.poller = Some(handle),
            None => handle.abort(),
        }
    }

    /// Один опрос; `false` — подключение закрыто, опрос прекращается.
    async fn poll_once(&self, id: &str) -> bool {
        let (profile, previous) = {
            let entries = self.entries.lock_safe();
            let Some(e) = entries.get(id) else {
                return false;
            };
            (e.profile.clone(), e.topology.clone())
        };
        self.pools.close_idle(IDLE_TIMEOUT);
        let result = self.source.observe(&profile, previous.as_ref()).await;
        let mut entries = self.entries.lock_safe();
        // Подключение закрыто или открыто заново с другим профилем, пока шёл опрос: результат устарел.
        let Some(e) = entries.get_mut(id).filter(|e| e.profile == profile) else {
            return false;
        };
        match result {
            Ok(t) => {
                let diffed = previous.as_ref().map(|p| diff(p, &t)).unwrap_or_default();
                if let Some(p) = &previous {
                    // Участник исчез или сменил адрес — его пулы не нужны.
                    for m in &p.members {
                        if t.member(&m.name)
                            .is_none_or(|n| n.host != m.host || n.port != m.port)
                        {
                            self.pools.evict_member(id, &m.name);
                        }
                    }
                }
                let changed = previous.as_ref().is_none_or(|p| !p.same_except_time(&t));
                // В интерфейс уходят только принятые журналом события (повторы таймлайна отброшены).
                let events = e.journal.extend(&diffed);
                e.journal.observe_timeline(t.cluster_timeline());
                let state = state_for(&t);
                let state_changed = state != e.state || e.error.is_some();
                e.state = state;
                e.error = None;
                e.topology = Some(t);
                let view = e.view();
                drop(entries);
                if state_changed {
                    self.emit_state(id, state, None);
                }
                if changed || !events.is_empty() {
                    self.emit_topology(id, &view, &events);
                }
            }
            Err(err) => {
                e.state = ConnState::Degraded;
                e.error = Some(err.clone());
                drop(entries);
                self.emit_state(id, ConnState::Degraded, Some(&err));
            }
        }
        true
    }

    pub fn disconnect(&self, id: &str) {
        let removed = self.entries.lock_safe().remove(id);
        if let Some(e) = removed
            && let Some(h) = e.poller
        {
            h.abort();
        }
        self.pools.evict_connection(id);
        self.source.forget(id);
        self.emit_state(id, ConnState::Disconnected, None);
    }

    pub fn view(&self, id: &str) -> TopologyView {
        self.entries
            .lock_safe()
            .get(id)
            .map(Entry::view)
            .unwrap_or_else(TopologyView::disconnected)
    }

    pub fn events(&self, id: &str) -> Vec<TopologyEvent> {
        self.entries
            .lock_safe()
            .get(id)
            .map(|e| e.journal.newest_first())
            .unwrap_or_default()
    }

    pub fn set_catalog_member(&self, id: &str, member: Option<String>) -> AppResult<TopologyView> {
        let mut entries = self.entries.lock_safe();
        let e = entries.get_mut(id).ok_or_else(not_connected)?;
        if let Some(name) = &member
            && e.topology.as_ref().and_then(|t| t.member(name)).is_none()
        {
            return Err(app_err!(
                ErrorCode::InvalidInput,
                "errors.topology.unknownMember",
                "Node {{member}} is not part of the connection",
                member = name
            ));
        }
        e.chosen_member = member;
        Ok(e.view())
    }

    /// Участник для запроса: `member` или узел каталога; профиль — тот, с которым открыто подключение.
    pub fn target(&self, id: &str, member: Option<&str>) -> AppResult<(Member, ConnectionProfile)> {
        let entries = self.entries.lock_safe();
        let e = entries.get(id).ok_or_else(not_connected)?;
        let t = e.topology.as_ref().ok_or_else(not_connected)?;
        let name = match member {
            Some(m) => m.to_string(),
            None => e.view().catalog_member.ok_or_else(|| {
                app_err!(
                    ErrorCode::NodeUnavailable,
                    "errors.topology.noNode",
                    "No suitable node is available"
                )
            })?,
        };
        let m = t.member(&name).cloned().ok_or_else(|| {
            app_err!(
                ErrorCode::InvalidInput,
                "errors.topology.unknownMember",
                "Node {{member}} is not part of the connection",
                member = name
            )
        })?;
        Ok((m, e.profile.clone()))
    }

    /// Останавливает все опросы (выход из приложения, конец теста).
    pub fn shutdown(&self) {
        for (_, e) in self.entries.lock_safe().drain() {
            if let Some(h) = e.poller {
                h.abort();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use super::*;
    use crate::conn::pool::PoolKey;
    use crate::conn::profile::{ConnectionKind, sample_profile};
    use crate::error::ErrorCode;
    use crate::topology::journal::EventKind;
    use crate::topology::model::{Role, TopologyWarning, member, topo};

    /// Источник, отдающий заранее заданные ответы; последний повторяется.
    struct Scripted {
        answers: Mutex<VecDeque<AppResult<Topology>>>,
        last: Mutex<Option<AppResult<Topology>>>,
        calls: AtomicUsize,
        delay: Duration,
    }

    impl Scripted {
        fn new(answers: Vec<AppResult<Topology>>) -> Arc<Self> {
            Arc::new(Self {
                answers: Mutex::new(answers.into()),
                last: Mutex::new(None),
                calls: AtomicUsize::new(0),
                delay: Duration::ZERO,
            })
        }

        /// Следующие ответы вместо оставшихся; прежний «последний» повторяется, пока очередь не опустеет.
        fn then(&self, answers: Vec<AppResult<Topology>>) {
            *self.answers.lock_safe() = answers.into();
        }
    }

    impl TopologySource for Scripted {
        fn observe<'a>(
            &'a self,
            _p: &'a ConnectionProfile,
            _prev: Option<&'a Topology>,
        ) -> BoxFuture<'a, AppResult<Topology>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let next = self.answers.lock_safe().pop_front();
            let r = match next {
                Some(r) => {
                    *self.last.lock_safe() = Some(r.clone());
                    r
                }
                None => self.last.lock_safe().clone().expect("есть ответ"),
            };
            let delay = self.delay;
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                r.map(|mut t| {
                    t.observed_at_ms = crate::topology::model::now_ms();
                    t
                })
            })
        }
    }

    #[derive(Default)]
    struct Recorder {
        states: Mutex<Vec<ConnState>>,
        topologies: Mutex<Vec<Vec<TopologyEvent>>>,
    }

    impl RuntimeSink for Recorder {
        fn state_changed(&self, _id: &str, state: ConnState, _e: Option<&AppError>) {
            self.states.lock_safe().push(state);
        }
        fn topology_changed(&self, _id: &str, _v: &TopologyView, events: &[TopologyEvent]) {
            self.topologies.lock_safe().push(events.to_vec());
        }
    }

    impl Recorder {
        fn sent_events(&self) -> Vec<TopologyEvent> {
            self.topologies
                .lock_safe()
                .iter()
                .flatten()
                .cloned()
                .collect()
        }
    }

    fn patroni_profile() -> ConnectionProfile {
        let mut p = sample_profile("c");
        p.kind = ConnectionKind::Patroni;
        p
    }

    fn two(leader: &str) -> Topology {
        let other = if leader == "a" { "b" } else { "a" };
        topo(vec![
            member(leader, Role::Primary),
            member(other, Role::Replica),
        ])
    }

    #[allow(clippy::type_complexity)]
    fn setup(
        answers: Vec<AppResult<Topology>>,
    ) -> (
        Arc<Runtimes>,
        Arc<Scripted>,
        Arc<Recorder>,
        Arc<PoolRegistry>,
    ) {
        let src = Scripted::new(answers);
        let pools = Arc::new(PoolRegistry::default());
        let rt = Runtimes::with_interval(src.clone(), pools.clone(), Duration::from_millis(40));
        let rec = Arc::new(Recorder::default());
        rt.set_sink(rec.clone());
        (rt, src, rec, pools)
    }

    fn err() -> AppError {
        app_err!(
            ErrorCode::PatroniError,
            "errors.patroni.noUrls",
            "No Patroni API addresses"
        )
    }

    /// Опрос идёт в фоне: ждём условия до 5 с, а не фиксированную паузу, — тест не зависит от нагрузки машины.
    async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < deadline, "не дождались: {what}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn pool_key(member: &str) -> PoolKey {
        PoolKey {
            connection_id: "c".into(),
            member: member.into(),
            database: "postgres".into(),
        }
    }

    fn add_pool(pools: &PoolRegistry, member: &str) {
        pools.insert_if_absent(
            pool_key(member),
            crate::conn::pg::ConnectSpec::build(
                &sample_profile("c"),
                "h",
                1,
                "postgres",
                &crate::secrets::MemoryStore::default(),
            )
            .unwrap(),
        );
    }

    #[tokio::test]
    async fn connect_then_poll_records_switchover() {
        let (rt, _src, rec, _) = setup(vec![Ok(two("a")), Ok(two("b"))]);
        let view = rt.connect(patroni_profile()).await.unwrap();
        assert_eq!(view.state, ConnState::Connected);
        assert_eq!(view.catalog_member.as_deref(), Some("a"));
        // Событие уходит в интерфейс после записи в журнал и смены узла каталога.
        eventually("событие в интерфейсе", || {
            rec.topologies.lock_safe().iter().any(|e| !e.is_empty())
        })
        .await;
        let events = rt.events("c");
        assert!(events.iter().any(|e| e.event
            == EventKind::LeaderChanged {
                from: Some("a".into()),
                to: Some("b".into())
            }));
        assert_eq!(
            rt.view("c").catalog_member.as_deref(),
            Some("b"),
            "каталог следует за лидером"
        );
        assert_eq!(
            &rec.states.lock_safe()[..2],
            &[ConnState::Connecting, ConnState::Connected]
        );
        assert!(rec.topologies.lock_safe().iter().any(|e| !e.is_empty()));
        rt.shutdown();
    }

    #[tokio::test]
    async fn poll_error_degrades_and_recovers() {
        // Ошибка повторяется, пока тест не подставит следующий ответ: состояние «Degraded» не зависит от темпа опроса.
        let (rt, src, rec, _) = setup(vec![Ok(two("a")), Err(err())]);
        rt.connect(patroni_profile()).await.unwrap();
        eventually("Degraded", || rt.view("c").state == ConnState::Degraded).await;
        let v = rt.view("c");
        assert!(v.error.is_some());
        assert!(v.topology.is_some(), "прошлая топология остаётся");

        src.then(vec![Ok(two("a"))]);
        eventually("Connected", || rt.view("c").state == ConnState::Connected).await;
        assert!(rt.view("c").error.is_none());
        let states = rec.states.lock_safe().clone();
        assert_eq!(
            states[..4],
            [
                ConnState::Connecting,
                ConnState::Connected,
                ConnState::Degraded,
                ConnState::Connected
            ]
        );
        rt.shutdown();
    }

    #[tokio::test]
    async fn failed_connect_and_disconnect() {
        let (rt, src, rec, pools) = setup(vec![Err(err())]);
        let e = rt.connect(patroni_profile()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::PatroniError);
        assert_eq!(rt.view("c").state, ConnState::Failed);

        *src.answers.lock_safe() = VecDeque::from(vec![Ok(two("a"))]);
        rt.connect(patroni_profile()).await.unwrap();
        pools.insert_if_absent(
            crate::conn::pool::PoolKey {
                connection_id: "c".into(),
                member: "a".into(),
                database: "postgres".into(),
            },
            crate::conn::pg::ConnectSpec::build(
                &sample_profile("c"),
                "h",
                1,
                "postgres",
                &crate::secrets::MemoryStore::default(),
            )
            .unwrap(),
        );
        rt.disconnect("c");
        assert_eq!(rt.view("c").state, ConnState::Disconnected);
        assert!(pools.keys().is_empty(), "пулы подключения закрыты");
        let calls = src.calls.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(src.calls.load(Ordering::SeqCst), calls, "опрос остановлен");
        assert_eq!(
            rec.states.lock_safe().last(),
            Some(&ConnState::Disconnected)
        );
    }

    #[tokio::test]
    async fn catalog_member_choice_and_fallback() {
        let mut down = two("a");
        down.members[1].reachable = false;
        // Пока тест не подставит `down`, опрос возвращает исходную топологию: выбор узла не гонится с опросом.
        let (rt, src, _rec, _) = setup(vec![Ok(two("a"))]);
        rt.connect(patroni_profile()).await.unwrap();
        let v = rt.set_catalog_member("c", Some("b".into())).unwrap();
        assert_eq!(
            (v.catalog_member.as_deref(), v.catalog_fallback),
            (Some("b"), false)
        );
        assert_eq!(
            rt.set_catalog_member("c", Some("zzz".into()))
                .unwrap_err()
                .key,
            "errors.topology.unknownMember"
        );
        src.then(vec![Ok(down)]);
        eventually("узел b недоступен", || {
            rt.view("c").catalog_fallback
        })
        .await;
        let v = rt.view("c");
        assert_eq!(
            (v.catalog_member.as_deref(), v.catalog_fallback),
            (Some("a"), true)
        );
        let (m, _) = rt.target("c", None).unwrap();
        assert_eq!(m.name, "a");
        assert_eq!(
            rt.target("nope", None).unwrap_err().key,
            "errors.topology.notConnected"
        );
        rt.shutdown();
    }

    #[tokio::test]
    async fn concurrent_connects_observe_once() {
        let src = Arc::new(Scripted {
            answers: Mutex::new(VecDeque::from(vec![Ok(two("a"))])),
            last: Mutex::new(None),
            calls: AtomicUsize::new(0),
            delay: Duration::from_millis(50),
        });
        let rt = Runtimes::with_interval(
            src.clone(),
            Arc::new(PoolRegistry::default()),
            Duration::from_secs(60),
        );
        let (a, b) = tokio::join!(rt.connect(patroni_profile()), rt.connect(patroni_profile()));
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(src.calls.load(Ordering::SeqCst), 1);
        rt.shutdown();
    }

    /// Топология с таймлайнами: у `a` (primary) таймлайн 2, у `b` (реплика) — 1.
    fn timelines(a_reachable: bool) -> Topology {
        let mut t = two("a");
        t.members[0].timeline = Some(2);
        t.members[1].timeline = Some(1);
        if !a_reachable {
            t.members[0].reachable = false;
            t.members[0].role = Role::Unknown;
            t.members[0].state = None;
            t.warnings = vec![TopologyWarning::MemberUnreachable { name: "a".into() }];
        }
        t
    }

    fn timeline_events(events: &[TopologyEvent]) -> Vec<&TopologyEvent> {
        events
            .iter()
            .filter(|e| matches!(e.event, EventKind::TimelineChanged { .. }))
            .collect()
    }

    #[tokio::test]
    async fn node_returning_after_start_does_not_repeat_the_timeline() {
        // Подключение открыто при видимом таймлайне 2; узел с TL 2 пропал и вернулся — 1→2 не новость.
        let (rt, src, rec, _) = setup(vec![Ok(timelines(true))]);
        rt.connect(patroni_profile()).await.unwrap();
        src.then(vec![Ok(timelines(false))]);
        eventually("узел a недоступен", || {
            rt.view("c").state == ConnState::Degraded
        })
        .await;
        src.then(vec![Ok(timelines(true))]);
        eventually("узел a вернулся", || {
            rec.sent_events()
                .iter()
                .any(|e| e.event == EventKind::MemberReachable { name: "a".into() })
        })
        .await;
        assert!(
            timeline_events(&rt.events("c")).is_empty(),
            "{:?}",
            rt.events("c")
        );
        assert!(
            timeline_events(&rec.sent_events()).is_empty(),
            "в интерфейс уходят только принятые события"
        );
        rt.shutdown();
    }

    #[tokio::test]
    async fn timeline_seen_at_connect_is_the_baseline_even_when_the_node_was_down() {
        // При подключении узел с TL 2 недоступен: таймлайн кластера 1. Рост до 2 — настоящее событие.
        let (rt, src, rec, _) = setup(vec![Ok(timelines(false))]);
        rt.connect(patroni_profile()).await.unwrap();
        src.then(vec![Ok(timelines(true))]);
        eventually("TimelineChanged в интерфейсе", || {
            !timeline_events(&rec.sent_events()).is_empty()
        })
        .await;
        let sent = rec.sent_events();
        let ev = timeline_events(&sent);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].event, EventKind::TimelineChanged { from: 1, to: 2 });
        assert_eq!(timeline_events(&rt.events("c")).len(), 1);
        rt.shutdown();
    }

    #[tokio::test]
    async fn timeline_first_seen_in_a_later_poll_is_remembered() {
        // При подключении таймлайн не известен ни у одного узла; на втором опросе узел a сообщает TL 2
        // (события нет: сравнивать не с чем). Пропал и вернулся — 1→2 не новость.
        let mut unknown = two("a");
        unknown.members.iter_mut().for_each(|m| m.timeline = None);
        let (rt, src, rec, _) = setup(vec![Ok(unknown)]);
        rt.connect(patroni_profile()).await.unwrap();
        src.then(vec![Ok(timelines(true))]);
        eventually("TL 2 виден", || {
            rt.view("c")
                .topology
                .is_some_and(|t| t.cluster_timeline() == Some(2))
        })
        .await;
        src.then(vec![Ok(timelines(false))]);
        eventually("узел a недоступен", || {
            rt.view("c").state == ConnState::Degraded
        })
        .await;
        src.then(vec![Ok(timelines(true))]);
        eventually("узел a вернулся", || {
            rec.sent_events()
                .iter()
                .any(|e| e.event == EventKind::MemberReachable { name: "a".into() })
        })
        .await;
        assert!(
            timeline_events(&rt.events("c")).is_empty(),
            "{:?}",
            rt.events("c")
        );
        assert!(timeline_events(&rec.sent_events()).is_empty());
        rt.shutdown();
    }

    #[tokio::test]
    async fn poll_result_for_a_replaced_profile_is_dropped() {
        let src = Arc::new(Scripted {
            answers: Mutex::new(VecDeque::from(vec![Ok(two("a"))])),
            last: Mutex::new(None),
            calls: AtomicUsize::new(0),
            delay: Duration::from_millis(150),
        });
        let rt = Runtimes::with_interval(
            src,
            Arc::new(PoolRegistry::default()),
            Duration::from_secs(60),
        );
        rt.connect(patroni_profile()).await.unwrap();
        // Опрос идёт, а подключение открывают заново с изменённым профилем: ответ прежнего профиля устарел.
        let poll = tokio::spawn({
            let rt = rt.clone();
            async move { rt.poll_once("c").await }
        });
        tokio::time::sleep(Duration::from_millis(40)).await;
        let mut changed = patroni_profile();
        changed.name = "renamed".into();
        rt.connect(changed).await.unwrap();
        assert!(!poll.await.unwrap(), "опрос прекращается");
        assert!(rt.events("c").is_empty());
        rt.shutdown();
    }

    #[tokio::test]
    async fn pools_follow_the_members() {
        let mut a_only = two("a");
        a_only.members.truncate(1);
        let mut moved = a_only.clone();
        moved.members[0].host = "elsewhere".into();
        let (rt, src, _rec, pools) = setup(vec![Ok(two("a"))]);
        // Пул, оставшийся от прежнего открытия, при подключении закрывается.
        add_pool(&pools, "stale");
        rt.connect(patroni_profile()).await.unwrap();
        assert!(pools.keys().is_empty());
        add_pool(&pools, "a");
        add_pool(&pools, "b");

        src.then(vec![Ok(a_only)]);
        eventually(
            "пул исчезнувшего узла закрыт",
            || pools.get(&pool_key("b")).is_none(),
        )
        .await;
        assert!(pools.get(&pool_key("a")).is_some());

        src.then(vec![Ok(moved)]);
        eventually(
            "пул узла со сменой адреса закрыт",
            || pools.get(&pool_key("a")).is_none(),
        )
        .await;
        rt.shutdown();
    }
}
