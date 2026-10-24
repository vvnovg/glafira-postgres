//! Пул соединений по ключу (подключение, участник, БД, назначение): не больше 3 соединений (у пула опроса
//! топологии — одно), проверка при выдаче, закрытие простаивающих (спец. P1 §7). Свой пул вместо
//! deadpool-postgres — см. «Отступления» плана P1a.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::runtime::Handle;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::AbortHandle;
use tokio_postgres::Client;

use crate::conn::errors::login_not_retried;
use crate::conn::pg::ConnectSpec;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::sync::MutexExt;

pub const MAX_CONNECTIONS: usize = 3;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Соединение, простоявшее дольше, перед выдачей проверяется пустым запросом.
const VALIDATE_AFTER: Duration = Duration::from_secs(30);

/// Назначение пула. Опрос топологии получает свой пул из одного соединения: иначе страницы каталога
/// занимали бы все три места, и опрос ждал бы их дольше своего предела — ложное `memberUnreachable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PoolPurpose {
    Catalog,
    Probe,
}

impl PoolPurpose {
    fn max_connections(self) -> usize {
        match self {
            Self::Catalog => MAX_CONNECTIONS,
            Self::Probe => 1,
        }
    }
}

/// Соединение вместе с задачей, которая его обслуживает.
///
/// Здоровое соединение закрывается штатно: drop `Client` заставляет tokio-postgres отправить
/// Terminate и закрыть сокет, и сервер не считает сеанс оборванным (`sessions_abandoned` в
/// `pg_stat_database`, строка «unexpected EOF» в журнале при TLS). Если ответа на запрос ждут, Terminate
/// не уйдёт никогда, поэтому после `grace` задача всё равно прерывается. Негодное соединение (таймаут,
/// обрыв, неудачная проверка) закрывается сразу — `PgConn::kill`.
struct PgConn {
    client: Client,
    abort: AbortHandle,
    /// Runtime, в котором работает задача соединения: drop может случиться в потоке без runtime
    /// (например, синхронная команда Tauri), а срок до принудительного закрытия должен отсчитываться всё равно.
    runtime: Handle,
    /// Сколько задача соединения ещё живёт после drop клиента (по умолчанию — таймаут подключения).
    grace: Duration,
    /// Закрыть сразу, не дожидаясь Terminate.
    immediate: bool,
}

impl PgConn {
    /// Соединение негодно: ответы могут не прийти никогда, Terminate не отправить. Задача прерывается сразу.
    fn kill(mut self) {
        self.immediate = true;
    }
}

impl Drop for PgConn {
    fn drop(&mut self) {
        // Закрытое соединение ждать нечего: задача уже закончилась или будет прервана сейчас.
        if self.immediate || self.client.is_closed() {
            self.abort.abort();
            return;
        }
        // `client` освобождается сразу после этой функции — тогда и уходит Terminate;
        // страховка закрывает соединение, которое не смогло его отправить.
        let (abort, grace) = (self.abort.clone(), self.grace);
        self.runtime.spawn(async move {
            tokio::time::sleep(grace).await;
            abort.abort();
        });
    }
}

