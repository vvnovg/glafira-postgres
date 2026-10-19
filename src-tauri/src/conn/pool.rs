//! Пул соединений по ключу (подключение, участник, БД): не больше 3 соединений, проверка при выдаче,
//! закрытие простаивающих (спец. P1 §7). Свой пул вместо deadpool-postgres — см. «Отступления» плана P1a.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_postgres::Client;

use crate::conn::pg::ConnectSpec;
use crate::error::{AppError, AppResult};
use crate::sync::MutexExt;

pub const MAX_CONNECTIONS: usize = 3;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Соединение, простоявшее дольше, перед выдачей проверяется пустым запросом.
const VALIDATE_AFTER: Duration = Duration::from_secs(30);

pub struct PgPool {
    spec: ConnectSpec,
    idle: Mutex<Vec<(Client, Instant)>>,
    permits: Arc<Semaphore>,
}

/// Соединение из пула; при drop живое соединение возвращается в пул.
pub struct PooledClient {
    client: Option<Client>,
    pool: Arc<PgPool>,
    _permit: OwnedSemaphorePermit,
}

impl Deref for PooledClient {
    type Target = Client;
    fn deref(&self) -> &Client {
        self.client.as_ref().expect("client is present until drop")
    }
}

impl Drop for PooledClient {
    fn drop(&mut self) {
        if let Some(c) = self.client.take()
            && !c.is_closed()
        {
            self.pool.idle.lock_safe().push((c, Instant::now()));
        }
    }
}

impl PgPool {
    pub fn new(spec: ConnectSpec) -> Arc<Self> {
        Arc::new(Self {
            spec,
            idle: Mutex::new(Vec::new()),
            permits: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
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
        while let Some((client, since)) = self.pop_idle() {
            if client.is_closed() {
                continue;
            }
            if since.elapsed() >= VALIDATE_AFTER && client.simple_query("").await.is_err() {
                continue;
            }
            return Ok(PooledClient {
                client: Some(client),
                pool: self.clone(),
                _permit: permit,
            });
        }
        let client = self.spec.connect().await?;
        Ok(PooledClient {
            client: Some(client),
            pool: self.clone(),
            _permit: permit,
        })
    }

    fn pop_idle(&self) -> Option<(Client, Instant)> {
        self.idle.lock_safe().pop()
    }

    /// Закрывает соединения, простоявшие не меньше `max_idle`; возвращает, сколько закрыто.
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
        r.insert_if_absent(key("c1", "m1", "d"), spec());
        r.insert_if_absent(key("c1", "m2", "d"), spec());
        r.insert_if_absent(key("c2", "m1", "d"), spec());
        r.evict_member("c1", "m1");
        let mut keys = r.keys();
        keys.sort_by(|a, b| (&a.connection_id, &a.member).cmp(&(&b.connection_id, &b.member)));
        assert_eq!(keys, vec![key("c1", "m2", "d"), key("c2", "m1", "d")]);
        r.evict_connection("c1");
        assert_eq!(r.keys(), vec![key("c2", "m1", "d")]);
        r.evict(&key("c2", "m1", "d"));
        assert!(r.keys().is_empty());
    }
}
