//! Реестр сэмплеров (спец. P2 §6): задача на узел, пока подписана хоть одна вкладка; буфер — до закрытия подключения
//! или исчезновения участника.
//!
//! Порядок блокировок. `nodes` — std-мьютекс, держится без `await` и без вызова чужого кода: подписчикам события
//! шлются после того, как блокировка отпущена, реестр сессий (`SessionRegistry::forget`) — тоже вне её.
//! `unsubscribe` вызывается из остановки сессии (`SessionRegistry` её зовёт вне своей блокировки), слушатель
//! закрытия — под блокировкой `entries` `Runtimes`; к `Runtimes` реестр не обращается. Вложенных блокировок
//! `nodes` и реестра сессий нет ни в каком порядке.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::Serialize;
use tokio::task::{AbortHandle, Id};

use crate::error::{AppError, AppResult, ErrorCode};
use crate::sampler::buffer::{Buffer, RawSample};
use crate::session::SessionRegistry;
use crate::sync::MutexExt;
use crate::topology::runtime::CloseListener;

/// Событие канала вкладки: `tick` — раз в секунду (индикатор «живой»), `gap` — промежуток в истории, `error` —
/// сэмпл не удался (сэмплер продолжает с паузой), `stopped` — сэмплер узла остановлен не вкладкой (отказ во
/// входе, подключение закрыто, участник исчез).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SamplerEvent {
    #[serde(rename_all = "camelCase")]
    Tick {
        at_ms: i64,
        active: u32,
        waiting: u32,
    },
    #[serde(rename_all = "camelCase")]
    Gap {
        from_ms: i64,
        to_ms: i64,
    },
    Error {
        error: AppError,
    },
    Stopped,
}

/// Канал вкладки. `false` — канал закрыт (вкладка закрыта, вебвью перезагружен): подписка снимается.
pub trait SamplerSink: Send + Sync {
    fn send(&self, e: SamplerEvent) -> bool;
}

/// Откуда берутся сэмплы: в приложении — запрос к узлу (`query::PoolSampleSource`), в тестах — подмена.
pub trait SampleSource: Send + Sync {
    fn sample<'a>(&'a self) -> BoxFuture<'a, AppResult<Vec<RawSample>>>;

    /// Предел одного сэмпла целиком (подключение и запрос), которым источник ограничивает себя сам и по
    /// которому закрывает соединение. Задача сэмплера ждёт на секунду дольше — только как страховка.
    fn limit(&self) -> Duration {
        SAMPLE_TIMEOUT
    }
}

/// Паузы после неудачных сэмплов подряд, секунды.
const BACKOFF_S: [u64; 5] = [1, 2, 5, 10, 30];
/// Предел самого запроса сэмпла (без подключения): молчащий узел не держит задачу.
pub(crate) const SAMPLE_TIMEOUT: Duration = Duration::from_secs(5);
/// Запас внешнего предела над `SampleSource::limit`. Источник сам ограничен своим пределом и по таймауту
/// закрывает соединение (`on_catalog_node_within`); внешний предел — страховка для источника без своего, и он
/// не должен сработать раньше: брошенный на полуслове запрос вернул бы соединение в пул с недочитанным ответом.
const SAMPLE_TIMEOUT_GRACE: Duration = Duration::from_secs(1);

type Key = (String, String);

struct Node {
    /// Номер открытия подключения (`Entry::generation`): буфер прошлого открытия не продолжается.
    generation: u64,
    buffer: Arc<Mutex<Buffer>>,
    /// Подписки вкладок: id сессии → канал.
    subscribers: HashMap<String, Arc<dyn SamplerSink>>,
    /// Задача сэмплера; `None` — не идёт.
    task: Option<AbortHandle>,
}

impl Node {
    fn new(generation: u64) -> Self {
        Self {
            generation,
            buffer: Arc::new(Mutex::new(Buffer::default())),
            subscribers: HashMap::new(),
            task: None,
        }
    }

