//! Пул соединений по ключу (подключение, участник, БД): не больше 3 соединений, проверка при выдаче,
//! закрытие простаивающих (спец. P1 §7). Свой пул вместо deadpool-postgres — см. «Отступления» плана P1a.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::AbortHandle;
use tokio_postgres::Client;

use crate::conn::pg::ConnectSpec;
use crate::error::{AppError, AppResult};
use crate::sync::MutexExt;

pub const MAX_CONNECTIONS: usize = 3;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Соединение, простоявшее дольше, перед выдачей проверяется пустым запросом.
const VALIDATE_AFTER: Duration = Duration::from_secs(30);

/// Соединение вместе с задачей, которая его обслуживает. При drop задача прерывается: так закрывается
/// и молчащее соединение (см. `ConnectSpec::connect_with_handle`), которое drop одного `Client` не
/// закрыл бы. Здоровое простаивающее соединение закрывается так же — сокет, без сообщения Terminate;
/// сервер видит конец потока между командами и закрывает сеанс без записи в журнал.
struct PgConn {
    client: Client,
    abort: AbortHandle,
}

impl Drop for PgConn {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

pub struct PgPool {
    spec: ConnectSpec,
    idle: Mutex<Vec<(PgConn, Instant)>>,
    permits: Arc<Semaphore>,
    /// Порог простоя, после которого соединение проверяется при выдаче (`VALIDATE_AFTER`, в тестах — меньше).
    validate_after: Duration,
}

/// Что делать с простаивающим соединением при выдаче.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reuse {
    /// Простояло не меньше `IDLE_TIMEOUT`: закрыть, не выдавать — правило «5 минут» не зависит от частоты обхода.
    Discard,
    /// Простояло не меньше порога проверки: выдать только после пустого запроса.
    Validate,
    /// Свежее соединение: выдать как есть.
    AsIs,
}

fn reuse_decision(idle: Duration, validate_after: Duration) -> Reuse {
    if idle >= IDLE_TIMEOUT {
        Reuse::Discard
    } else if idle >= validate_after {
        Reuse::Validate
    } else {
        Reuse::AsIs
    }
}

/// Соединение из пула; при drop живое соединение возвращается в пул.
pub struct PooledClient {
    conn: Option<PgConn>,
    pool: Arc<PgPool>,
    _permit: OwnedSemaphorePermit,
}

impl PooledClient {
    /// Соединение негодно (таймаут, обрыв, молчащий путь): прерывает его задачу и не возвращает в пул.
    /// Обычного drop здесь мало — запрос без ответа не даёт tokio-postgres закрыть соединение.
    pub fn discard(mut self) {
        self.conn.take();
    }
}

impl Deref for PooledClient {
    type Target = Client;
    fn deref(&self) -> &Client {
        &self
            .conn
            .as_ref()
            .expect("connection is present until drop")
            .client
    }
}

impl Drop for PooledClient {
    fn drop(&mut self) {
        // Закрытое соединение здесь же и освобождается (с прерыванием уже завершённой задачи).
        if let Some(c) = self.conn.take()
            && !c.client.is_closed()
        {
            self.pool.idle.lock_safe().push((c, Instant::now()));
        }
    }
}

impl PgPool {
    pub fn new(spec: ConnectSpec) -> Arc<Self> {
        Self::with_validate_after(spec, VALIDATE_AFTER)
    }

    /// Только для тестов: пул с другим порогом проверки при выдаче (`Duration::ZERO` — проверять всегда).
    /// Рабочий код использует `new` с порогом в 30 с.
    #[doc(hidden)]
    pub fn with_validate_after(spec: ConnectSpec, validate_after: Duration) -> Arc<Self> {
        Arc::new(Self {
            spec,
            idle: Mutex::new(Vec::new()),
            permits: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
            validate_after,
        })
    }

    pub fn spec(&self) -> &ConnectSpec {
        &self.spec
    }

