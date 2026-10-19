//! Соединение с узлом для опроса топологии: разовое (проверка подключения) или из пула (Task 12).

use std::sync::Arc;
use std::time::Duration;

use crate::conn::pg::ConnectSpec;
use crate::conn::pool::{PoolKey, PoolRegistry, PooledClient};
use crate::conn::profile::ConnectionProfile;
use crate::conn::run_blocking;
use crate::error::{AppResult, ErrorCode};
use crate::secrets::SecretStore;
use crate::topology::BoxFuture;
use crate::topology::probe::{COLLECT_TIMEOUT, NodeFacts, collect_with_timeout};

pub trait NodeConnector: Send + Sync {
    fn facts<'a>(
        &'a self,
        member: &'a str,
        host: &'a str,
        port: u16,
    ) -> BoxFuture<'a, AppResult<NodeFacts>>;
}

/// Новое соединение на каждый опрос; секреты — как переданы (для проверки — форма поверх keychain).
pub struct AdhocConnector {
    pub profile: ConnectionProfile,
    pub secrets: Arc<dyn SecretStore>,
}

impl AdhocConnector {
    /// Опрос узла новым соединением не дольше `limit`. Если узел не ответил, задача соединения
    /// прерывается: drop клиента с неотвеченным запросом сокет не закрывает.
    pub async fn facts_within(
        &self,
        host: &str,
        port: u16,
        limit: Duration,
    ) -> AppResult<NodeFacts> {
        let (profile, secrets, h) = (self.profile.clone(), self.secrets.clone(), host.to_string());
        let db = profile.pg.database.clone();
        let spec =
            run_blocking(move || ConnectSpec::build(&profile, &h, port, &db, secrets.as_ref()))
                .await?;
        let (client, abort) = spec.connect_with_handle().await?;
        let facts = collect_with_timeout(&client, limit).await;
        if facts.is_err() {
            abort.abort();
        }
        facts
    }
}

impl NodeConnector for AdhocConnector {
    fn facts<'a>(
        &'a self,
        _member: &'a str,
        host: &'a str,
        port: u16,
    ) -> BoxFuture<'a, AppResult<NodeFacts>> {
        Box::pin(async move { self.facts_within(host, port, COLLECT_TIMEOUT).await })
    }
}

/// Соединение из пула приложения; при AUTH/TLS-ошибке пул вытесняется — следующая попытка
/// перечитает keychain и файлы сертификатов.
pub async fn pooled_client(
    pools: &PoolRegistry,
    profile: &ConnectionProfile,
    secrets: Arc<dyn SecretStore>,
    member: &str,
    host: &str,
    port: u16,
    database: &str,
) -> AppResult<PooledClient> {
    let key = PoolKey {
        connection_id: profile.id.clone(),
        member: member.into(),
        database: database.into(),
    };
    let pool = match pools.get(&key) {
        Some(p) => p,
        None => {
            let (p, h, d) = (profile.clone(), host.to_string(), database.to_string());
            let spec = run_blocking(move || ConnectSpec::build(&p, &h, port, &d, secrets.as_ref()))
                .await?;
            pools.insert_if_absent(key.clone(), spec)
        }
    };
    pool.get().await.inspect_err(|e| {
        if matches!(e.code, ErrorCode::AuthFailed | ErrorCode::TlsError) {
            pools.evict(&key);
        }
    })
}

pub struct PooledConnector {
    pub pools: Arc<PoolRegistry>,
    pub profile: ConnectionProfile,
    pub secrets: Arc<dyn SecretStore>,
}

impl PooledConnector {
    /// Опрос узла соединением из пула не дольше `limit`. Если опрос не удался (узел молчит, обрыв,
    /// ошибка запроса), соединение закрывается, а не возвращается в пул: следующий опрос не должен
    /// получить зависшее соединение.
    pub async fn facts_within(
        &self,
        member: &str,
        host: &str,
        port: u16,
        limit: Duration,
    ) -> AppResult<NodeFacts> {
        let client = pooled_client(
            &self.pools,
            &self.profile,
            self.secrets.clone(),
            member,
            host,
            port,
            &self.profile.pg.database,
        )
        .await?;
        match collect_with_timeout(&client, limit).await {
            Ok(facts) => Ok(facts),
            Err(e) => {
                client.discard();
                Err(e)
            }
        }
    }
}