    /// Задача `id` — текущая задача узла. Прерванная задача может ещё дойти до конца синхронного участка
    /// (прерывание срабатывает на `await`): ни событий, ни изменений узла от неё быть не должно.
    fn runs(&self, id: Id) -> bool {
        self.task.as_ref().is_some_and(|t| t.id() == id)
    }
}

pub struct SamplerRegistry {
    nodes: Mutex<HashMap<Key, Node>>,
    interval: Duration,
    sessions: Arc<SessionRegistry>,
}

impl SamplerRegistry {
    pub fn new(interval: Duration, sessions: Arc<SessionRegistry>) -> Arc<Self> {
        Arc::new(Self {
            nodes: Mutex::new(HashMap::new()),
            interval,
            sessions,
        })
    }

    /// Подписка вкладки на сэмплер узла: узла нет или он от более раннего открытия подключения — новый буфер;
    /// задача сэмплера запускается, если не идёт. Подписка от более раннего открытия, чем у узла, отклоняется:
    /// узел не трогается, вкладке — `Stopped`, её сессия забывается. Нужен рантайм tokio (`tokio::spawn`).
    pub fn subscribe(
        self: &Arc<Self>,
        connection: &str,
        generation: u64,
        member: &str,
        session_id: &str,
        sink: Arc<dyn SamplerSink>,
        source: Arc<dyn SampleSource>,
    ) {
        let key = (connection.to_string(), member.to_string());
        let stale = {
            let mut nodes = self.nodes.lock_safe();
            if nodes.get(&key).is_some_and(|n| n.generation > generation) {
                drop(nodes);
                // Подписка запоздала: подключение уже переоткрыто, и узел принадлежит новому открытию.
                sink.send(SamplerEvent::Stopped);
                self.sessions.forget(session_id);
                return;
            }
            let node = nodes
                .entry(key.clone())
                .or_insert_with(|| Node::new(generation));
            let stale = (node.generation != generation)
                .then(|| std::mem::replace(node, Node::new(generation)));
            node.subscribers.insert(session_id.to_string(), sink);
            if node.task.is_none() {
                let (weak, buffer) = (Arc::downgrade(self), node.buffer.clone());
                node.task = Some(
                    tokio::spawn(run(weak, key, source, buffer, self.interval)).abort_handle(),
                );
            }
            stale
        };
        self.stop_nodes(stale.into_iter().collect());
    }

    /// Убирает узел открытия `generation`, если подписчиков у него не осталось (`true` — убран): так
    /// отменённый запуск не оставляет узел с буфером закрытого подключения. Узел другого открытия или с
    /// подписчиками не трогается; задача сэмплера пустого узла уже снята отпиской.
    pub fn forget_empty_node(&self, connection: &str, member: &str, generation: u64) -> bool {
        let mut nodes = self.nodes.lock_safe();
        let key = (connection.to_string(), member.to_string());
        let gone = nodes
            .get(&key)
            .is_some_and(|n| n.generation == generation && n.subscribers.is_empty());
        if gone
            && let Some(n) = nodes.remove(&key)
            && let Some(t) = n.task
        {
            t.abort();
        }
        gone
    }

    /// Отписка вкладки; последняя — задача прерывается, буфер остаётся.
    pub fn unsubscribe(&self, connection: &str, member: &str, session_id: &str) {
        let mut nodes = self.nodes.lock_safe();
        if let Some(n) = nodes.get_mut(&(connection.to_string(), member.to_string())) {
            n.subscribers.remove(session_id);
            if n.subscribers.is_empty()
                && let Some(t) = n.task.take()
            {
                t.abort();
            }
        }
    }

    pub fn buffer(&self, connection: &str, member: &str) -> Option<Arc<Mutex<Buffer>>> {
        self.nodes
            .lock_safe()
            .get(&(connection.to_string(), member.to_string()))
            .map(|n| n.buffer.clone())
    }