    /// Свободное соединение или новое; при занятых трёх — ждёт освобождения.
    pub async fn get(self: &Arc<Self>) -> AppResult<PooledClient> {
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AppError::internal_with("connection pool is closed"))?;
        while let Some((conn, since)) = self.pop_idle() {
            if conn.client.is_closed() {
                continue;
            }
            match reuse_decision(since.elapsed(), self.validate_after) {
                // Устаревшее и не прошедшее проверку соединение закрывается при drop `conn`.
                Reuse::Discard => continue,
                Reuse::AsIs => {}
                // Проверка с таймаутом подключения: на молчащем пути (без RST и keepalive) пустой запрос
                // иначе висит минутами, удерживая один из трёх permit. Ошибка и таймаут — соединение мёртво.
                Reuse::Validate => {
                    match tokio::time::timeout(self.spec.timeout, conn.client.simple_query(""))
                        .await
                    {
                        Ok(Ok(_)) => {}
                        Ok(Err(_)) | Err(_) => continue,
                    }
                }
            }
            return Ok(PooledClient {
                conn: Some(conn),
                pool: self.clone(),
                _permit: permit,
            });
        }
        let (client, abort) = self.spec.connect_with_handle().await?;
        Ok(PooledClient {
            conn: Some(PgConn { client, abort }),
            pool: self.clone(),
            _permit: permit,
        })
    }

    fn pop_idle(&self) -> Option<(PgConn, Instant)> {
        self.idle.lock_safe().pop()
    }

    /// Закрывает соединения, простоявшие не меньше `max_idle` (их задачи прерываются при drop);
    /// возвращает, сколько закрыто.
    pub fn close_idle(&self, max_idle: Duration) -> usize {
        let mut idle = self.idle.lock_safe();
        let before = idle.len();
        idle.retain(|(_, since)| since.elapsed() < max_idle);
        before - idle.len()
    }

    pub fn idle_count(&self) -> usize {
        self.idle.lock_safe().len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PoolKey {
    pub connection_id: String,
    pub member: String,
    pub database: String,
}

#[derive(Default)]
pub struct PoolRegistry {
    pools: Mutex<HashMap<PoolKey, Arc<PgPool>>>,
}

impl PoolRegistry {
    pub fn get(&self, key: &PoolKey) -> Option<Arc<PgPool>> {
        self.pools.lock_safe().get(key).cloned()
    }

    /// Пул по ключу; если его нет — новый из `spec` (успевший раньше поток выигрывает).
    pub fn insert_if_absent(&self, key: PoolKey, spec: ConnectSpec) -> Arc<PgPool> {
        self.pools
            .lock_safe()
            .entry(key)
            .or_insert_with(|| PgPool::new(spec))
            .clone()
    }

    pub fn evict(&self, key: &PoolKey) {
        self.pools.lock_safe().remove(key);
    }

    pub fn evict_connection(&self, connection_id: &str) {
        self.pools
            .lock_safe()
            .retain(|k, _| k.connection_id != connection_id);
    }

    pub fn evict_member(&self, connection_id: &str, member: &str) {
        self.pools
            .lock_safe()
            .retain(|k, _| !(k.connection_id == connection_id && k.member == member));
    }

    pub fn close_idle(&self, max_idle: Duration) {
        for p in self.pools.lock_safe().values() {
            p.close_idle(max_idle);
        }
    }

    pub fn keys(&self) -> Vec<PoolKey> {
        self.pools.lock_safe().keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::SslMode;
    use crate::conn::testutil;

    fn spec() -> ConnectSpec {
        ConnectSpec {
            tcp_host: "h".into(),
            tcp_port: 5432,
            host: "h".into(),
            port: 5432,
            user: "u".into(),
            password: None,
            database: "postgres".into(),
            ssl_mode: SslMode::Disable,
            connector: None,
            timeout: Duration::from_secs(1),
        }
    }

    fn key(c: &str, m: &str, d: &str) -> PoolKey {
        PoolKey {
            connection_id: c.into(),
            member: m.into(),
            database: d.into(),
        }
    }

    #[test]
    fn same_key_same_pool() {
        let r = PoolRegistry::default();
        let a = r.insert_if_absent(key("c", "m", "d"), spec());
        let b = r.insert_if_absent(key("c", "m", "d"), spec());
        assert!(Arc::ptr_eq(&a, &b));
        assert!(Arc::ptr_eq(&a, &r.get(&key("c", "m", "d")).unwrap()));
        assert!(r.get(&key("c", "m", "other")).is_none());
    }

    #[test]
    fn eviction_by_connection_and_member() {
        let r = PoolRegistry::default();
        // Слабые ссылки: после вытеснения пул никто не держит и он должен быть освобождён.
        let weak = |c: &str, m: &str| Arc::downgrade(&r.insert_if_absent(key(c, m, "d"), spec()));
        let (w_c1m1, w_c1m2, w_c2m1) = (weak("c1", "m1"), weak("c1", "m2"), weak("c2", "m1"));
        assert!(
            w_c1m1.upgrade().is_some() && w_c1m2.upgrade().is_some() && w_c2m1.upgrade().is_some()
        );

        r.evict_member("c1", "m1");
        let mut keys = r.keys();
        keys.sort_by(|a, b| (&a.connection_id, &a.member).cmp(&(&b.connection_id, &b.member)));
        assert_eq!(keys, vec![key("c1", "m2", "d"), key("c2", "m1", "d")]);
        assert!(w_c1m1.upgrade().is_none(), "вытесненный пул освобождён");
        assert!(w_c1m2.upgrade().is_some() && w_c2m1.upgrade().is_some());

        r.evict_connection("c1");
        assert_eq!(r.keys(), vec![key("c2", "m1", "d")]);
        assert!(
            w_c1m2.upgrade().is_none(),
            "пул вытесненного подключения освобождён"
        );
        assert!(w_c2m1.upgrade().is_some());

        r.evict(&key("c2", "m1", "d"));
        assert!(r.keys().is_empty());
        assert!(
            w_c2m1.upgrade().is_none(),
            "пул, вытесненный по ключу, освобождён"
        );
    }

    #[test]
    fn reuse_decision_by_idle_time() {
        let s = Duration::from_secs;
        // Порог проверки по умолчанию — 30 с, жизнь простаивающего соединения — 5 мин.
        assert_eq!(reuse_decision(s(0), VALIDATE_AFTER), Reuse::AsIs);
        assert_eq!(reuse_decision(s(29), VALIDATE_AFTER), Reuse::AsIs);
        assert_eq!(reuse_decision(s(30), VALIDATE_AFTER), Reuse::Validate);
        assert_eq!(reuse_decision(s(299), VALIDATE_AFTER), Reuse::Validate);
        assert_eq!(reuse_decision(IDLE_TIMEOUT, VALIDATE_AFTER), Reuse::Discard);
        assert_eq!(reuse_decision(s(3600), VALIDATE_AFTER), Reuse::Discard);
        // Порог проверки ноль (тесты): проверяется всё, кроме устаревшего.
        assert_eq!(reuse_decision(s(0), Duration::ZERO), Reuse::Validate);
        assert_eq!(reuse_decision(IDLE_TIMEOUT, Duration::ZERO), Reuse::Discard);
        // Даже с порогом выше IDLE_TIMEOUT устаревшее соединение не выдаётся.
        assert_eq!(reuse_decision(IDLE_TIMEOUT, s(600)), Reuse::Discard);
    }

    #[test]
    fn new_uses_default_validate_after() {
        assert_eq!(PgPool::new(spec()).validate_after, Duration::from_secs(30));
    }

    /// Спецификация для заглушки без ответов на запросы (`testutil::stalling_pg_server`).
    fn stalling_spec(port: u16, timeout: Duration) -> ConnectSpec {
        ConnectSpec {
            tcp_host: "127.0.0.1".into(),
            tcp_port: port,
            timeout,
            ..spec()
        }
    }

    /// Запрос ушёл на сервер, ответа нет. Пул об этом не знает: соединение «живое».
    async fn stall(c: &PooledClient) {
        tokio::time::timeout(Duration::from_millis(200), c.simple_query("SELECT 1"))
            .await
            .expect_err("сервер не отвечает");
    }

    const WAIT: Duration = Duration::from_secs(2);

    #[tokio::test]
    async fn discard_aborts_the_connection_and_keeps_it_out_of_idle() {
        let server = testutil::stalling_pg_server();
        let pool = PgPool::new(stalling_spec(server.port, Duration::from_secs(1)));
        let c = pool.get().await.unwrap();
        stall(&c).await;
        c.discard();
        assert!(server.wait_closed(1, WAIT).await, "сокет закрыт");
        assert_eq!(
            pool.idle_count(),
            0,
            "негодное соединение не вернулось в пул"
        );
        // Permit освобождён: пул выдаёт новое соединение.
        let again = tokio::time::timeout(WAIT, pool.get())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(server.accepted(), 2);
        drop(again);
        assert_eq!(
            pool.idle_count(),
            1,
            "здоровое соединение возвращается в пул"
        );
    }

    #[tokio::test]
    async fn closing_idle_connections_aborts_their_tasks() {
        let server = testutil::stalling_pg_server();
        let pool = PgPool::new(stalling_spec(server.port, Duration::from_secs(1)));
        let c = pool.get().await.unwrap();
        stall(&c).await;
        drop(c);
        assert_eq!(pool.idle_count(), 1);
        assert_eq!(pool.close_idle(Duration::ZERO), 1);
        assert!(
            server.wait_closed(1, WAIT).await,
            "close_idle закрывает сокет"
        );
    }

    #[tokio::test]
    async fn eviction_aborts_idle_connections() {
        let server = testutil::stalling_pg_server();
        let registry = PoolRegistry::default();
        let k = key("c", "m", "d");
        let pool = registry.insert_if_absent(
            k.clone(),
            stalling_spec(server.port, Duration::from_secs(1)),
        );
        let c = pool.get().await.unwrap();
        stall(&c).await;
        drop(c);
        drop(pool);
        registry.evict(&k);
        assert!(
            server.wait_closed(1, WAIT).await,
            "вытеснение пула закрывает сокет"
        );
    }

    #[tokio::test]
    async fn failed_validation_aborts_the_stalled_connection() {
        let server = testutil::stalling_pg_server();
        let pool = PgPool::with_validate_after(
            stalling_spec(server.port, Duration::from_millis(300)),
            Duration::ZERO,
        );
        let c = pool.get().await.unwrap();
        stall(&c).await;
        drop(c);
        // Проверка при выдаче упирается в таймаут; сервер молчит и на новом соединении, но оно
        // открывается: рукопожатие заглушка проходит.
        let fresh = tokio::time::timeout(WAIT, pool.get())
            .await
            .unwrap()
            .unwrap();
        assert!(
            server.wait_closed(1, WAIT).await,
            "проверенное и негодное соединение закрыто"
        );
        assert_eq!(server.accepted(), 2);
        drop(fresh);
    }
}