impl NodeConnector for PooledConnector {
    fn facts<'a>(
        &'a self,
        member: &'a str,
        host: &'a str,
        port: u16,
    ) -> BoxFuture<'a, AppResult<NodeFacts>> {
        Box::pin(async move { self.facts_within(member, host, port, COLLECT_TIMEOUT).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::{SslMode, sample_profile};
    use crate::conn::testutil;
    use crate::secrets::MemoryStore;

    const WAIT: Duration = Duration::from_secs(2);
    const LIMIT: Duration = Duration::from_millis(300);

    fn profile(mode: SslMode) -> ConnectionProfile {
        let mut p = sample_profile("c");
        p.pg.ssl_mode = mode;
        p.pg.connect_timeout_s = 1;
        p
    }

    fn pooled(mode: SslMode) -> PooledConnector {
        PooledConnector {
            pools: Arc::default(),
            profile: profile(mode),
            secrets: Arc::new(MemoryStore::default()),
        }
    }

    fn key(member: &str) -> PoolKey {
        PoolKey {
            connection_id: "c".into(),
            member: member.into(),
            database: "postgres".into(),
        }
    }

    #[tokio::test]
    async fn failed_facts_discard_the_pooled_connection() {
        let server = testutil::stalling_pg_server();
        let nodes = pooled(SslMode::Disable);
        let e = nodes
            .facts_within("m", "127.0.0.1", server.port, LIMIT)
            .await
            .unwrap_err();
        assert_eq!(e.key, "errors.pg.noAnswer");
        let pool = nodes.pools.get(&key("m")).expect("пул создан");
        assert_eq!(
            pool.idle_count(),
            0,
            "зависшее соединение не вернулось в пул"
        );
        assert!(server.wait_closed(1, WAIT).await, "сокет закрыт");

        // Следующий опрос открывает новое соединение, а не берёт зависшее.
        nodes
            .facts_within("m", "127.0.0.1", server.port, LIMIT)
            .await
            .unwrap_err();
        assert_eq!(server.accepted(), 2);
        assert!(server.wait_closed(2, WAIT).await);
        assert_eq!(pool.idle_count(), 0);
    }

    #[tokio::test]
    async fn auth_failure_evicts_the_pool() {
        // Заглушка делает TLS-рукопожатие и отвечает ошибкой 28000.
        let server = testutil::tls_server(&testutil::make_self_signed_server("localhost"));
        let nodes = pooled(SslMode::Require);
        let e = nodes
            .facts_within("m", "127.0.0.1", server.port, LIMIT)
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::AuthFailed);
        assert!(
            nodes.pools.keys().is_empty(),
            "пул вытеснен: пароль перечитается"
        );
    }

    #[tokio::test]
    async fn connect_failure_keeps_the_pool() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let nodes = pooled(SslMode::Disable);
        let e = nodes
            .facts_within("m", "127.0.0.1", port, LIMIT)
            .await
            .unwrap_err();
        assert_eq!(e.key, "errors.pg.tcpFailed");
        assert_eq!(nodes.pools.keys(), vec![key("m")]);
    }

    #[tokio::test]
    async fn adhoc_failed_facts_abort_the_connection() {
        let server = testutil::stalling_pg_server();
        let nodes = AdhocConnector {
            profile: profile(SslMode::Disable),
            secrets: Arc::new(MemoryStore::default()),
        };
        let e = nodes
            .facts_within("127.0.0.1", server.port, LIMIT)
            .await
            .unwrap_err();
        assert_eq!(e.key, "errors.pg.noAnswer");
        assert!(server.wait_closed(1, WAIT).await, "сокет закрыт");
    }
}
