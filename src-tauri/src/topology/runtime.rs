//! Открытые подключения (спец. P1 §6–7): состояние, последняя топология, журнал, узел каталога,
//! фоновый опрос раз в 10 с. События уходят в интерфейс через `RuntimeSink`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde::Serialize;
use tokio::task::JoinHandle;

use crate::conn::pool::{IDLE_TIMEOUT, PoolRegistry};
use crate::conn::profile::{ConnectionKind, ConnectionProfile, same_connection_settings};
use crate::conn::run_blocking;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::net::ssh::tunnel::TunnelRegistry;
use crate::secrets::{PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, SecretStore, secret_key};
use crate::sync::MutexExt;
use crate::topology::BoxFuture;
use crate::topology::connector::PooledConnector;
use crate::topology::journal::{Journal, TopologyEvent, diff};
use crate::topology::model::{Member, Topology, now_ms};
use crate::topology::observe::{default_member, leader_member, observe};
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

/// Приёмник событий открытых подключений. Методы вызываются под внутренней блокировкой `Runtimes`
/// (так порядок событий совпадает с порядком изменений: после `Disconnected` не приходит ничего от
/// прежнего открытия), поэтому обработчик должен быть быстрым и не обращаться к `Runtimes`.
pub trait RuntimeSink: Send + Sync {
    fn state_changed(&self, connection_id: &str, state: ConnState, error: Option<&AppError>);
    fn topology_changed(
        &self,
        connection_id: &str,
        view: &TopologyView,
        new_events: &[TopologyEvent],
    );
}

/// Слушатель закрытий (спец. P2 §3.4): подключение закрыто, удалено или переоткрыто с другими настройками
/// соединения; участник исчез из топологии или сменил адрес. Состояние, привязанное к подключению или узлу
/// (прошлые счётчики диагностики, сэмплеры), стирается. Как и `RuntimeSink`, вызывается под внутренней
/// блокировкой `Runtimes`: обработчик быстрый (std-мьютекс, без `await`) и не обращается к `Runtimes`.
pub trait CloseListener: Send + Sync {
    fn connection_closed(&self, connection_id: &str);
    fn member_removed(&self, connection_id: &str, member: &str);
}

/// Что нужно инструменту диагностики: профиль, номер открытия, топология и узел.
#[derive(Debug, Clone)]
pub struct DiagTarget {
    pub profile: ConnectionProfile,
    pub generation: u64,
    pub topology: Topology,
    /// `None` — инструмент кластера.
    pub member: Option<Member>,
    /// Узел выбран правилом «за лидером», а не пользователем.
    pub follows_leader: bool,
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

/// Patroni REST и опрос узлов через пулы приложения. Клиент HTTP строится один раз на подключение и маршрут:
/// keychain не читается каждые 10 с, а после перезапуска туннеля на новом порту клиент строится заново.
pub struct LiveSource {
    secrets: Arc<dyn SecretStore>,
    pools: Arc<PoolRegistry>,
    tunnels: Arc<TunnelRegistry>,
    http: Mutex<HashMap<String, RoutedHttp>>,
}

/// Клиент HTTP подключения и маршрут, на который он настроен (`None` — напрямую, иначе порт туннеля).
type RoutedHttp = (Option<SocketAddr>, Arc<ReqwestHttp>);

impl LiveSource {
    pub fn new(
        secrets: Arc<dyn SecretStore>,
        pools: Arc<PoolRegistry>,
        tunnels: Arc<TunnelRegistry>,
    ) -> Self {
        Self {
            secrets,
            pools,
            tunnels,
            http: Mutex::new(HashMap::new()),
        }
    }

    /// Клиент HTTP Patroni подключения (общий с опросом топологии): инструменты Patroni не строят второго.
    pub async fn http_for(&self, profile: &ConnectionProfile) -> AppResult<Arc<ReqwestHttp>> {
        // Туннель упал или закрыт — его ошибка: напрямую к API не идём.
        let proxy = self.tunnels.route_for(profile)?;
        if let Some((route, h)) = self.http.lock_safe().get(&profile.id)
            && *route == proxy
        {
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
            ReqwestHttp::new(
                settings,
                password,
                key_password.as_deref(),
                proxy,
                p.ssh.as_ref().map(|ssh| ssh.target.clone()),
            )
        })
        .await?;
        let http = Arc::new(http);
        self.http
            .lock_safe()
            .insert(profile.id.clone(), (proxy, http.clone()));
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
                tunnels: self.tunnels.clone(),
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
    /// Номер открытия: ответ, пришедший после закрытия или переоткрытия, относится к прошлому открытию.
    generation: u64,
    profile: ConnectionProfile,
    state: ConnState,
    topology: Option<Topology>,
    journal: Journal,
    chosen_member: Option<String>,
    error: Option<AppError>,
    poller: Option<JoinHandle<()>>,
    /// Очередь опросов открытия: фоновый опрос и опрос «Переподключить» идут по одному. Каждый сравнивает
    /// итог со снимком топологии, взятым в начале, — два опроса одного открытия сразу записали бы одно
    /// переключение в журнал дважды, а опрос, закончившийся позже, вернул бы устаревшее состояние.
    observing: Arc<tokio::sync::Mutex<()>>,
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

/// Первое подключение к серверу или списку узлов, где не ответил ни один узел, — это отказ, а не «частично
/// доступно» (спец. P1 §7: `degraded` — недоступна часть участников): ошибка — первого узла, опрос не
/// запускается. Иначе неверный пароль или выключенный сервер давал бы живое подключение, которое каждые 10 с
/// пробовало бы войти снова. У Patroni доступность участника — ответ REST API, и первое подключение без
/// ответа `/cluster` и так ошибка (`discover`).
fn first_connect_failure(profile: &ConnectionProfile, t: &Topology) -> Option<AppError> {
    if profile.kind == ConnectionKind::Patroni || t.members.iter().any(|m| m.reachable) {
        return None;
    }
    Some(
        t.members
            .iter()
            .find_map(|m| m.error.clone())
            .unwrap_or_else(|| {
                app_err!(
                    ErrorCode::NodeUnavailable,
                    "errors.topology.noNode",
                    "No suitable node is available"
                )
            }),
    )
}

fn state_for(t: &Topology) -> ConnState {
    if t.is_degraded() {
        ConnState::Degraded
    } else {
        ConnState::Connected
    }
}

/// Порядок блокировок один, обратного нет — взаимной блокировки нет: замок подключения (`connect_locks`,
/// tokio) → очередь опросов открытия (`Entry::observing`, tokio) → `entries` (std) → пулы, кэш клиентов
/// HTTP, реестр туннелей, `connect_locks`, приёмник событий, слушатели закрытия (std; обработчик слушателя
/// берёт только свои std-мьютексы). `reconnect` берёт оба замка tokio, фоновый опрос — только очередь
/// опросов, `connect` — только замок подключения, `disconnect`, `discard_stale` и `refresh_profile` — ни
/// одного. std-мьютекс не держится через `await`.
pub struct Runtimes {
    source: Arc<dyn TopologySource>,
    pools: Arc<PoolRegistry>,
    /// Туннели ssh подключений: открываются перед первым опросом, закрываются вместе с подключением.
    tunnels: Arc<TunnelRegistry>,
    interval: Duration,
    /// Счётчик номеров открытий (`Entry::generation`).
    generations: AtomicU64,
    entries: Mutex<HashMap<String, Entry>>,
    connect_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    sink: Mutex<Option<Arc<dyn RuntimeSink>>>,
    listeners: Mutex<Vec<Arc<dyn CloseListener>>>,
}

fn not_connected() -> AppError {
    app_err!(
        ErrorCode::NodeUnavailable,
        "errors.topology.notConnected",
        "The connection is not open"
    )
}

/// Срок открытия туннеля: таймаут подключения профиля плюс время на вход на бастион.
fn tunnel_timeout(profile: &ConnectionProfile) -> Duration {
    Duration::from_secs(profile.pg.connect_timeout_s + 5)
}

impl Runtimes {
    pub fn new(
        source: Arc<dyn TopologySource>,
        pools: Arc<PoolRegistry>,
        tunnels: Arc<TunnelRegistry>,
    ) -> Arc<Self> {
        Self::with_interval(source, pools, tunnels, POLL_INTERVAL)
    }