    pub fn is_running(&self, connection: &str, member: &str) -> bool {
        self.nodes
            .lock_safe()
            .get(&(connection.to_string(), member.to_string()))
            .is_some_and(|n| n.task.is_some())
    }

    /// Рассылка подписчикам от задачи `me`; закрытые каналы отписываются (и забываются в реестре сессий).
    /// Без подписчиков задача снимается с узла — `false`; задача уже не текущая — тоже `false`, без рассылки.
    fn broadcast(&self, key: &Key, me: Id, e: &SamplerEvent) -> bool {
        let sinks: Vec<(String, Arc<dyn SamplerSink>)> = {
            let nodes = self.nodes.lock_safe();
            match nodes.get(key).filter(|n| n.runs(me)) {
                Some(n) => n
                    .subscribers
                    .iter()
                    .map(|(id, s)| (id.clone(), s.clone()))
                    .collect(),
                None => return false,
            }
        };
        let closed: Vec<String> = sinks
            .into_iter()
            .filter(|(_, s)| !s.send(e.clone()))
            .map(|(id, _)| id)
            .collect();
        let alive = {
            let mut nodes = self.nodes.lock_safe();
            match nodes.get_mut(key).filter(|n| n.runs(me)) {
                Some(n) => {
                    for id in &closed {
                        n.subscribers.remove(id);
                    }
                    // Решение «подписчиков нет — задача снята» — под той же блокировкой, что и подписка:
                    // вкладка, подписавшаяся сразу после, запустит новую задачу, а не повиснет без неё.
                    if n.subscribers.is_empty() {
                        n.task = None;
                    }
                    n.task.is_some()
                }
                None => false,
            }
        };
        for id in &closed {
            self.sessions.forget(id);
        }
        alive
    }

    /// Задача `me` остановилась сама (отказ во входе): узел без задачи, подписчикам — `Stopped`, их сессии
    /// закончены. Буфер остаётся до закрытия подключения.
    fn finished(&self, key: &Key, me: Id) {
        let subscribers = {
            let mut nodes = self.nodes.lock_safe();
            match nodes.get_mut(key).filter(|n| n.runs(me)) {
                Some(n) => {
                    n.task = None;
                    std::mem::take(&mut n.subscribers)
                }
                None => return,
            }
        };
        self.stop_subscribers(subscribers);
    }

    fn stop_subscribers(&self, subscribers: HashMap<String, Arc<dyn SamplerSink>>) {
        for (id, sink) in subscribers {
            sink.send(SamplerEvent::Stopped);
            self.sessions.forget(&id);
        }
    }

    /// Узлы удалены из реестра: задачи прерываются, подписчикам — `Stopped`.
    fn stop_nodes(&self, gone: Vec<Node>) {
        for n in gone {
            if let Some(t) = n.task {
                t.abort();
            }
            self.stop_subscribers(n.subscribers);
        }
    }
}

/// Закрытие подключения и исчезновение участника: задачи прерываются, буферы удаляются, подписчикам —
/// `Stopped`. Регистрируется в `Runtimes` раньше реестра сессий: иначе остановка сессий успела бы отписать
/// вкладки, и `Stopped` никто бы не получил.
impl CloseListener for SamplerRegistry {
    fn connection_closed(&self, connection_id: &str) {
        let gone: Vec<Node> = {
            let mut nodes = self.nodes.lock_safe();
            let keys: Vec<Key> = nodes
                .keys()
                .filter(|k| k.0 == connection_id)
                .cloned()
                .collect();
            keys.iter().filter_map(|k| nodes.remove(k)).collect()
        };
        self.stop_nodes(gone);
    }

    fn member_removed(&self, connection_id: &str, member: &str) {
        let gone = self
            .nodes
            .lock_safe()
            .remove(&(connection_id.to_string(), member.to_string()));
        self.stop_nodes(gone.into_iter().collect());
    }
}