pub struct PgPool {
    spec: ConnectSpec,
    idle: Mutex<Vec<(PgConn, Instant)>>,
    permits: Arc<Semaphore>,
    /// Порог простоя, после которого соединение проверяется при выдаче (`VALIDATE_AFTER`, в тестах — меньше).
    validate_after: Duration,
    /// Срок штатного закрытия соединения (`PgConn::grace`).
    close_grace: Duration,
    /// Сервер отверг вход (`AUTH_FAILED`): новых попыток входа пул больше не делает и сразу отдаёт эту
    /// ошибку. Иначе фоновый опрос и обновление обзора раз в 10 с подбирали бы пароль заново — с LDAP, PAM
    /// или fail2ban это блокирует учётную запись или адрес. Отметка живёт, пока жив пул: его вытесняют
    /// переоткрытие подключения, сохранение профиля или секретов (`Runtimes::disconnect`) и смена адреса узла.
    auth_failure: Mutex<Option<AppError>>,
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
    /// Соединение негодно (таймаут, обрыв, молчащий путь): сразу прерывает его задачу и не возвращает в пул.
    /// Обычного drop здесь мало — запрос без ответа не даёт tokio-postgres закрыть соединение.
    pub fn discard(mut self) {
        if let Some(c) = self.conn.take() {
            c.kill();
        }
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

    /// Пул не больше чем на `max` соединений (пул опроса топологии — на одно).
    pub fn with_max(spec: ConnectSpec, max: usize) -> Arc<Self> {
        let close_grace = spec.timeout;
        Self::build(spec, VALIDATE_AFTER, close_grace, max)
    }

    /// Только для тестов: пул с другим порогом проверки при выдаче (`Duration::ZERO` — проверять всегда).
    /// Рабочий код использует `new` с порогом в 30 с.
    #[doc(hidden)]
    pub fn with_validate_after(spec: ConnectSpec, validate_after: Duration) -> Arc<Self> {
        let close_grace = spec.timeout;
        Self::build(spec, validate_after, close_grace, MAX_CONNECTIONS)
    }

    fn build(
        spec: ConnectSpec,
        validate_after: Duration,
        close_grace: Duration,
        max: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            spec,
            idle: Mutex::new(Vec::new()),
            permits: Arc::new(Semaphore::new(max)),
            validate_after,
            close_grace,
            auth_failure: Mutex::new(None),
        })
    }

    pub fn spec(&self) -> &ConnectSpec {
        &self.spec
    }

    /// Пул открывает соединения с этим адресом узла.
    pub fn looks_at(&self, host: &str, port: u16) -> bool {
        self.spec.host == host && self.spec.port == port
    }

    /// Пул открывает соединения с этим адресом узла этим путём: напрямую (`None`) или через этот порт туннеля.
    /// Туннель перезапускается на новом порту — пул старого порта не годится.
    pub fn routes_to(&self, host: &str, port: u16, socks: Option<std::net::SocketAddr>) -> bool {
        self.looks_at(host, port) && self.spec.socks == socks
    }

    /// Отказ во входе, после которого пул новых соединений не открывает.
    pub fn auth_failure(&self) -> Option<AppError> {
        self.auth_failure.lock_safe().clone()
    }

    /// Свободное соединение или новое; когда все места заняты (три, у пула опроса — одно), ждёт освобождения.
    /// Если сервер уже отверг вход, новое соединение не открывается (`auth_failure`); уже открытые, пока
    /// живы, выдаются как обычно.
    pub async fn get(self: &Arc<Self>) -> AppResult<PooledClient> {
        if let Some(e) = self.auth_failure()
            && self.idle_count() == 0
        {
            return Err(e);
        }
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
                // Устаревшее здоровое соединение закрывается штатно (drop `conn`).
                Reuse::Discard => continue,
                Reuse::AsIs => {}
                // Проверка с таймаутом подключения: на молчащем пути (без RST и keepalive) пустой запрос
                // иначе висит минутами, удерживая permit пула (у пула опроса топологии он единственный).
                // Ошибка и таймаут — соединение мёртво, ответ на пустой запрос может прийти когда угодно:
                // закрываем сразу.
                Reuse::Validate => {
                    match tokio::time::timeout(self.spec.timeout, conn.client.simple_query(""))
                        .await
                    {
                        Ok(Ok(_)) => {}
                        Ok(Err(_)) | Err(_) => {
                            conn.kill();
                            continue;
                        }
                    }
                }
            }
            return Ok(PooledClient {
                conn: Some(conn),
                pool: self.clone(),
                _permit: permit,
            });
        }
        // Пока ждали места в пуле, вход мог отвергнуть параллельный запрос.
        if let Some(e) = self.auth_failure() {
            return Err(e);
        }
        let (client, abort) = self.spec.connect_with_handle().await.inspect_err(|e| {
            if e.code == ErrorCode::AuthFailed {
                self.auth_failure
                    .lock_safe()
                    .get_or_insert_with(|| login_not_retried(e));
            }
        })?;
        let conn = PgConn {
            client,
            abort,
            runtime: Handle::current(),
            grace: self.close_grace,
            immediate: false,
        };
        Ok(PooledClient {
            conn: Some(conn),
            pool: self.clone(),
            _permit: permit,
        })
    }

    fn pop_idle(&self) -> Option<(PgConn, Instant)> {
        self.idle.lock_safe().pop()
    }

    /// Закрывает соединения, простоявшие не меньше `max_idle` (штатно: Terminate, затем, если не вышло,
    /// принудительно); возвращает, сколько закрыто.
    pub fn close_idle(&self, max_idle: Duration) -> usize {
        let mut idle = self.idle.lock_safe();
        let before = idle.len();
        idle.retain(|(_, since)| since.elapsed() < max_idle);
        before - idle.len()
    }

    pub fn idle_count(&self) -> usize {
        self.idle.lock_safe().len()
    }

    /// Отметка «сервер отверг вход», как после настоящего отказа: тесты других модулей проверяют, что
    /// переподключение её сбрасывает.
    #[cfg(test)]
    pub(crate) fn remember_auth_failure(&self, e: AppError) {
        self.auth_failure.lock_safe().get_or_insert(e);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PoolKey {
    pub connection_id: String,
    pub member: String,
    pub database: String,
    pub purpose: PoolPurpose,
}

/// Пулы приложения по ключу (подключение, участник, БД, назначение). У страниц каталога — до
/// `MAX_CONNECTIONS` соединений на узел и БД; опрос топологии держит рядом свой пул из одного соединения
/// (`PoolPurpose::Probe`), так что на узел приходится одно соединение сверх трёх каталожных.
#[derive(Default)]
pub struct PoolRegistry {
    pools: Mutex<HashMap<PoolKey, Arc<PgPool>>>,
}

impl PoolRegistry {
    pub fn get(&self, key: &PoolKey) -> Option<Arc<PgPool>> {
        self.pools.lock_safe().get(key).cloned()
    }

    /// Пул по ключу; если его нет или он смотрит на другой адрес узла (узел сменил адрес, пока пул жил) —
    /// новый из `spec`, прежний вытесняется. Пул того же адреса остаётся со своей спецификацией:
    /// успевший раньше поток выигрывает, а смена пароля и сертификатов закрывает пулы подключения целиком.
    pub fn insert_if_absent(&self, key: PoolKey, spec: ConnectSpec) -> Arc<PgPool> {
        let mut pools = self.pools.lock_safe();
        if let Some(p) = pools.get(&key)
            && p.routes_to(&spec.host, spec.port, spec.socks)
        {
            return p.clone();
        }
        let pool = PgPool::with_max(spec, key.purpose.max_connections());
        pools.insert(key, pool.clone());
        pool
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
            socks: None,
            ssh_target: None,
        }
    }

    fn key(c: &str, m: &str, d: &str) -> PoolKey {
        PoolKey {
            connection_id: c.into(),
            member: m.into(),
            database: d.into(),
            purpose: PoolPurpose::Catalog,
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
    fn existing_pool_keeps_its_spec() {
        let r = PoolRegistry::default();
        let first = r.insert_if_absent(key("c", "m", "d"), spec());
        // Тот же адрес узла: пул остаётся со своей спецификацией, остальное в новой не важно.
        let mut same_address = spec();
        same_address.user = "other".into();
        let again = r.insert_if_absent(key("c", "m", "d"), same_address);
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(again.spec().user, "u");
    }

    #[test]
    fn pool_is_replaced_when_the_member_changes_address() {
        let r = PoolRegistry::default();
        let first = r.insert_if_absent(key("c", "m", "d"), spec());
        let weak_first = Arc::downgrade(&first);

        let mut other_host = spec();
        other_host.host = "other".into();
        let by_host = r.insert_if_absent(key("c", "m", "d"), other_host.clone());
        assert!(!Arc::ptr_eq(&first, &by_host));
        assert_eq!(by_host.spec().host, "other");
        assert!(Arc::ptr_eq(&by_host, &r.get(&key("c", "m", "d")).unwrap()));
        drop(first);
        assert!(weak_first.upgrade().is_none(), "прежний пул освобождён");
        // Тот же (новый) адрес — пул уже тот.
        let again = r.insert_if_absent(key("c", "m", "d"), other_host);
        assert!(Arc::ptr_eq(&by_host, &again));

        let mut other_port = spec();
        other_port.host = "other".into();
        other_port.port = 6432;
        let by_port = r.insert_if_absent(key("c", "m", "d"), other_port);
        assert!(!Arc::ptr_eq(&by_host, &by_port));
        assert_eq!(
            (by_port.spec().host.as_str(), by_port.spec().port),
            ("other", 6432)
        );
        assert_eq!(r.keys().len(), 1);
    }

    /// Туннель перезапустился на новом порту: пул старого порта заменяется, хотя адрес узла тот же.
    #[test]
    fn pool_is_replaced_when_the_tunnel_port_changes() {
        let r = PoolRegistry::default();
        let mut via_old = spec();
        via_old.socks = Some("127.0.0.1:40001".parse().unwrap());
        let first = r.insert_if_absent(key("c", "m", "d"), via_old.clone());
        assert!(first.routes_to("h", 5432, via_old.socks));
        assert!(!first.routes_to("h", 5432, None));
        let mut via_new = spec();
        via_new.socks = Some("127.0.0.1:40002".parse().unwrap());
        let second = r.insert_if_absent(key("c", "m", "d"), via_new.clone());
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(second.spec().socks, via_new.socks);
        assert!(Arc::ptr_eq(
            &second,
            &r.insert_if_absent(key("c", "m", "d"), via_new)
        ));
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

    #[tokio::test]
    async fn probe_pool_has_a_single_permit() {
        let pool = PgPool::with_max(spec(), 1);
        assert_eq!(pool.permits.available_permits(), 1);
        assert_eq!(
            PgPool::new(spec()).permits.available_permits(),
            MAX_CONNECTIONS
        );
    }

    /// Пул опроса и пул каталога одного узла и БД — разные пулы: у опроса одно место, у каталога три.
    #[test]
    fn registry_sizes_pools_by_purpose() {
        let r = PoolRegistry::default();
        let catalog = r.insert_if_absent(key("c", "m", "d"), spec());
        let probe_key = PoolKey {
            purpose: PoolPurpose::Probe,
            ..key("c", "m", "d")
        };
        let probe = r.insert_if_absent(probe_key.clone(), spec());
        assert!(!Arc::ptr_eq(&catalog, &probe));
        assert_eq!(catalog.permits.available_permits(), MAX_CONNECTIONS);
        assert_eq!(probe.permits.available_permits(), 1);
        // Вытеснение участника и подключения убирает оба.
        r.evict_member("c", "m");
        assert!(r.keys().is_empty());
        r.insert_if_absent(probe_key, spec());
        r.evict_connection("c");
        assert!(r.keys().is_empty());
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
    /// Срок штатного закрытия, который тест не должен ждать.
    const LONG_GRACE: Duration = Duration::from_secs(60);

    /// Пул, где принудительное закрытие наступило бы через минуту: что закрылось раньше — закрылось сразу.
    fn pool_with_long_grace(port: u16, validate_after: Duration) -> Arc<PgPool> {
        PgPool::build(
            stalling_spec(port, Duration::from_millis(300)),
            validate_after,
            LONG_GRACE,
            MAX_CONNECTIONS,
        )
    }

    #[tokio::test]
    async fn discard_aborts_the_connection_and_keeps_it_out_of_idle() {
        let server = testutil::stalling_pg_server();
        let pool = pool_with_long_grace(server.port, VALIDATE_AFTER);
        let c = pool.get().await.unwrap();
        stall(&c).await;
        c.discard();
        assert!(
            server.wait_closed(1, WAIT).await,
            "сокет закрыт сразу, не после срока штатного закрытия"
        );
        assert_eq!(server.terminated(), 0, "ответы ждали: Terminate не уходит");
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
    async fn closing_a_healthy_idle_connection_sends_terminate() {
        let server = testutil::stalling_pg_server();
        let pool = pool_with_long_grace(server.port, VALIDATE_AFTER);
        let c = pool.get().await.unwrap();
        drop(c);
        assert_eq!(pool.idle_count(), 1);
        assert_eq!(pool.close_idle(Duration::ZERO), 1);
        assert!(
            server.wait_terminated(1, WAIT).await,
            "сервер получил Terminate, а не обрыв потока"
        );
        assert!(server.wait_closed(1, WAIT).await);
    }

    #[tokio::test]
    async fn evicting_a_pool_closes_its_healthy_idle_connections_with_terminate() {
        let server = testutil::stalling_pg_server();
        let registry = PoolRegistry::default();
        let k = key("c", "m", "d");
        let pool = registry.insert_if_absent(
            k.clone(),
            stalling_spec(server.port, Duration::from_secs(60)),
        );
        drop(pool.get().await.unwrap());
        drop(pool);
        registry.evict_connection("c");
        assert!(server.wait_terminated(1, WAIT).await);
        assert!(server.wait_closed(1, WAIT).await);
    }

    #[tokio::test]
    async fn dropping_a_pool_outside_the_runtime_still_closes_gracefully() {
        let server = testutil::stalling_pg_server();
        let pool = pool_with_long_grace(server.port, VALIDATE_AFTER);
        drop(pool.get().await.unwrap());
        // Синхронная команда Tauri вытесняет пул из потока без runtime tokio.
        std::thread::spawn(move || drop(pool)).join().unwrap();
        assert!(server.wait_terminated(1, WAIT).await);
        assert!(server.wait_closed(1, WAIT).await);
    }

    #[tokio::test]
    async fn closing_a_stalled_idle_connection_falls_back_to_abort_after_the_grace_period() {
        let server = testutil::stalling_pg_server();
        let pool = PgPool::new(stalling_spec(server.port, Duration::from_millis(400)));
        let c = pool.get().await.unwrap();
        stall(&c).await;
        drop(c);
        assert_eq!(pool.close_idle(Duration::ZERO), 1);
        // Ответ на запрос ждут: Terminate не уйдёт, но и обрывать сразу незачем — срок штатного закрытия.
        assert!(
            !server.wait_closed(1, Duration::from_millis(100)).await,
            "до конца срока сокет открыт"
        );
        assert!(
            server.wait_closed(1, WAIT).await,
            "по истечении срока задача прервана"
        );
        assert_eq!((server.closed(), server.terminated()), (1, 0));
    }

    #[tokio::test]
    async fn failed_validation_aborts_the_stalled_connection_at_once() {
        let server = testutil::stalling_pg_server();
        let pool = pool_with_long_grace(server.port, Duration::ZERO);
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
            "негодное соединение закрыто сразу, не по сроку штатного закрытия"
        );
        assert_eq!(server.terminated(), 0);
        assert_eq!(server.accepted(), 2);
        drop(fresh);
    }
}