    pub fn with_interval(
        source: Arc<dyn TopologySource>,
        pools: Arc<PoolRegistry>,
        tunnels: Arc<TunnelRegistry>,
        interval: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            source,
            pools,
            tunnels,
            interval,
            generations: AtomicU64::new(0),
            entries: Mutex::new(HashMap::new()),
            connect_locks: Mutex::new(HashMap::new()),
            sink: Mutex::new(None),
            listeners: Mutex::new(Vec::new()),
        })
    }

    pub fn set_sink(&self, sink: Arc<dyn RuntimeSink>) {
        *self.sink.lock_safe() = Some(sink);
    }

    fn sink(&self) -> Option<Arc<dyn RuntimeSink>> {
        self.sink.lock_safe().clone()
    }

    pub fn add_close_listener(&self, listener: Arc<dyn CloseListener>) {
        self.listeners.lock_safe().push(listener);
    }

    /// Слушатели — копией списка: блокировка списка не держится, пока они работают.
    fn listeners(&self) -> Vec<Arc<dyn CloseListener>> {
        self.listeners.lock_safe().clone()
    }

    fn notify_closed(&self, id: &str) {
        for l in self.listeners() {
            l.connection_closed(id);
        }
    }

    fn notify_member_removed(&self, id: &str, member: &str) {
        for l in self.listeners() {
            l.member_removed(id, member);
        }
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

    /// Открывает подключение. Уже открытое с теми же настройками соединения возвращается как есть
    /// (журнал, узел каталога и пулы сохраняются), оформление профиля — имя, цвет, папка — обновляется.
    pub async fn connect(self: &Arc<Self>, profile: ConnectionProfile) -> AppResult<TopologyView> {
        let lock = self.connect_lock(&profile.id);
        let _guard = lock.lock().await;
        self.connect_locked(profile).await
    }

    /// Замок подключения `id`: открытия и переподключения одного подключения идут по очереди.
    fn connect_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.connect_locks
            .lock_safe()
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    /// `connect` под уже взятым замком подключения.
    async fn connect_locked(
        self: &Arc<Self>,
        profile: ConnectionProfile,
    ) -> AppResult<TopologyView> {
        let id = profile.id.clone();
        let generation = {
            let mut entries = self.entries.lock_safe();
            if let Some(e) = entries.get_mut(&id)
                && matches!(e.state, ConnState::Connected | ConnState::Degraded)
                && same_connection_settings(&e.profile, &profile)
            {
                e.profile = profile;
                return Ok(e.view());
            }
            if let Some(old) = entries.remove(&id) {
                if let Some(h) = old.poller {
                    h.abort();
                }
                // Прежнее открытие закончилось: его состояние у слушателей к новому не относится.
                self.notify_closed(&id);
            }
            self.pools.evict_connection(&id);
            self.source.forget(&id);
            let generation = self.generations.fetch_add(1, Ordering::Relaxed) + 1;
            entries.insert(
                id.clone(),
                Entry {
                    generation,
                    profile: profile.clone(),
                    state: ConnState::Connecting,
                    topology: None,
                    journal: Journal::default(),
                    chosen_member: None,
                    error: None,
                    poller: None,
                    observing: Arc::default(),
                },
            );
            self.emit_state(&id, ConnState::Connecting, None);
            generation
        };
        // Туннель — до первого опроса: соединения профиля с ssh идут только через него.
        if profile.ssh.is_some()
            && let Err(err) = self.tunnels.open(&profile, tunnel_timeout(&profile)).await
        {
            let mut entries = self.entries.lock_safe();
            let Some(e) = entries.get_mut(&id).filter(|e| e.generation == generation) else {
                drop(entries);
                return Err(self.discard_stale(&id));
            };
            e.state = ConnState::Failed;
            e.error = Some(err.clone());
            self.emit_state(&id, ConnState::Failed, Some(&err));
            return Err(err);
        }
        let result = self.source.observe(&profile, None).await.and_then(|t| {
            match first_connect_failure(&profile, &t) {
                Some(err) => Err(err),
                None => Ok(t),
            }
        });
        let mut entries = self.entries.lock_safe();
        // Подключение закрыто (или открыто заново), пока шёл опрос: результат устарел, событий не будет.
        let Some(e) = entries.get_mut(&id).filter(|e| e.generation == generation) else {
            drop(entries);
            return Err(self.discard_stale(&id));
        };
        match result {
            Ok(t) => {
                // Таймлайн первого опроса — исходная точка: событие о нём не создаётся.
                e.journal.observe_timeline(t.cluster_timeline());
                e.state = state_for(&t);
                e.topology = Some(t);
                let view = e.view();
                e.poller = Some(self.spawn_poller(&id, generation));
                self.emit_state(&id, view.state, None);
                self.emit_topology(&id, &view, &[]);
                Ok(view)
            }
            Err(err) => {
                e.state = ConnState::Failed;
                e.error = Some(err.clone());
                // Неудачное подключение без опроса: процесс ssh не нужен (повтор откроет туннель заново).
                self.tunnels.close(&id);
                self.emit_state(&id, ConnState::Failed, Some(&err));
                Err(err)
            }
        }
    }

    /// Опрос, закончившийся после закрытия подключения, мог заново создать то, что закрытие убрало
    /// (пулы, клиент HTTP, туннель): убираем это и сообщаем, что подключение уже не открыто. Туннель
    /// закрывается, только если подключение и правда закрыто: опрос прошлого открытия не должен закрыть
    /// туннель нового. Проверка и закрытие — под `entries`: новое открытие сначала записывается туда, а
    /// туннель открывает потом.
    fn discard_stale(&self, id: &str) -> AppError {
        self.pools.evict_connection(id);
        self.source.forget(id);
        let entries = self.entries.lock_safe();
        if !entries.contains_key(id) {
            self.tunnels.close(id);
        }
        not_connected()
    }

    /// Фоновый опрос открытия `generation`: прекращается, как только открытие закрыто или заменено.
    fn spawn_poller(self: &Arc<Self>, id: &str, generation: u64) -> JoinHandle<()> {
        let weak: Weak<Self> = Arc::downgrade(self);
        let (key, interval) = (id.to_string(), self.interval);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(rt) = weak.upgrade() else { break };
                if !rt.poll_once(&key, generation).await {
                    break;
                }
            }
        })
    }

    /// Один опрос открытия `generation`; `false` — оно закрыто или заменено, опрос прекращается.
    async fn poll_once(&self, id: &str, generation: u64) -> bool {
        let observing = {
            let entries = self.entries.lock_safe();
            let Some(e) = entries.get(id).filter(|e| e.generation == generation) else {
                return false;
            };
            e.observing.clone()
        };
        let _turn = observing.lock().await;
        self.observe_entry(id, generation, false).await
    }

    /// Опрос открытия `generation` — общий для фонового опроса и «Переподключить»: те же переходы
    /// состояний (`connected`/`degraded`; неудача опроса — `degraded`, а не `failed`). С `announce`
    /// вид объявляется и без изменений: переподключение — действие пользователя, интерфейс получает
    /// итог целиком. `false` — открытие закрыто или заменено. Только в очереди опросов открытия
    /// (`Entry::observing`): снимок `previous` и запись итога не должны перемежаться с другим опросом.
    async fn observe_entry(&self, id: &str, generation: u64, announce: bool) -> bool {
        let (profile, previous) = {
            let entries = self.entries.lock_safe();
            let Some(e) = entries.get(id).filter(|e| e.generation == generation) else {
                return false;
            };
            (e.profile.clone(), e.topology.clone())
        };
        self.pools.close_idle(IDLE_TIMEOUT);
        let result = self.source.observe(&profile, previous.as_ref()).await;
        let mut entries = self.entries.lock_safe();
        // Подключение закрыто или открыто заново, пока шёл опрос: результат устарел.
        let Some(e) = entries.get_mut(id).filter(|e| e.generation == generation) else {
            drop(entries);
            self.discard_stale(id);
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
                            self.notify_member_removed(id, &m.name);
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
                if state_changed {
                    self.emit_state(id, state, None);
                }
                if changed || !events.is_empty() || announce {
                    self.emit_topology(id, &view, &events);
                }
            }
            Err(err) => {
                e.state = ConnState::Degraded;
                e.error = Some(err.clone());
                self.emit_state(id, ConnState::Degraded, Some(&err));
            }
        }
        true
    }

    /// Закрывает подключение. Всё под одной блокировкой: открытие, начатое сразу после, не увидит
    /// полузакрытого состояния, а событие `Disconnected` не обгонит и не отстанет от изменений.
    pub fn disconnect(&self, id: &str) {
        let mut entries = self.entries.lock_safe();
        if let Some(e) = entries.remove(id) {
            if let Some(h) = e.poller {
                h.abort();
            }
            self.notify_closed(id);
        }
        self.pools.evict_connection(id);
        self.source.forget(id);
        self.tunnels.close(id);
        // Замок подключения нужен только пока кто-то подключается: иначе карта росла бы с каждым id.
        self.connect_locks
            .lock_safe()
            .retain(|k, l| k != id || Arc::strong_count(l) > 1);
        self.emit_state(id, ConnState::Disconnected, None);
    }

    /// «Переподключить» (пароль сменили на сервере, LDAP отказал и снова работает). Открытое подключение
    /// остаётся тем же: журнал, выбранный узел каталога, номер открытия и фоновый опрос сохраняются —
    /// журнал хранит историю переключений, ради которой подключение и держат открытым. Сбрасывается только
    /// то, что держит прежний вход: пулы подключения (с запомненным отказом во входе) и клиент HTTP
    /// Patroni, — и сразу идёт один опрос тем же путём, что и фоновый: секреты читаются из keychain заново.
    /// Неудача этого опроса, как и любого, оставляет подключение `degraded` с опросом, который вернёт его
    /// сам, а не `failed` без опроса. Не открытое (`disconnected`, `failed`) или открытое с другими
    /// настройками соединения подключение закрывается и открывается заново. Под замком подключения:
    /// переподключение не пересекается с идущим `connect`, а дожидается его. Опрос переподключения стоит
    /// в очереди опросов открытия вместе с фоновым: идущий фоновый опрос сначала заканчивается (до сброса
    /// пулов — иначе он, начатый со старым входом, закончился бы позже и вернул бы прежний итог).
    pub async fn reconnect(
        self: &Arc<Self>,
        profile: ConnectionProfile,
    ) -> AppResult<TopologyView> {
        let id = profile.id.clone();
        let lock = self.connect_lock(&id);
        let _guard = lock.lock().await;
        let live = self
            .entries
            .lock_safe()
            .get(&id)
            .filter(|e| {
                matches!(e.state, ConnState::Connected | ConnState::Degraded)
                    && same_connection_settings(&e.profile, &profile)
            })
            .map(|e| (e.generation, e.observing.clone()));
        let Some((generation, observing)) = live else {
            // Замок не теряется: `disconnect` оставляет замок, который кто-то держит.
            self.disconnect(&id);
            return self.connect_locked(profile).await;
        };
        let _turn = observing.lock().await;
        {
            let mut entries = self.entries.lock_safe();
            // Пока ждали очереди, подключение закрыли. Заменить его не могли: замок подключения у нас.
            let Some(e) = entries.get_mut(&id).filter(|e| e.generation == generation) else {
                return Err(not_connected());
            };
            e.profile = profile.clone();
            self.pools.evict_connection(&id);
            self.source.forget(&id);
        }
        // Упавший туннель поднимается сразу, не дожидаясь паузы супервизора; живой не трогается. Неудача
        // перезапуска переподключение не обрывает: опрос ниже упрётся в ту же ошибку туннеля (маршрута нет) и
        // оставит подключение `degraded` с ней, а итог объявится — как у любого неудачного опроса.
        if let Err(e) = self
            .tunnels
            .restart_if_down(&profile, tunnel_timeout(&profile))
            .await
        {
            tracing::warn!(connection = %id, "ssh tunnel restart on reconnect failed: {}", e.message);
        }
        if !self.observe_entry(&id, generation, true).await {
            return Err(not_connected());
        }
        Ok(self.view(&id))
    }

    /// Профиль сохранён с правкой, не затрагивающей соединение (имя, цвет, папка…): открытое подключение
    /// остаётся, профиль в нём обновляется. Если настройки соединения изменились, ничего не делает —
    /// такое подключение закрывают (`disconnect`).
    pub fn refresh_profile(&self, profile: &ConnectionProfile) {
        if let Some(e) = self.entries.lock_safe().get_mut(&profile.id)
            && same_connection_settings(&e.profile, profile)
        {
            e.profile = profile.clone();
        }
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

    /// Узел инструмента диагностики: `cluster` — без узла; `member` — названный участник; иначе «за лидером»
    /// (`observe::leader_member`). Подключение не открыто — `notConnected`; лидера нет — `noNode`.
    pub fn diag_target(
        &self,
        id: &str,
        member: Option<&str>,
        cluster: bool,
    ) -> AppResult<DiagTarget> {
        let entries = self.entries.lock_safe();
        let e = entries.get(id).ok_or_else(not_connected)?;
        let t = e.topology.clone().ok_or_else(not_connected)?;
        let (m, follows_leader) = if cluster {
            (None, false)
        } else if let Some(name) = member {
            let m = t.member(name).cloned().ok_or_else(|| {
                app_err!(
                    ErrorCode::InvalidInput,
                    "errors.topology.unknownMember",
                    "Node {{member}} is not part of the connection",
                    member = name
                )
            })?;
            (Some(m), false)
        } else {
            let m = leader_member(e.profile.kind, &t).cloned().ok_or_else(|| {
                app_err!(
                    ErrorCode::NodeUnavailable,
                    "errors.topology.noNode",
                    "No suitable node is available"
                )
            })?;
            (Some(m), true)
        };
        Ok(DiagTarget {
            profile: e.profile.clone(),
            generation: e.generation,
            topology: t,
            member: m,
            follows_leader,
        })
    }

    /// Останавливает все опросы и туннели (выход из приложения, конец теста). Рантайм не нужен.
    pub fn shutdown(&self) {
        for (_, e) in self.entries.lock_safe().drain() {
            if let Some(h) = e.poller {
                h.abort();
            }
        }
        self.tunnels.close_all_blocking();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Instant;

    use super::*;
    use crate::conn::pool::{PoolKey, PoolPurpose};
    use crate::conn::profile::{ConnectionKind, sample_profile};
    use crate::error::ErrorCode;
    use crate::topology::journal::EventKind;
    use crate::topology::model::{Role, TopologyWarning, member, topo};

    /// Источник, отдающий заранее заданные ответы; последний повторяется.
    struct Scripted {
        answers: Mutex<VecDeque<AppResult<Topology>>>,
        last: Mutex<Option<AppResult<Topology>>>,
        calls: AtomicUsize,
        delay: Mutex<Duration>,
        /// Что происходило: «observe-done» — ответ готов, «forget» — вызван `forget`.
        log: Mutex<Vec<&'static str>>,
        /// Выполняется, когда ответ готов: так опрос, не успевший закончиться до закрытия, заново
        /// создаёт то, что закрытие убрало (пул узла).
        on_finish: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
        /// Туннели, как у `LiveSource`: опрос профиля с ssh начинается с маршрута, упавший туннель — его ошибка.
        tunnels: Mutex<Option<Arc<TunnelRegistry>>>,
    }

    impl Scripted {
        fn new(answers: Vec<AppResult<Topology>>) -> Arc<Self> {
            Self::with_delay(answers, Duration::ZERO)
        }

        fn with_delay(answers: Vec<AppResult<Topology>>, delay: Duration) -> Arc<Self> {
            Arc::new(Self {
                answers: Mutex::new(answers.into()),
                last: Mutex::new(None),
                calls: AtomicUsize::new(0),
                delay: Mutex::new(delay),
                log: Mutex::new(Vec::new()),
                on_finish: Mutex::new(None),
                tunnels: Mutex::new(None),
            })
        }

        /// Следующие ответы вместо оставшихся; прежний «последний» повторяется, пока очередь не опустеет.
        fn then(&self, answers: Vec<AppResult<Topology>>) {
            *self.answers.lock_safe() = answers.into();
        }

        fn set_delay(&self, delay: Duration) {
            *self.delay.lock_safe() = delay;
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl TopologySource for Scripted {
        fn observe<'a>(
            &'a self,
            p: &'a ConnectionProfile,
            _prev: Option<&'a Topology>,
        ) -> BoxFuture<'a, AppResult<Topology>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let tunnels = self.tunnels.lock_safe().clone();
            if let Some(Err(e)) = tunnels.map(|t| t.route_for(p)) {
                return Box::pin(async move { Err(e) });
            }
            let next = self.answers.lock_safe().pop_front();
            let r = match next {
                Some(r) => {
                    *self.last.lock_safe() = Some(r.clone());
                    r
                }
                None => self.last.lock_safe().clone().expect("есть ответ"),
            };
            let delay = *self.delay.lock_safe();
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                self.log.lock_safe().push("observe-done");
                if let Some(f) = self.on_finish.lock_safe().as_ref() {
                    f();
                }
                r.map(|mut t| {
                    t.observed_at_ms = crate::topology::model::now_ms();
                    t
                })
            })
        }

        fn forget(&self, _id: &str) {
            self.log.lock_safe().push("forget");
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

    /// Реестр туннелей без ssh: профили этих тестов без туннеля, файлов реестр не трогает.
    fn no_tunnels(pools: &Arc<PoolRegistry>) -> Arc<TunnelRegistry> {
        crate::net::ssh::tunnel::unused_registry(pools.clone())
    }

    type Fixture = (
        Arc<Runtimes>,
        Arc<Scripted>,
        Arc<Recorder>,
        Arc<PoolRegistry>,
    );

    fn setup_with(interval: Duration, answers: Vec<AppResult<Topology>>) -> Fixture {
        let src = Scripted::new(answers);
        let pools = Arc::new(PoolRegistry::default());
        let rt = Runtimes::with_interval(src.clone(), pools.clone(), no_tunnels(&pools), interval);
        let rec = Arc::new(Recorder::default());
        rt.set_sink(rec.clone());
        (rt, src, rec, pools)
    }

    /// Фоновый опрос идёт каждые 40 мс.
    fn setup(answers: Vec<AppResult<Topology>>) -> Fixture {
        setup_with(Duration::from_millis(40), answers)
    }

    /// Опрос вручную: интервал 60 с, тест сам решает, когда опрашивать, и не гонится с фоновым опросом.
    fn manual(answers: Vec<AppResult<Topology>>) -> Fixture {
        setup_with(Duration::from_secs(60), answers)
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
            purpose: PoolPurpose::Catalog,
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

    #[derive(Default)]
    struct Closed(Mutex<Vec<String>>);
    impl CloseListener for Closed {
        fn connection_closed(&self, id: &str) {
            self.0.lock_safe().push(format!("conn:{id}"));
        }
        fn member_removed(&self, id: &str, member: &str) {
            self.0.lock_safe().push(format!("member:{id}:{member}"));
        }
    }

    #[tokio::test]
    async fn diag_target_follows_the_leader_or_takes_the_named_member() {
        let (rt, _src, _rec, _pools) = manual(vec![Ok(two("b"))]);
        rt.connect(patroni_profile()).await.unwrap();
        let t = rt.diag_target("c", None, false).unwrap();
        assert_eq!(
            (t.member.as_ref().map(|m| m.name.as_str()), t.follows_leader),
            (Some("b"), true)
        );
        let t = rt.diag_target("c", Some("a"), false).unwrap();
        assert_eq!(
            (t.member.as_ref().map(|m| m.name.as_str()), t.follows_leader),
            (Some("a"), false)
        );
        let t = rt.diag_target("c", Some("a"), true).unwrap();
        assert!(t.member.is_none(), "инструмент кластера узла не берёт");
        assert_eq!(
            rt.diag_target("c", Some("zz"), false).unwrap_err().key,
            "errors.topology.unknownMember"
        );
        assert_eq!(
            rt.diag_target("nope", None, false).unwrap_err().key,
            "errors.topology.notConnected"
        );
    }

    #[tokio::test]
    async fn close_listeners_hear_disconnect_reopen_and_removed_members() {
        let (rt, src, _rec, _pools) = setup(vec![Ok(two("b"))]);
        let heard = Arc::new(Closed::default());
        rt.add_close_listener(heard.clone());
        let mut p = patroni_profile();
        rt.connect(p.clone()).await.unwrap();
        // Участник «a» исчез из топологии: следующий фоновый опрос (раз в 40 мс) его не увидит.
        src.then(vec![Ok(topo(vec![member("b", Role::Primary)]))]);
        eventually("участник a убран", || {
            heard.0.lock_safe().contains(&"member:c:a".to_string())
        })
        .await;
        // Переоткрытие с другими настройками соединения и закрытие.
        p.pg.connect_timeout_s += 1;
        rt.connect(p).await.unwrap();
        rt.disconnect("c");
        let log = heard.0.lock_safe().clone();
        assert_eq!(log.iter().filter(|e| *e == "conn:c").count(), 2, "{log:?}");
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
                purpose: PoolPurpose::Catalog,
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
        let calls = src.calls();
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(src.calls(), calls, "опрос остановлен");
        assert_eq!(
            rec.states.lock_safe().last(),
            Some(&ConnState::Disconnected)
        );
    }

    fn forgets(src: &Scripted) -> usize {
        src.log
            .lock_safe()
            .iter()
            .filter(|&&what| what == "forget")
            .count()
    }

    fn is_leader_change(e: &TopologyEvent) -> bool {
        matches!(e.event, EventKind::LeaderChanged { .. })
    }

    /// «Переподключить» живое подключение: журнал, выбранный узел каталога, номер открытия и фоновый опрос
    /// остаются; пулы с отметкой об отказе во входе и клиент HTTP сброшены, узлы опрошены сразу заново.
    #[tokio::test]
    async fn reconnect_of_a_live_connection_keeps_journal_catalog_member_and_polling() {
        let (rt, src, rec, pools) = setup(vec![Ok(two("a")), Ok(two("b"))]);
        rt.connect(patroni_profile()).await.unwrap();
        eventually("переключение в журнале", || {
            rt.events("c").iter().any(is_leader_change)
        })
        .await;
        // Узел каталога выбран вручную: не лидер.
        rt.set_catalog_member("c", Some("a".into())).unwrap();
        let (journal, generation) = (rt.events("c"), generation_of(&rt, "c"));
        add_pool(&pools, "a");
        pools
            .get(&pool_key("a"))
            .unwrap()
            .remember_auth_failure(err());
        let (calls, forgot) = (src.calls(), forgets(&src));
        let announced = rec.topologies.lock_safe().len();

        let view = rt.reconnect(patroni_profile()).await.unwrap();
        assert_eq!(view.state, ConnState::Connected);
        assert_eq!(view.catalog_member.as_deref(), Some("a"));
        assert!(!view.catalog_fallback);
        assert_eq!(rt.events("c"), journal, "журнал не потерян");
        assert_eq!(generation_of(&rt, "c"), generation, "открытие то же");
        assert!(src.calls() > calls, "узлы опрошены заново");
        assert!(
            pools.get(&pool_key("a")).is_none(),
            "пул с запомненным отказом во входе сброшен"
        );
        assert!(forgets(&src) > forgot, "клиент HTTP забыт");
        assert!(
            rec.topologies.lock_safe().len() > announced,
            "итог объявлен, хотя топология не менялась"
        );
        assert!(
            !rec.states.lock_safe().contains(&ConnState::Disconnected),
            "подключение не закрывалось: интерфейс не стирает журнал"
        );
        let after = src.calls();
        eventually("фоновый опрос продолжается", || {
            src.calls() >= after + 2
        })
        .await;
        rt.shutdown();
    }

    /// Узлы недоступны в момент переподключения (Patroni API не отвечает; сервер отверг вход): подключение
    /// остаётся `degraded` с фоновым опросом и возвращается само, а не становится `failed` без опроса, как
    /// при первом открытии.
    #[tokio::test]
    async fn failed_observation_during_reconnect_stays_degraded_and_keeps_polling() {
        let server_up = hosts_topology(ConnectionKind::Server, vec![member("a", Role::Primary)]);
        let server_down = hosts_topology(ConnectionKind::Server, vec![refused("a")]);
        for (kind, up, down) in [
            (ConnectionKind::Patroni, two("a"), Err(err())),
            (ConnectionKind::Server, server_up, Ok(server_down)),
        ] {
            let (rt, src, rec, _) = setup(vec![Ok(up.clone())]);
            rt.connect(profile_of(kind)).await.unwrap();
            let generation = generation_of(&rt, "c");
            // Отказ повторяется, пока тест не подставит следующий ответ.
            src.then(vec![down]);

            let view = rt.reconnect(profile_of(kind)).await.unwrap();
            assert_eq!(view.state, ConnState::Degraded, "{kind:?}");
            assert!(view.topology.is_some(), "{kind:?}: топология остаётся");
            assert_eq!(generation_of(&rt, "c"), generation, "{kind:?}");

            src.then(vec![Ok(up)]);
            eventually("Connected", || rt.view("c").state == ConnState::Connected).await;
            assert!(rt.view("c").error.is_none(), "{kind:?}");
            let states = rec.states.lock_safe().clone();
            assert!(
                !states.contains(&ConnState::Failed) && !states.contains(&ConnState::Disconnected),
                "{kind:?}: {states:?}"
            );
            rt.shutdown();
        }
    }

    /// Не открытое подключение (`failed`) «Переподключить» закрывает и открывает заново, как прежде:
    /// пулы сброшены, состояния прошли через Disconnected, замок подключения после отключения не копится.
    #[tokio::test]
    async fn reconnect_of_a_failed_connection_reopens_it() {
        let (rt, src, rec, pools) = manual(vec![Err(err())]);
        rt.connect(patroni_profile()).await.unwrap_err();
        add_pool(&pools, "a");
        src.then(vec![Ok(two("a"))]);

        let view = rt.reconnect(patroni_profile()).await.unwrap();
        assert_eq!(view.state, ConnState::Connected);
        assert!(pools.keys().is_empty(), "пулы вытеснены");
        assert_eq!(
            *rec.states.lock_safe(),
            vec![
                ConnState::Connecting,
                ConnState::Failed,
                ConnState::Disconnected,
                ConnState::Connecting,
                ConnState::Connected
            ]
        );

        rt.disconnect("c");
        assert!(
            rt.connect_locks.lock_safe().is_empty(),
            "замок удалён вместе с подключением"
        );
    }

    /// Открытое с другими настройками соединения (профиль сохранён, подключение ещё не закрыто) —
    /// переоткрывается: журнал прежнего адреса к новому не относится.
    #[tokio::test]
    async fn reconnect_with_changed_settings_reopens_the_connection() {
        let (rt, src, _rec, _) = manual(vec![Ok(two("a"))]);
        rt.connect(patroni_profile()).await.unwrap();
        let generation = generation_of(&rt, "c");
        src.then(vec![Ok(two("b"))]);
        assert!(rt.poll_once("c", generation).await);
        assert!(!rt.events("c").is_empty());

        rt.reconnect(relocated()).await.unwrap();
        assert_ne!(generation_of(&rt, "c"), generation);
        assert!(rt.events("c").is_empty(), "журнал новый");
        assert_eq!(rt.target("c", None).unwrap().1.hosts[0].host, "elsewhere");
        rt.shutdown();
    }

    /// Переподключение, нажатое во время открытия, дожидается его и опрашивает уже открытое подключение,
    /// а не закрывает его на середине.
    #[tokio::test]
    async fn reconnect_waits_for_a_connect_in_progress() {
        let src = Scripted::with_delay(vec![Ok(two("a"))], Duration::from_millis(100));
        let pools = Arc::new(PoolRegistry::default());
        let rt = Runtimes::with_interval(
            src.clone(),
            pools.clone(),
            no_tunnels(&pools),
            Duration::from_secs(60),
        );
        let rec = Arc::new(Recorder::default());
        rt.set_sink(rec.clone());
        let connect = tokio::spawn({
            let rt = rt.clone();
            async move { rt.connect(patroni_profile()).await }
        });
        eventually("опрос начался", || src.calls() == 1).await;

        let view = rt.reconnect(patroni_profile()).await.unwrap();
        assert_eq!(view.state, ConnState::Connected);
        connect.await.unwrap().unwrap();
        assert_eq!(src.calls(), 2, "открытие и один опрос переподключения");
        assert_eq!(
            *rec.states.lock_safe(),
            vec![ConnState::Connecting, ConnState::Connected]
        );
        rt.shutdown();
    }

    /// «Переподключить» нажато, пока идёт фоновый опрос (в `degraded` недоступный узел держит опрос
    /// секундами), а лидер после прошлого опроса сменился: переключение попадает в журнал и в интерфейс
    /// один раз. Опрос переподключения ждёт идущий и сравнивает уже с тем, что тот записал.
    #[tokio::test]
    async fn reconnect_during_a_poll_records_the_switchover_once() {
        let (rt, src, rec, _) = manual(vec![Ok(two("a"))]);
        rt.connect(patroni_profile()).await.unwrap();
        let generation = generation_of(&rt, "c");
        src.set_delay(Duration::from_millis(150));
        src.then(vec![Ok(two("b"))]);
        let poll = tokio::spawn({
            let rt = rt.clone();
            async move { rt.poll_once("c", generation).await }
        });
        eventually("опрос начался", || src.calls() == 2).await;

        rt.reconnect(patroni_profile()).await.unwrap();
        assert!(poll.await.unwrap());
        let journal = rt.events("c");
        assert_eq!(
            journal.iter().filter(|e| is_leader_change(e)).count(),
            1,
            "одно переключение — одно событие: {journal:?}"
        );
        let once = diff(&two("a"), &two("b")).len();
        assert_eq!(journal.len(), once, "{journal:?}");
        assert_eq!(rec.sent_events().len(), once, "в интерфейс — тоже по разу");
        rt.shutdown();
    }

    /// Опрос, начатый до сброса пулов (ещё с запомненным отказом во входе), не может закончиться после
    /// опроса переподключения: устаревший итог не возвращает `degraded`, а следующий опрос не повторяет
    /// события восстановления.
    #[tokio::test]
    async fn poll_started_before_reconnect_does_not_undo_it() {
        let up = hosts_topology(ConnectionKind::Server, vec![member("a", Role::Primary)]);
        let down = hosts_topology(ConnectionKind::Server, vec![refused("a")]);
        let (rt, src, rec, _) = manual(vec![Ok(up.clone())]);
        rt.connect(profile_of(ConnectionKind::Server))
            .await
            .unwrap();
        let generation = generation_of(&rt, "c");
        src.then(vec![Ok(down.clone())]);
        assert!(rt.poll_once("c", generation).await);
        assert_eq!(rt.view("c").state, ConnState::Degraded);
        let before = rt.events("c").len();
        // Медленный опрос ещё видит отказ во входе…
        src.set_delay(Duration::from_millis(200));
        let poll = tokio::spawn({
            let rt = rt.clone();
            async move { rt.poll_once("c", generation).await }
        });
        eventually("опрос начался", || src.calls() == 3).await;
        // …а вход уже снова работает.
        src.set_delay(Duration::ZERO);
        src.then(vec![Ok(up.clone())]);

        let view = rt
            .reconnect(profile_of(ConnectionKind::Server))
            .await
            .unwrap();
        assert_eq!(view.state, ConnState::Connected);
        assert!(poll.await.unwrap());
        assert_eq!(
            rt.view("c").state,
            ConnState::Connected,
            "устаревший опрос не отменил переподключение"
        );
        assert!(rt.poll_once("c", generation).await);
        let journal = rt.events("c");
        assert_eq!(
            journal.len() - before,
            diff(&down, &up).len(),
            "восстановление записано один раз: {journal:?}"
        );
        assert_eq!(
            *rec.states.lock_safe(),
            vec![
                ConnState::Connecting,
                ConnState::Connected,
                ConnState::Degraded,
                ConnState::Connected
            ]
        );
        rt.shutdown();
    }

    /// Подключение закрыли, пока «Переподключить» ждал очереди за идущим опросом: переподключение
    /// заканчивается `notConnected`, узлы не опрашивает и подключение заново не открывает.
    #[tokio::test]
    async fn disconnect_while_reconnect_waits_for_a_poll() {
        let (rt, src, rec, pools) = manual(vec![Ok(two("a"))]);
        rt.connect(patroni_profile()).await.unwrap();
        let generation = generation_of(&rt, "c");
        let observing = rt.entries.lock_safe()["c"].observing.clone();
        src.set_delay(Duration::from_millis(150));
        let poll = tokio::spawn({
            let rt = rt.clone();
            async move { rt.poll_once("c", generation).await }
        });
        eventually("опрос начался", || src.calls() == 2).await;
        let reconnect = tokio::spawn({
            let rt = rt.clone();
            async move { rt.reconnect(patroni_profile()).await }
        });
        // Очередь опросов держат открытие, тест, идущий опрос и ждущее переподключение.
        eventually("переподключение в очереди", || {
            Arc::strong_count(&observing) == 4
        })
        .await;

        rt.disconnect("c");
        let e = reconnect.await.unwrap().unwrap_err();
        assert_eq!(e.key, "errors.topology.notConnected");
        assert!(
            !poll.await.unwrap(),
            "опрос закрытого подключения прекращается"
        );
        assert_eq!(src.calls(), 2, "переподключение узлы не опрашивало");
        assert_eq!(rt.view("c").state, ConnState::Disconnected);
        assert_eq!(
            rec.states.lock_safe().last(),
            Some(&ConnState::Disconnected)
        );
        assert!(pools.keys().is_empty());
        rt.shutdown();
    }

    /// Замок, который держит идущее открытие, отключение не удаляет: иначе следующее открытие взяло бы новый
    /// замок и пошло бы параллельно с ним.
    #[tokio::test]
    async fn disconnect_keeps_the_lock_of_a_connect_in_progress() {
        let src = Scripted::with_delay(vec![Ok(two("a"))], Duration::from_millis(150));
        let pools = Arc::new(PoolRegistry::default());
        let rt = Runtimes::with_interval(
            src.clone(),
            pools.clone(),
            no_tunnels(&pools),
            Duration::from_secs(60),
        );
        let connect = tokio::spawn({
            let rt = rt.clone();
            async move { rt.connect(patroni_profile()).await }
        });
        eventually("опрос начался", || src.calls() == 1).await;
        rt.disconnect("c");
        assert_eq!(
            rt.connect_locks.lock_safe().len(),
            1,
            "замок идущего открытия остался"
        );
        connect.await.unwrap().unwrap_err();
        rt.disconnect("c");
        assert!(rt.connect_locks.lock_safe().is_empty());
        rt.shutdown();
    }

    /// Узел, который не ответил: отказ во входе.
    fn refused(name: &str) -> Member {
        let mut m = member(name, Role::Unknown);
        m.reachable = false;
        m.state = None;
        m.error = Some(app_err!(
            ErrorCode::AuthFailed,
            "errors.pg.authFailed",
            "Authentication failed: check the user, the password and pg_hba.conf"
        ));
        m
    }

    fn hosts_topology(kind: ConnectionKind, members: Vec<Member>) -> Topology {
        let mut t = topo(members);
        t.kind = kind;
        (t.scope, t.paused) = (None, None);
        t.warnings = t
            .members
            .iter()
            .filter(|m| !m.reachable)
            .map(|m| TopologyWarning::MemberUnreachable {
                name: m.name.clone(),
            })
            .collect();
        t
    }

    fn profile_of(kind: ConnectionKind) -> ConnectionProfile {
        let mut p = sample_profile("c");
        p.kind = kind;
        p
    }

    #[tokio::test]
    async fn server_without_an_answer_fails_on_first_connect_and_is_not_polled() {
        for kind in [ConnectionKind::Server, ConnectionKind::HostList] {
            let mut down = vec![refused("a")];
            if kind == ConnectionKind::HostList {
                let mut tcp = refused("b");
                tcp.error = Some(err());
                down.push(tcp);
            }
            let (rt, src, rec, _) = setup(vec![Ok(hosts_topology(kind, down))]);
            let e = rt.connect(profile_of(kind)).await.unwrap_err();
            assert_eq!(
                (e.code, e.key.as_str()),
                (ErrorCode::AuthFailed, "errors.pg.authFailed"),
                "{kind:?}: ошибка первого узла"
            );
            let v = rt.view("c");
            assert_eq!(v.state, ConnState::Failed, "{kind:?}");
            assert_eq!(
                v.error.as_ref().map(|e| e.key.as_str()),
                Some("errors.pg.authFailed")
            );
            assert!(
                v.topology.is_none() && v.catalog_member.is_none(),
                "{kind:?}"
            );
            // Опрос не запущен: за несколько интервалов (по 40 мс) ни одного нового опроса узлов.
            tokio::time::sleep(Duration::from_millis(250)).await;
            assert_eq!(src.calls(), 1, "{kind:?}: узлы не опрашиваются снова");
            assert_eq!(
                *rec.states.lock_safe(),
                [ConnState::Connecting, ConnState::Failed],
                "{kind:?}"
            );
            assert!(rec.topologies.lock_safe().is_empty());
            assert_eq!(
                rt.target("c", None).unwrap_err().key,
                "errors.topology.notConnected",
                "{kind:?}: каталог и факты с упавшего подключения не читаются"
            );
            rt.shutdown();
        }
    }

    #[tokio::test]
    async fn some_reachable_member_keeps_the_connection_degraded_and_polled() {
        let partly = hosts_topology(
            ConnectionKind::HostList,
            vec![member("a", Role::Primary), refused("b")],
        );
        let (rt, src, _rec, _) = setup(vec![Ok(partly)]);
        let v = rt
            .connect(profile_of(ConnectionKind::HostList))
            .await
            .unwrap();
        assert_eq!(v.state, ConnState::Degraded);
        assert_eq!(v.catalog_member.as_deref(), Some("a"));
        eventually("опрос идёт", || src.calls() >= 3).await;
        rt.shutdown();
    }

    #[tokio::test]
    async fn patroni_members_without_api_answers_keep_the_old_behaviour() {
        // У Patroni доступность участника — ответ REST API; `/cluster` ответил, `/patroni` — нет.
        let mut t = two("a");
        for m in &mut t.members {
            m.reachable = false;
        }
        t.warnings = vec![
            TopologyWarning::MemberUnreachable { name: "a".into() },
            TopologyWarning::MemberUnreachable { name: "b".into() },
        ];
        let (rt, _src, _rec, _) = manual(vec![Ok(t)]);
        let v = rt.connect(patroni_profile()).await.unwrap();
        assert_eq!(v.state, ConnState::Degraded);
        rt.shutdown();
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
        let src = Scripted::with_delay(vec![Ok(two("a"))], Duration::from_millis(50));
        let pools = Arc::new(PoolRegistry::default());
        let rt = Runtimes::with_interval(
            src.clone(),
            pools.clone(),
            no_tunnels(&pools),
            Duration::from_secs(60),
        );
        let (a, b) = tokio::join!(rt.connect(patroni_profile()), rt.connect(patroni_profile()));
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(src.calls(), 1);
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

    fn generation_of(rt: &Runtimes, id: &str) -> u64 {
        rt.entries.lock_safe().get(id).expect("открыто").generation
    }

    /// Профиль с теми же настройками соединения, но другим оформлением.
    fn restyled() -> ConnectionProfile {
        let mut p = patroni_profile();
        p.name = "renamed".into();
        p.color = "#ff0000".into();
        p.environment = crate::conn::profile::Environment::Stage;
        p.folder_id = Some("folder".into());
        p.order = 5;
        p
    }

    /// Профиль с другим адресом: соединение нужно открыть заново.
    fn relocated() -> ConnectionProfile {
        let mut p = patroni_profile();
        p.hosts[0].host = "elsewhere".into();
        p
    }

    #[tokio::test]
    async fn poll_result_for_a_replaced_connection_is_dropped() {
        let (rt, src, rec, _) = manual(vec![Ok(two("a"))]);
        rt.connect(patroni_profile()).await.unwrap();
        let first = generation_of(&rt, "c");
        src.set_delay(Duration::from_millis(150));
        // Опрос идёт, а подключение открывают заново с другими настройками: ответ прежнего открытия устарел.
        src.then(vec![Ok(two("b"))]);
        let poll = tokio::spawn({
            let rt = rt.clone();
            async move { rt.poll_once("c", first).await }
        });
        eventually("опрос начался", || src.calls() == 2).await;
        src.then(vec![Ok(two("a"))]);
        rt.connect(relocated()).await.unwrap();
        assert!(!poll.await.unwrap(), "опрос прекращается");
        assert_ne!(generation_of(&rt, "c"), first);
        assert!(rt.events("c").is_empty());
        assert!(rec.sent_events().is_empty());
        rt.shutdown();
    }

    #[tokio::test]
    async fn cosmetic_change_during_a_poll_keeps_the_poll() {
        let (rt, src, _rec, _) = manual(vec![Ok(two("a"))]);
        rt.connect(patroni_profile()).await.unwrap();
        let generation = generation_of(&rt, "c");
        src.set_delay(Duration::from_millis(150));
        src.then(vec![Ok(two("b"))]);
        let poll = tokio::spawn({
            let rt = rt.clone();
            async move { rt.poll_once("c", generation).await }
        });
        eventually("опрос начался", || src.calls() == 2).await;
        // Переименование идёт мимо опроса: соединение то же, результат опроса принимается.
        rt.connect(restyled()).await.unwrap();
        assert!(poll.await.unwrap(), "опрос продолжается");
        assert_eq!(generation_of(&rt, "c"), generation);
        assert!(
            rt.events("c")
                .iter()
                .any(|e| matches!(e.event, EventKind::LeaderChanged { .. })),
            "результат опроса записан в журнал"
        );
        rt.shutdown();
    }

    #[tokio::test]
    async fn cosmetic_reconnect_keeps_journal_catalog_member_and_pools() {
        let (rt, src, rec, pools) = manual(vec![Ok(two("a"))]);
        rt.connect(patroni_profile()).await.unwrap();
        let generation = generation_of(&rt, "c");
        add_pool(&pools, "a");
        src.then(vec![Ok(two("b"))]);
        assert!(rt.poll_once("c", generation).await);
        let events_before = rt.events("c");
        assert!(
            events_before
                .iter()
                .any(|e| matches!(e.event, EventKind::LeaderChanged { .. })),
            "в журнале переключение лидера"
        );
        // Узел каталога выбран вручную: не лидер.
        rt.set_catalog_member("c", Some("a".into())).unwrap();
        let (calls, states) = (src.calls(), rec.states.lock_safe().len());

        // Переименование, цвет, папка и порядок (перетаскивание в дереве) соединение не трогают.
        let view = rt.connect(restyled()).await.unwrap();
        assert_eq!(view.state, ConnState::Connected);
        assert_eq!(view.catalog_member.as_deref(), Some("a"));
        assert_eq!(rt.events("c"), events_before, "журнал не потерян");
        assert_eq!(generation_of(&rt, "c"), generation);
        assert!(pools.get(&pool_key("a")).is_some(), "пулы на месте");
        assert_eq!(src.calls(), calls, "узлы не опрашивались заново");
        assert_eq!(
            rec.states.lock_safe().len(),
            states,
            "состояние не менялось и не объявлялось"
        );
        let (_, profile) = rt.target("c", None).unwrap();
        assert_eq!(
            (profile.name.as_str(), profile.order),
            ("renamed", 5),
            "оформление профиля обновлено"
        );
        rt.shutdown();
    }

    #[tokio::test]
    async fn changed_connection_settings_reopen_the_connection() {
        let (rt, src, rec, pools) = manual(vec![Ok(two("a"))]);
        rt.connect(patroni_profile()).await.unwrap();
        let generation = generation_of(&rt, "c");
        add_pool(&pools, "a");
        src.then(vec![Ok(two("b"))]);
        assert!(rt.poll_once("c", generation).await);
        rt.set_catalog_member("c", Some("a".into())).unwrap();
        assert!(!rt.events("c").is_empty());
        let calls = src.calls();
        src.then(vec![Ok(two("a"))]);

        let view = rt.connect(relocated()).await.unwrap();
        assert_eq!(src.calls(), calls + 1, "узлы опрошены заново");
        assert_ne!(generation_of(&rt, "c"), generation);
        assert!(rt.events("c").is_empty(), "журнал новый");
        assert_eq!(
            view.catalog_member.as_deref(),
            Some("a"),
            "узел каталога — по умолчанию (лидер)"
        );
        assert!(!view.catalog_fallback);
        assert!(pools.keys().is_empty(), "пулы прежнего открытия закрыты");
        assert!(
            rec.states
                .lock_safe()
                .ends_with(&[ConnState::Connecting, ConnState::Connected])
        );
        rt.shutdown();
    }

    #[tokio::test]
    async fn refresh_profile_updates_cosmetics_only() {
        let (rt, _src, _rec, _) = manual(vec![Ok(two("a"))]);
        rt.connect(patroni_profile()).await.unwrap();
        rt.refresh_profile(&relocated());
        assert_eq!(rt.target("c", None).unwrap().1.hosts[0].host, "localhost");
        rt.refresh_profile(&restyled());
        assert_eq!(rt.target("c", None).unwrap().1.name, "renamed");
        // Чужого подключения нет — ничего не происходит.
        let mut other = patroni_profile();
        other.id = "nope".into();
        rt.refresh_profile(&other);
        assert_eq!(rt.view("nope").state, ConnState::Disconnected);
        rt.shutdown();
    }

    #[tokio::test]
    async fn disconnect_during_a_slow_connect_leaves_no_trace() {
        let src = Scripted::with_delay(vec![Ok(two("a"))], Duration::from_millis(150));
        let pools = Arc::new(PoolRegistry::default());
        let rt = Runtimes::with_interval(
            src.clone(),
            pools.clone(),
            no_tunnels(&pools),
            Duration::from_secs(60),
        );
        let rec = Arc::new(Recorder::default());
        rt.set_sink(rec.clone());
        // Опрос, не успевший до закрытия, создаёт пул уже после того, как закрытие их убрало.
        *src.on_finish.lock_safe() = Some(Box::new({
            let pools = pools.clone();
            move || add_pool(&pools, "a")
        }));

        let connect = tokio::spawn({
            let rt = rt.clone();
            async move { rt.connect(patroni_profile()).await }
        });
        eventually("опрос начался", || src.calls() == 1).await;
        rt.disconnect("c");
        let e = connect.await.unwrap().unwrap_err();
        assert_eq!(e.key, "errors.topology.notConnected");

        assert_eq!(
            *rec.states.lock_safe(),
            vec![ConnState::Connecting, ConnState::Disconnected],
            "после Disconnected событий нет"
        );
        assert!(rec.topologies.lock_safe().is_empty());
        assert!(pools.keys().is_empty(), "пул, созданный опросом, закрыт");
        assert_eq!(
            src.log.lock_safe().last(),
            Some(&"forget"),
            "кэш источника забыт и после завершения опроса"
        );
        assert_eq!(rt.view("c").state, ConnState::Disconnected);
        rt.shutdown();
    }

    #[tokio::test]
    async fn disconnect_during_a_failing_slow_connect_leaves_no_trace() {
        let src = Scripted::with_delay(vec![Err(err())], Duration::from_millis(150));
        let pools = Arc::new(PoolRegistry::default());
        let rt = Runtimes::with_interval(
            src.clone(),
            pools.clone(),
            no_tunnels(&pools),
            Duration::from_secs(60),
        );
        let rec = Arc::new(Recorder::default());
        rt.set_sink(rec.clone());
        *src.on_finish.lock_safe() = Some(Box::new({
            let pools = pools.clone();
            move || add_pool(&pools, "a")
        }));

        let connect = tokio::spawn({
            let rt = rt.clone();
            async move { rt.connect(patroni_profile()).await }
        });
        eventually("опрос начался", || src.calls() == 1).await;
        rt.disconnect("c");
        let e = connect.await.unwrap().unwrap_err();
        assert_eq!(
            e.key, "errors.topology.notConnected",
            "ошибка опроса уже никому не нужна"
        );

        assert_eq!(
            *rec.states.lock_safe(),
            vec![ConnState::Connecting, ConnState::Disconnected],
            "Failed после Disconnected не приходит"
        );
        assert!(pools.keys().is_empty());
        assert_eq!(src.log.lock_safe().last(), Some(&"forget"));
        assert_eq!(rt.view("c").state, ConnState::Disconnected);
        rt.shutdown();
    }

    /// Приёмник, который на `Connected` останавливается, пока тест не разрешит продолжить.
    struct Gate {
        states: Mutex<Vec<ConnState>>,
        entered: Mutex<mpsc::Sender<()>>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl RuntimeSink for Gate {
        fn state_changed(&self, _id: &str, state: ConnState, _e: Option<&AppError>) {
            if state == ConnState::Connected {
                self.entered.lock_safe().send(()).unwrap();
                let _ = self
                    .release
                    .lock_safe()
                    .recv_timeout(Duration::from_secs(5));
            }
            self.states.lock_safe().push(state);
        }
        fn topology_changed(&self, _id: &str, _v: &TopologyView, _events: &[TopologyEvent]) {}
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disconnect_cannot_overtake_a_notification_in_progress() {
        let (rt, _src, _rec, _pools) = manual(vec![Ok(two("a"))]);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let gate = Arc::new(Gate {
            states: Mutex::new(Vec::new()),
            entered: Mutex::new(entered_tx),
            release: Mutex::new(release_rx),
        });
        rt.set_sink(gate.clone());

        let connect = tokio::spawn({
            let rt = rt.clone();
            async move { rt.connect(patroni_profile()).await }
        });
        // Тест ждёт блокирующе, на своём потоке: рабочий поток runtime занят остановленным приёмником.
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("Connected объявляется");
        // Закрытие приходит, пока объявляется `Connected`: оно ждёт, а не обгоняет, — иначе интерфейс
        // получил бы `Connected` после `Disconnected` и показал бы закрытое подключение открытым.
        let disconnect = std::thread::spawn({
            let rt = rt.clone();
            move || rt.disconnect("c")
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(!disconnect.is_finished(), "закрытие ждёт объявления");
        release_tx.send(()).unwrap();
        disconnect.join().unwrap();
        connect.await.unwrap().unwrap();
        assert_eq!(
            *gate.states.lock_safe(),
            vec![
                ConnState::Connecting,
                ConnState::Connected,
                ConnState::Disconnected
            ]
        );
        rt.shutdown();
    }

    #[tokio::test]
    async fn poll_finishing_after_disconnect_emits_nothing_and_cleans_up() {
        let (rt, src, rec, pools) = manual(vec![Ok(two("a"))]);
        rt.connect(patroni_profile()).await.unwrap();
        let generation = generation_of(&rt, "c");
        let (states, topologies) = (
            rec.states.lock_safe().len(),
            rec.topologies.lock_safe().len(),
        );
        src.set_delay(Duration::from_millis(150));
        src.then(vec![Ok(two("b"))]);
        *src.on_finish.lock_safe() = Some(Box::new({
            let pools = pools.clone();
            move || add_pool(&pools, "a")
        }));
        let poll = tokio::spawn({
            let rt = rt.clone();
            async move { rt.poll_once("c", generation).await }
        });
        eventually("опрос начался", || src.calls() == 2).await;
        rt.disconnect("c");
        assert!(!poll.await.unwrap());
        assert_eq!(
            rec.states.lock_safe()[states..],
            [ConnState::Disconnected],
            "после Disconnected событий нет"
        );
        assert_eq!(rec.topologies.lock_safe().len(), topologies);
        assert!(pools.keys().is_empty());
        assert!(rt.events("c").is_empty());
        rt.shutdown();
    }

    /// Рантайм с поддельным ssh: профили с туннелем (`tunnel::tests::profile`, id `c1`).
    fn with_fake_ssh(
        dir: &std::path::Path,
        env: Vec<(&str, String)>,
        answers: Vec<AppResult<Topology>>,
    ) -> (
        Arc<Runtimes>,
        Arc<Scripted>,
        Arc<Recorder>,
        Arc<TunnelRegistry>,
    ) {
        let (tunnels, pools) = crate::net::ssh::tunnel::tests::registry(dir, env);
        let src = Scripted::new(answers);
        *src.tunnels.lock_safe() = Some(tunnels.clone());
        let rt =
            Runtimes::with_interval(src.clone(), pools, tunnels.clone(), Duration::from_secs(60));
        let rec = Arc::new(Recorder::default());
        rt.set_sink(rec.clone());
        (rt, src, rec, tunnels)
    }

    fn ssh_profile() -> ConnectionProfile {
        let mut p =
            crate::net::ssh::tunnel::tests::profile(crate::conn::profile::SshAuth::Password);
        p.kind = ConnectionKind::Patroni;
        p
    }

    /// Туннель не поднялся (вход на бастион отклонён): подключение `failed` с ошибкой ssh, узлы не опрашиваются.
    #[tokio::test]
    async fn tunnel_failure_fails_the_connection() {
        let dir = tempfile::tempdir().unwrap();
        let (rt, src, rec, tunnels) = with_fake_ssh(
            dir.path(),
            vec![("FAKE_SSH_MODE", "auth".into())],
            vec![Ok(two("a"))],
        );
        let e = rt.connect(ssh_profile()).await.unwrap_err();
        assert_eq!(e.key, "errors.ssh.authFailed");
        assert_eq!(rt.view("c1").state, ConnState::Failed);
        assert_eq!(rt.view("c1").error.unwrap().key, "errors.ssh.authFailed");
        assert_eq!(
            *rec.states.lock_safe(),
            [ConnState::Connecting, ConnState::Failed]
        );
        assert_eq!(src.calls(), 0, "без туннеля узлы не опрашиваются");
        rt.disconnect("c1");
        assert_eq!(tunnels.status("c1"), None);
        rt.shutdown();
    }

    /// Туннель открывается до первого опроса и закрывается вместе с подключением.
    #[tokio::test]
    async fn tunnel_lives_as_long_as_the_connection() {
        let dir = tempfile::tempdir().unwrap();
        let (rt, _src, _rec, tunnels) = with_fake_ssh(dir.path(), vec![], vec![Ok(two("a"))]);
        rt.connect(ssh_profile()).await.unwrap();
        assert!(matches!(
            tunnels.status("c1"),
            Some(crate::net::ssh::tunnel::TunnelStatus::Up { .. })
        ));
        rt.disconnect("c1");
        assert_eq!(tunnels.status("c1"), None);
        rt.shutdown();
    }

    /// «Переподключить», когда туннель не поднимается (вход на бастион отклонён после смены пароля): подключение
    /// остаётся открытым, но `degraded` с ошибкой туннеля, и итог объявлен — как у любого неудачного опроса
    /// переподключения.
    #[tokio::test]
    async fn reconnect_with_a_tunnel_that_cannot_restart_degrades_the_connection() {
        let dir = tempfile::tempdir().unwrap();
        let refuse = dir.path().join("refuse");
        let (rt, _src, rec, tunnels) = with_fake_ssh(
            dir.path(),
            vec![
                (
                    "FAKE_SSH_DIE_MS",
                    crate::net::ssh::tunnel::tests::DIE_MS.into(),
                ),
                (
                    "FAKE_SSH_AUTH_FAIL_IF",
                    refuse.to_string_lossy().into_owned(),
                ),
            ],
            vec![Ok(two("a"))],
        );
        rt.connect(ssh_profile()).await.unwrap();
        assert_eq!(rt.view("c1").state, ConnState::Connected);
        std::fs::write(&refuse, "1").unwrap();
        eventually("туннель упал", || {
            matches!(
                tunnels.status("c1"),
                Some(crate::net::ssh::tunnel::TunnelStatus::Down { .. })
            )
        })
        .await;
        let states = rec.states.lock_safe().len();
        let view = rt.reconnect(ssh_profile()).await.unwrap();
        assert_eq!(view.state, ConnState::Degraded);
        assert_eq!(view.error.unwrap().key, "errors.ssh.authFailed");
        assert_eq!(rec.states.lock_safe()[states..], [ConnState::Degraded]);
        assert_eq!(rt.view("c1").state, ConnState::Degraded);
        rt.shutdown();
    }

    /// Неудачное первое подключение (узлы не ответили) не держит процесс ssh.
    #[tokio::test]
    async fn failed_first_connect_closes_its_tunnel() {
        let dir = tempfile::tempdir().unwrap();
        let (rt, _src, _rec, tunnels) = with_fake_ssh(dir.path(), vec![], vec![Err(err())]);
        rt.connect(ssh_profile()).await.unwrap_err();
        assert_eq!(rt.view("c1").state, ConnState::Failed);
        assert_eq!(tunnels.status("c1"), None);
        rt.shutdown();
    }

    /// Опрос прошлого открытия, закончившийся после переоткрытия, не закрывает туннель нового открытия.
    #[tokio::test]
    async fn stale_poll_does_not_close_the_tunnel_of_the_new_opening() {
        let dir = tempfile::tempdir().unwrap();
        let (rt, src, _rec, tunnels) = with_fake_ssh(dir.path(), vec![], vec![Ok(two("a"))]);
        rt.connect(ssh_profile()).await.unwrap();
        let generation = generation_of(&rt, "c1");
        src.set_delay(Duration::from_millis(300));
        let poll = tokio::spawn({
            let rt = rt.clone();
            async move { rt.poll_once("c1", generation).await }
        });
        eventually("опрос начался", || src.calls() == 2).await;
        rt.disconnect("c1");
        src.set_delay(Duration::ZERO);
        rt.connect(ssh_profile()).await.unwrap();
        assert!(!poll.await.unwrap(), "опрос прошлого открытия устарел");
        assert!(
            matches!(
                tunnels.status("c1"),
                Some(crate::net::ssh::tunnel::TunnelStatus::Up { .. })
            ),
            "туннель нового открытия жив"
        );
        assert_eq!(rt.view("c1").state, ConnState::Connected);
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