/// Задача сэмплера узла: тик раз в `interval`, сэмпл, буфер, события подписчикам.
async fn run(
    weak: Weak<SamplerRegistry>,
    key: Key,
    source: Arc<dyn SampleSource>,
    buffer: Arc<Mutex<Buffer>>,
    interval: Duration,
) {
    let me = tokio::task::id();
    let mut tick = tokio::time::interval(interval);
    // Запрос дольше интервала: пропущенные тики не догоняются — запросы не накладываются.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut failures = 0usize;
    loop {
        tick.tick().await;
        // Предел источника может быть больше `SAMPLE_TIMEOUT` (подключение по медленной сети), внешний — за ним.
        let limit = source.limit();
        let result = tokio::time::timeout(limit + SAMPLE_TIMEOUT_GRACE, source.sample()).await;
        let failed = !matches!(result, Ok(Ok(_)));
        let Some(reg) = weak.upgrade() else { return };
        let alive = match result {
            Ok(Ok(rows)) => {
                failures = 0;
                // Время сэмпла — часы приложения (уточнение 10 плана P2a): тики, промежутки и окно отчёта
                // считаются одними часами.
                let at_ms = crate::topology::model::now_ms();
                let active = rows.len() as u32;
                let waiting = rows
                    .iter()
                    .filter(|r| r.wait_type.as_deref() == Some("Lock"))
                    .count() as u32;
                let gap = buffer.lock_safe().push(at_ms, rows);
                let mut alive = true;
                if let Some((from_ms, to_ms)) = gap {
                    alive &= reg.broadcast(&key, me, &SamplerEvent::Gap { from_ms, to_ms });
                }
                alive
                    && reg.broadcast(
                        &key,
                        me,
                        &SamplerEvent::Tick {
                            at_ms,
                            active,
                            waiting,
                        },
                    )
            }
            Ok(Err(e)) if e.code == ErrorCode::AuthFailed => {
                // Вход отвергнут: повтор подбирал бы пароль (LDAP, fail2ban) — сэмплер останавливается.
                reg.broadcast(&key, me, &SamplerEvent::Error { error: e });
                reg.finished(&key, me);
                return;
            }
            Ok(Err(e)) => reg.broadcast(&key, me, &SamplerEvent::Error { error: e }),
            Err(_) => reg.broadcast(
                &key,
                me,
                &SamplerEvent::Error {
                    error: crate::topology::probe::no_answer(limit),
                },
            ),
        };
        if !alive {
            reg.finished(&key, me);
            return;
        }
        if failed {
            drop(reg);
            tokio::time::sleep(Duration::from_secs(
                BACKOFF_S[failures.min(BACKOFF_S.len() - 1)],
            ))
            .await;
            failures += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake(Mutex<Vec<AppResult<Vec<RawSample>>>>, AtomicUsize);
    impl SampleSource for Fake {
        fn sample<'a>(&'a self) -> BoxFuture<'a, AppResult<Vec<RawSample>>> {
            Box::pin(async move {
                self.1.fetch_add(1, Ordering::SeqCst);
                let mut q = self.0.lock_safe();
                if q.len() > 1 {
                    q.remove(0)
                } else {
                    q[0].clone()
                }
            })
        }
    }

    #[derive(Default)]
    struct Sink(Mutex<Vec<SamplerEvent>>, std::sync::atomic::AtomicBool);
    impl SamplerSink for Sink {
        fn send(&self, e: SamplerEvent) -> bool {
            self.0.lock_safe().push(e);
            !self.1.load(Ordering::SeqCst)
        }
    }

    fn reg() -> Arc<SamplerRegistry> {
        SamplerRegistry::new(
            Duration::from_millis(20),
            Arc::new(SessionRegistry::default()),
        )
    }

    #[tokio::test]
    async fn one_task_per_node_stops_with_the_last_subscriber() {
        let r = reg();
        let src = Arc::new(Fake(
            Mutex::new(vec![Ok(vec![RawSample {
                pid: 1,
                wait_type: Some("Lock".into()),
                ..Default::default()
            }])]),
            AtomicUsize::new(0),
        ));
        let (a, b) = (Arc::new(Sink::default()), Arc::new(Sink::default()));
        r.subscribe("c", 1, "m", "s1", a.clone(), src.clone());
        r.subscribe("c", 1, "m", "s2", b.clone(), src.clone());
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(r.is_running("c", "m"));
        r.unsubscribe("c", "m", "s1");
        assert!(r.is_running("c", "m"), "второй подписчик держит сэмплер");
        r.unsubscribe("c", "m", "s2");
        let calls = src.1.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!r.is_running("c", "m"));
        assert!(
            src.1.load(Ordering::SeqCst) <= calls + 1,
            "запросы прекратились"
        );
        assert!(
            !r.buffer("c", "m").unwrap().lock_safe().is_empty(),
            "буфер живёт до закрытия подключения"
        );
        assert!(
            a.0.lock_safe()
                .iter()
                .any(|e| matches!(e, SamplerEvent::Tick { waiting: 1, .. }))
        );
        r.connection_closed("c");
        assert!(r.buffer("c", "m").is_none());
    }

    #[tokio::test]
    async fn auth_failure_stops_and_closed_sink_unsubscribes() {
        let r = reg();
        let denied = app_err!(
            ErrorCode::AuthFailed,
            "errors.pg.authFailed",
            "Authentication failed: check the user, the password and pg_hba.conf"
        );
        let src = Arc::new(Fake(Mutex::new(vec![Err(denied)]), AtomicUsize::new(0)));
        let sink = Arc::new(Sink::default());
        r.subscribe("c", 1, "m", "s1", sink.clone(), src);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!r.is_running("c", "m"));
        assert!(matches!(
            sink.0.lock_safe().last(),
            Some(SamplerEvent::Stopped)
        ));

        let ok = Arc::new(Fake(Mutex::new(vec![Ok(vec![])]), AtomicUsize::new(0)));
        let gone = Arc::new(Sink::default());
        gone.1.store(true, Ordering::SeqCst);
        r.subscribe("c", 1, "n", "s2", gone, ok);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!r.is_running("c", "n"), "закрытый канал — отписка");
    }

    #[tokio::test(start_paused = true)]
    async fn query_errors_pause_and_the_sampler_goes_on() {
        let r = reg();
        let failed = crate::topology::probe::no_answer(Duration::from_secs(5));
        let src = Arc::new(Fake(
            Mutex::new(vec![Err(failed), Ok(vec![])]),
            AtomicUsize::new(0),
        ));
        let sink = Arc::new(Sink::default());
        r.subscribe("c", 1, "m", "s1", sink.clone(), src.clone());
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(src.1.load(Ordering::SeqCst), 1, "пауза 1 с после ошибки");
        assert!(matches!(
            sink.0.lock_safe()[..],
            [SamplerEvent::Error { .. }]
        ));
        tokio::time::sleep(Duration::from_millis(1000)).await;
        assert!(r.is_running("c", "m"));
        assert!(
            sink.0
                .lock_safe()
                .iter()
                .any(|e| matches!(e, SamplerEvent::Tick { .. }))
        );
    }

    #[tokio::test]
    async fn closing_stops_subscribers_and_ends_their_sessions() {
        use crate::session::SessionKind;
        let sessions = Arc::new(SessionRegistry::default());
        let r = SamplerRegistry::new(Duration::from_millis(20), sessions.clone());
        let src = Arc::new(Fake(Mutex::new(vec![Ok(vec![])]), AtomicUsize::new(0)));
        let sinks: Vec<Arc<Sink>> = (0..4).map(|_| Arc::new(Sink::default())).collect();
        for (sid, member, sink) in [
            ("s1", "m", &sinks[0]),
            ("s2", "n", &sinks[1]),
            ("s4", "k", &sinks[3]),
        ] {
            sessions.insert(sid.into(), "c", SessionKind::Sampler, Box::new(|| {}));
            r.subscribe("c", 1, member, sid, sink.clone(), src.clone());
        }
        sinks[3].1.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(!sessions.stop("s4"), "закрытый канал — сессия забыта");

        r.member_removed("c", "n");
        assert!(r.buffer("c", "n").is_none() && r.buffer("c", "m").is_some());
        assert!(matches!(
            sinks[1].0.lock_safe().last(),
            Some(SamplerEvent::Stopped)
        ));
        assert!(!sessions.stop("s2"), "сессия ушедшего узла закончена");

        // Подключение переоткрыто: буфер прошлого открытия не продолжается, его подписчикам — `Stopped`.
        let old = r.buffer("c", "m").unwrap();
        r.subscribe("c", 2, "m", "s3", sinks[2].clone(), src.clone());
        assert!(matches!(
            sinks[0].0.lock_safe().last(),
            Some(SamplerEvent::Stopped)
        ));
        assert!(!Arc::ptr_eq(&old, &r.buffer("c", "m").unwrap()));
        assert!(r.is_running("c", "m"));

        r.connection_closed("c");
        assert!(matches!(
            sinks[2].0.lock_safe().last(),
            Some(SamplerEvent::Stopped)
        ));
        assert!(!r.is_running("c", "m") && r.buffer("c", "m").is_none());
        let n = sinks[2].0.lock_safe().len();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            sinks[2].0.lock_safe().len(),
            n,
            "после закрытия событий нет"
        );
    }

    #[tokio::test]
    async fn subscribe_with_an_older_generation_is_refused_and_leaves_the_node() {
        use crate::session::SessionKind;
        let sessions = Arc::new(SessionRegistry::default());
        let r = SamplerRegistry::new(Duration::from_millis(20), sessions.clone());
        let src = Arc::new(Fake(Mutex::new(vec![Ok(vec![])]), AtomicUsize::new(0)));
        let (fresh, late) = (Arc::new(Sink::default()), Arc::new(Sink::default()));
        for sid in ["new", "late"] {
            sessions.insert(sid.into(), "c", SessionKind::Sampler, Box::new(|| {}));
        }
        r.subscribe("c", 2, "m", "new", fresh.clone(), src.clone());
        let buffer = r.buffer("c", "m").unwrap();

        r.subscribe("c", 1, "m", "late", late.clone(), src.clone());
        assert_eq!(
            late.0.lock_safe()[..],
            [SamplerEvent::Stopped],
            "запоздавшей вкладке — только `Stopped`"
        );
        assert!(!sessions.stop("late"), "её сессия забыта");
        assert!(
            Arc::ptr_eq(&buffer, &r.buffer("c", "m").unwrap()) && r.is_running("c", "m"),
            "узел нового открытия не тронут"
        );
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(late.0.lock_safe().len(), 1, "тиков запоздавшей вкладке нет");
        {
            let events = fresh.0.lock_safe();
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, SamplerEvent::Tick { .. }))
            );
            assert!(!events.iter().any(|e| matches!(e, SamplerEvent::Stopped)));
        }

        // Пустой узел убирается только для своего открытия.
        assert!(!r.forget_empty_node("c", "m", 2), "подписчик ещё есть");
        r.unsubscribe("c", "m", "new");
        assert!(!r.forget_empty_node("c", "m", 1), "узел другого открытия");
        assert!(r.buffer("c", "m").is_some());
        assert!(r.forget_empty_node("c", "m", 2));
        assert!(r.buffer("c", "m").is_none() && !r.is_running("c", "m"));
        assert!(!r.forget_empty_node("c", "m", 2), "узла уже нет");
    }

    /// Источник с собственным пределом больше `SAMPLE_TIMEOUT` (подключение по медленной сети): сэмпл дольше
    /// 5 с, но в пределе источника — не ошибка; медленный тик пропускается, запросы не накладываются.
    struct Slow {
        limit: Duration,
        takes: Duration,
        running: AtomicUsize,
        overlaps: AtomicUsize,
        calls: AtomicUsize,
    }
    impl SampleSource for Slow {
        fn sample<'a>(&'a self) -> BoxFuture<'a, AppResult<Vec<RawSample>>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if self.running.fetch_add(1, Ordering::SeqCst) > 0 {
                    self.overlaps.fetch_add(1, Ordering::SeqCst);
                }
                tokio::time::sleep(self.takes).await;
                self.running.fetch_sub(1, Ordering::SeqCst);
                Ok(vec![])
            })
        }
        fn limit(&self) -> Duration {
            self.limit
        }
    }

    #[tokio::test(start_paused = true)]
    async fn slow_link_sample_within_the_source_limit_is_not_cut_and_ticks_never_overlap() {
        let r = SamplerRegistry::new(Duration::from_secs(1), Arc::new(SessionRegistry::default()));
        let src = Arc::new(Slow {
            limit: Duration::from_secs(25),
            takes: Duration::from_secs(8),
            running: AtomicUsize::new(0),
            overlaps: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        });
        let sink = Arc::new(Sink::default());
        r.subscribe("c", 1, "m", "s1", sink.clone(), src.clone());
        tokio::time::sleep(Duration::from_secs(30)).await;
        let events = sink.0.lock_safe().clone();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, SamplerEvent::Error { .. })),
            "8 с в пределе источника (25 с): ошибки нет, {events:?}"
        );
        let ticks = events
            .iter()
            .filter(|e| matches!(e, SamplerEvent::Tick { .. }))
            .count();
        assert!((3..=4).contains(&ticks), "тик каждые ~8 с: {ticks}");
        assert_eq!(
            src.calls.load(Ordering::SeqCst),
            ticks + usize::from(src.running.load(Ordering::SeqCst) > 0)
        );
        assert_eq!(
            src.overlaps.load(Ordering::SeqCst),
            0,
            "запросы не накладываются"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn silent_source_is_cut_by_the_outer_guard_past_its_limit() {
        let r = SamplerRegistry::new(Duration::from_secs(1), Arc::new(SessionRegistry::default()));
        let src = Arc::new(Slow {
            limit: Duration::from_secs(3),
            takes: Duration::from_secs(60),
            running: AtomicUsize::new(0),
            overlaps: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        });
        let sink = Arc::new(Sink::default());
        r.subscribe("c", 1, "m", "s1", sink.clone(), src);
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            sink.0.lock_safe().is_empty(),
            "до предела источника и запаса тишина"
        );
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let events = sink.0.lock_safe().clone();
        assert!(
            matches!(&events[..], [SamplerEvent::Error { error }] if error.params["secs"] == "3"),
            "{events:?}"
        );
    }

    #[test]
    fn event_json_is_tagged_camel_case() {
        let v = |e| serde_json::to_value(e).unwrap();
        assert_eq!(
            v(SamplerEvent::Tick {
                at_ms: 5,
                active: 2,
                waiting: 1
            }),
            serde_json::json!({"kind": "tick", "atMs": 5, "active": 2, "waiting": 1})
        );
        assert_eq!(
            v(SamplerEvent::Gap {
                from_ms: 1,
                to_ms: 2
            }),
            serde_json::json!({"kind": "gap", "fromMs": 1, "toMs": 2})
        );
        assert_eq!(
            v(SamplerEvent::Stopped),
            serde_json::json!({"kind": "stopped"})
        );
        let e = v(SamplerEvent::Error {
            error: crate::topology::probe::no_answer(Duration::from_secs(5)),
        });
        assert_eq!(
            (&e["kind"], &e["error"]["code"]),
            (&"error".into(), &"CONNECTION_FAILED".into())
        );
    }
}
