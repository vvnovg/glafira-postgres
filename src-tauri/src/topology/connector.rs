//! Соединение с узлом для опроса топологии: разовое (проверка подключения) или из пула (Task 12).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use crate::conn::pg::ConnectSpec;
use crate::conn::pool::{PoolKey, PoolPurpose, PoolRegistry, PooledClient};
use crate::conn::profile::ConnectionProfile;
use crate::conn::run_blocking;
use crate::error::{AppResult, ErrorCode};
use crate::net::ssh::tunnel::TunnelRegistry;
use crate::secrets::SecretStore;
use crate::topology::BoxFuture;
use crate::topology::probe::{COLLECT_TIMEOUT, NodeFacts, collect_with_timeout, no_answer};

pub trait NodeConnector: Send + Sync {
    fn facts<'a>(
        &'a self,
        member: &'a str,
        host: &'a str,
        port: u16,
    ) -> BoxFuture<'a, AppResult<NodeFacts>>;
}

/// Новое соединение на каждый опрос; секреты — как переданы (для проверки — форма поверх keychain).
/// `socks` — порт туннеля проверки (`None` — напрямую).
pub struct AdhocConnector {
    pub profile: ConnectionProfile,
    pub secrets: Arc<dyn SecretStore>,
    pub socks: Option<SocketAddr>,
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
        let (db, socks) = (profile.pg.database.clone(), self.socks);
        let spec = run_blocking(move || {
            ConnectSpec::build_via(&profile, &h, port, &db, secrets.as_ref(), socks)
        })
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

/// Соединение из пула приложения. При ошибке TLS пул вытесняется — следующая попытка перечитает файлы
/// сертификатов. Отказ во входе пул запоминает и новых попыток входа не делает (`PgPool::auth_failure`):
/// keychain не перечитывается, пароль не перебирается каждые 10 с. Пул, открытый на другой адрес узла,
/// заменяется. `purpose` выбирает пул: опрос топологии не стоит в очереди за страницами каталога. `socks` —
/// маршрут подключения (`TunnelRegistry::route_for`): пул, открытый через другой порт туннеля, тоже заменяется.
#[allow(clippy::too_many_arguments)]
pub async fn pooled_client(
    pools: &PoolRegistry,
    profile: &ConnectionProfile,
    secrets: Arc<dyn SecretStore>,
    member: &str,
    host: &str,
    port: u16,
    database: &str,
    purpose: PoolPurpose,
    socks: Option<SocketAddr>,
) -> AppResult<PooledClient> {
    let key = PoolKey {
        connection_id: profile.id.clone(),
        member: member.into(),
        database: database.into(),
        purpose,
    };
    let pool = match pools.get(&key).filter(|p| p.routes_to(host, port, socks)) {
        Some(p) => p,
        None => {
            let (p, h, d) = (profile.clone(), host.to_string(), database.to_string());
            let spec = run_blocking(move || {
                ConnectSpec::build_via(&p, &h, port, &d, secrets.as_ref(), socks)
            })
            .await?;
            pools.insert_if_absent(key.clone(), spec)
        }
    };
    pool.get().await.inspect_err(|e| {
        if e.code == ErrorCode::TlsError {
            pools.evict(&key);
        }
    })
}

pub struct PooledConnector {
    pub pools: Arc<PoolRegistry>,
    pub profile: ConnectionProfile,
    pub secrets: Arc<dyn SecretStore>,
    /// Туннели подключений: соединения профиля с ssh идут только через его туннель.
    pub tunnels: Arc<TunnelRegistry>,
}

impl PooledConnector {
    /// Опрос узла соединением из пула опроса (`PoolPurpose::Probe`, одно соединение на узел: страницы
    /// каталога его не занимают). Получение соединения (ожидание свободного места в пуле, проверка
    /// простоявшего соединения, подключение) и `collect` укладываются в один предел:
    /// таймаут подключения профиля + `limit`; иначе опрос не завершился бы, пока пул занят другими.
    /// Если опрос не удался (узел молчит, обрыв, ошибка запроса, предел вышел), соединение закрывается,
    /// а не возвращается в пул: следующий опрос не должен получить зависшее соединение.
    pub async fn facts_within(
        &self,
        member: &str,
        host: &str,
        port: u16,
        limit: Duration,
    ) -> AppResult<NodeFacts> {
        // Туннель упал или закрыт — узел недоступен с его ошибкой (`SSH_ERROR`), напрямую не идём.
        let socks = self.tunnels.route_for(&self.profile)?;
        let total = Duration::from_secs(self.profile.pg.connect_timeout_s) + limit;
        // Соединение лежит вне будущего под таймаутом: если предел вышел посреди `collect`, будущее
        // отбрасывается, а соединение с неотвеченными запросами нужно закрыть, а не вернуть в пул.
        let mut client = None;
        let outcome = tokio::time::timeout(total, async {
            let c = client.insert(
                pooled_client(
                    &self.pools,
                    &self.profile,
                    self.secrets.clone(),
                    member,
                    host,
                    port,
                    &self.profile.pg.database,
                    PoolPurpose::Probe,
                    socks,
                )
                .await?,
            );
            collect_with_timeout(c, limit).await
        })
        .await;
        let result = outcome.unwrap_or_else(|_| Err(no_answer(total)));
        if result.is_err()
            && let Some(c) = client
        {
            c.discard();
        }
        result
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
    use crate::secrets::{MemoryStore, SecretStore};

    const WAIT: Duration = Duration::from_secs(2);
    const LIMIT: Duration = Duration::from_millis(300);

    fn profile(mode: SslMode) -> ConnectionProfile {
        let mut p = sample_profile("c");
        p.pg.ssl_mode = mode;
        p.pg.connect_timeout_s = 1;
        p
    }

    fn pooled(mode: SslMode) -> PooledConnector {
        let pools: Arc<PoolRegistry> = Arc::default();
        PooledConnector {
            pools: pools.clone(),
            profile: profile(mode),
            secrets: Arc::new(MemoryStore::default()),
            tunnels: crate::net::ssh::tunnel::unused_registry(pools),
        }
    }

    fn key(member: &str) -> PoolKey {
        PoolKey {
            connection_id: "c".into(),
            member: member.into(),
            database: "postgres".into(),
            purpose: PoolPurpose::Catalog,
        }
    }

    /// Пул опроса топологии того же узла и БД.
    fn probe_key(member: &str) -> PoolKey {
        PoolKey {
            purpose: PoolPurpose::Probe,
            ..key(member)
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
        let pool = nodes.pools.get(&probe_key("m")).expect("пул создан");
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

    /// Keychain, который считает чтения пароля PostgreSQL.
    #[derive(Default)]
    struct CountingSecrets {
        inner: MemoryStore,
        password_reads: std::sync::atomic::AtomicUsize,
    }

    impl SecretStore for CountingSecrets {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            if key.ends_with(crate::secrets::PG_PASSWORD) {
                self.password_reads
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            self.inner.get(key)
        }
        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            self.inner.set(key, value)
        }
        fn delete(&self, key: &str) -> AppResult<()> {
            self.inner.delete(key)
        }
    }

    #[tokio::test]
    async fn rejected_login_is_not_retried_until_the_pool_is_evicted() {
        let server = testutil::auth_failing_pg_server();
        let secrets = Arc::new(CountingSecrets::default());
        let nodes = PooledConnector {
            secrets: secrets.clone(),
            ..pooled(SslMode::Disable)
        };
        let reads = || {
            secrets
                .password_reads
                .load(std::sync::atomic::Ordering::SeqCst)
        };
        let first = nodes
            .facts_within("m", "127.0.0.1", server.port, LIMIT)
            .await
            .unwrap_err();
        assert_eq!(
            (first.code, first.key.as_str()),
            (ErrorCode::AuthFailed, "errors.pg.authFailed")
        );
        // Опрос раз в 10 с: ни новых попыток входа, ни чтений keychain — сразу запомненный отказ.
        for _ in 0..5 {
            let e = nodes
                .facts_within("m", "127.0.0.1", server.port, LIMIT)
                .await
                .unwrap_err();
            assert_eq!(
                (e.code, e.key.as_str()),
                (ErrorCode::AuthFailed, "errors.pg.loginNotRetried")
            );
            assert!(e.details.as_deref().unwrap().contains("28P01"), "{e:?}");
        }
        assert_eq!(server.accepted(), 1, "одна попытка входа");
        assert_eq!(reads(), 1, "keychain прочитан один раз");
        assert_eq!(
            nodes.pools.keys(),
            vec![probe_key("m")],
            "пул с отметкой остаётся"
        );

        // Переоткрытие подключения (сохранение профиля или секретов) вытесняет пулы: вход пробуется снова.
        nodes.pools.evict_connection("c");
        nodes
            .facts_within("m", "127.0.0.1", server.port, LIMIT)
            .await
            .unwrap_err();
        assert_eq!(server.accepted(), 2);
        assert_eq!(reads(), 2);

        // Узел переехал на другой адрес — новый пул, новая попытка.
        let moved = testutil::auth_failing_pg_server();
        nodes
            .facts_within("m", "127.0.0.1", moved.port, LIMIT)
            .await
            .unwrap_err();
        assert_eq!((server.accepted(), moved.accepted()), (2, 1));
    }

    #[tokio::test]
    async fn rejected_login_does_not_spoil_other_databases_or_members() {
        let server = testutil::auth_failing_pg_server();
        let nodes = pooled(SslMode::Disable);
        nodes
            .facts_within("m", "127.0.0.1", server.port, LIMIT)
            .await
            .unwrap_err();
        // Отказ на одной БД (правило pg_hba.conf для конкретной базы) не запрещает вход в другие и на другие узлы.
        for (member, db) in [("m", "other"), ("n", "postgres")] {
            pooled_client(
                &nodes.pools,
                &nodes.profile,
                nodes.secrets.clone(),
                member,
                "127.0.0.1",
                server.port,
                db,
                PoolPurpose::Catalog,
                None,
            )
            .await
            .err()
            .unwrap();
        }
        assert_eq!(server.accepted(), 3);
    }

    #[tokio::test]
    async fn tls_failure_evicts_the_pool() {
        // CA не тот: проверка сертификата не проходит, пул вытесняется — файлы сертификатов перечитаются.
        let dir = tempfile::tempdir().unwrap();
        let other = testutil::make_ca("other");
        let ca_path = dir.path().join("ca.pem");
        std::fs::write(&ca_path, other.cert.to_pem().unwrap()).unwrap();
        let server = testutil::tls_server(&testutil::make_self_signed_server("localhost"));
        let mut nodes = pooled(SslMode::VerifyCa);
        nodes.profile.pg.tls.ca_path = Some(ca_path.to_string_lossy().into_owned());
        let e = nodes
            .facts_within("m", "127.0.0.1", server.port, LIMIT)
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::TlsError, "{e:?}");
        assert!(nodes.pools.keys().is_empty(), "пул вытеснен");
    }

    #[tokio::test]
    async fn connect_failure_keeps_the_pool() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        // На Windows отказ на закрытый loopback-порт приходит примерно через секунду: с пределом
        // `connect_timeout_s = 1` из `pooled()` вместо tcpFailed был бы connectTimeout.
        let mut nodes = pooled(SslMode::Disable);
        nodes.profile.pg.connect_timeout_s = 5;
        let e = nodes
            .facts_within("m", "127.0.0.1", port, LIMIT)
            .await
            .unwrap_err();
        assert_eq!(e.key, "errors.pg.tcpFailed");
        assert_eq!(nodes.pools.keys(), vec![probe_key("m")]);
    }

    /// Через туннель: узел, до которого бастион не достучался, — сбой проброса, а не ошибка PostgreSQL; закрытый
    /// туннель — ошибка туннеля без попытки соединиться напрямую.
    #[tokio::test]
    async fn pooled_connection_goes_through_the_tunnel_only() {
        use crate::net::ssh::tunnel::tests::{UNREACHABLE, profile as ssh_profile, registry};
        let dir = tempfile::tempdir().unwrap();
        let (tunnels, pools) =
            registry(dir.path(), vec![("FAKE_SSH_UPSTREAM", UNREACHABLE.into())]);
        let mut p = ssh_profile(crate::conn::profile::SshAuth::Agent);
        p.pg.ssl_mode = SslMode::Disable;
        let socks = tunnels.open(&p, Duration::from_secs(5)).await.unwrap();
        let nodes = PooledConnector {
            pools: pools.clone(),
            profile: p.clone(),
            secrets: Arc::new(MemoryStore::default()),
            tunnels: tunnels.clone(),
        };
        let e = nodes
            .facts_within("m", "pg-single", 5432, LIMIT)
            .await
            .unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str(), e.params["kind"].as_str()),
            (
                ErrorCode::SshError,
                "errors.ssh.forwardFailed",
                "forwardFailed"
            ),
            "{e:?}"
        );
        assert_eq!(
            (e.params["host"].as_str(), e.params["port"].as_str()),
            ("pg-single", "5432"),
            "{e:?}"
        );
        let pool = pools.get(&probe_key_of("c1", "m")).expect("пул создан");
        assert_eq!(pool.spec().socks, Some(socks));

        tunnels.close("c1");
        let e = nodes
            .facts_within("m", "pg-single", 5432, LIMIT)
            .await
            .unwrap_err();
        assert_eq!(
            (e.code, e.params["kind"].as_str()),
            (ErrorCode::SshError, "exited")
        );
    }

    fn probe_key_of(connection: &str, member: &str) -> PoolKey {
        PoolKey {
            connection_id: connection.into(),
            ..probe_key(member)
        }
    }

    #[tokio::test]
    async fn adhoc_failed_facts_abort_the_connection() {
        let server = testutil::stalling_pg_server();
        let nodes = AdhocConnector {
            profile: profile(SslMode::Disable),
            secrets: Arc::new(MemoryStore::default()),
            socks: None,
        };
        let e = nodes
            .facts_within("127.0.0.1", server.port, LIMIT)
            .await
            .unwrap_err();
        assert_eq!(e.key, "errors.pg.noAnswer");
        assert!(server.wait_closed(1, WAIT).await, "сокет закрыт");
    }

    async fn client_for(nodes: &PooledConnector, port: u16, purpose: PoolPurpose) -> PooledClient {
        pooled_client(
            &nodes.pools,
            &nodes.profile,
            nodes.secrets.clone(),
            "m",
            "127.0.0.1",
            port,
            "postgres",
            purpose,
            None,
        )
        .await
        .unwrap()
    }

    /// Соединение для страниц каталога.
    async fn client_of(nodes: &PooledConnector, port: u16) -> PooledClient {
        client_for(nodes, port, PoolPurpose::Catalog).await
    }

    #[tokio::test]
    async fn pool_is_replaced_when_the_member_moves_to_another_address() {
        let (first, second) = (
            testutil::stalling_pg_server(),
            testutil::stalling_pg_server(),
        );
        let nodes = pooled(SslMode::Disable);
        drop(client_of(&nodes, first.port).await);
        let pool = nodes.pools.get(&key("m")).unwrap();
        assert!(pool.looks_at("127.0.0.1", first.port));

        // Тот же адрес — тот же пул, соединение переиспользуется.
        drop(client_of(&nodes, first.port).await);
        assert!(Arc::ptr_eq(&pool, &nodes.pools.get(&key("m")).unwrap()));
        assert_eq!(first.accepted(), 1);

        // Узел переехал: соединения идут на новый адрес, а не на старый.
        drop(client_of(&nodes, second.port).await);
        let moved = nodes.pools.get(&key("m")).unwrap();
        assert!(!Arc::ptr_eq(&pool, &moved));
        assert!(moved.looks_at("127.0.0.1", second.port));
        assert_eq!((first.accepted(), second.accepted()), (1, 1));
        assert_eq!(nodes.pools.keys(), vec![key("m")]);
    }

    #[tokio::test]
    async fn waiting_for_a_pool_permit_is_bounded_by_the_limit() {
        let server = testutil::stalling_pg_server();
        let nodes = pooled(SslMode::Disable);
        // Единственное место пула опроса занято: опрос ждал бы освобождения бесконечно.
        let held = client_for(&nodes, server.port, PoolPurpose::Probe).await;
        let started = std::time::Instant::now();
        let e = tokio::time::timeout(
            Duration::from_secs(5),
            nodes.facts_within("m", "127.0.0.1", server.port, LIMIT),
        )
        .await
        .expect("опрос не должен висеть на занятом пуле")
        .unwrap_err();
        assert_eq!(e.key, "errors.pg.noAnswer");
        // Таймаут подключения профиля (1 с) + предел опроса.
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_secs(1), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
        drop(held);
    }

    /// Опрос топологии не ждёт, пока страницы каталога освободят пул: у него свой пул из одного соединения.
    #[tokio::test]
    async fn probe_has_its_own_pool_and_is_not_blocked_by_catalog_clients() {
        let server = testutil::stalling_pg_server();
        let nodes = pooled(SslMode::Disable);
        let mut held = Vec::new();
        for _ in 0..crate::conn::pool::MAX_CONNECTIONS {
            held.push(client_of(&nodes, server.port).await);
        }
        assert_eq!(server.accepted(), 3);
        // Молчащий сервер: опрос заканчивается своим пределом, но соединение он открыл — четвёртое.
        let e = nodes
            .facts_within("m", "127.0.0.1", server.port, Duration::from_millis(300))
            .await
            .unwrap_err();
        assert_eq!(e.key, "errors.pg.noAnswer");
        assert_eq!(
            server.accepted(),
            4,
            "опрос не стоял в очереди за местами каталога"
        );
        let mut keys = nodes.pools.keys();
        keys.sort_by_key(|k| k.purpose == PoolPurpose::Probe);
        assert_eq!(
            keys.iter().map(|k| k.purpose).collect::<Vec<_>>(),
            [PoolPurpose::Catalog, PoolPurpose::Probe]
        );
        drop(held);
    }

    /// Предел вышел, пока опрос уже шёл (соединения пришлось ждать): соединение с неотвеченными запросами
    /// закрывается, а не возвращается в пул.
    #[tokio::test]
    async fn limit_expiring_during_collect_discards_the_connection() {
        let server = testutil::stalling_pg_server();
        let nodes = pooled(SslMode::Disable);
        // Место пула опроса занято; страницы каталога держат свои три соединения — их опрос не трогает.
        let mut held = vec![client_for(&nodes, server.port, PoolPurpose::Probe).await];
        let mut catalog = Vec::new();
        for _ in 0..crate::conn::pool::MAX_CONNECTIONS {
            catalog.push(client_of(&nodes, server.port).await);
        }
        let pool = nodes.pools.get(&probe_key("m")).unwrap();
        // Предел: 1 с (подключение) + 1 с. Место освобождается через 1,5 с — опрос начнётся, но до
        // собственного таймаута `collect` (2,5 с) общий предел (2 с) уже выйдет.
        let facts = nodes.facts_within("m", "127.0.0.1", server.port, Duration::from_secs(1));
        let release = async {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            held.pop();
        };
        let (result, ()) = tokio::join!(facts, release);
        let e = result.unwrap_err();
        assert_eq!(e.key, "errors.pg.noAnswer");
        assert_eq!(
            pool.idle_count(),
            0,
            "зависшее соединение не вернулось в пул"
        );
        assert!(server.wait_closed(1, WAIT).await, "и его сокет закрыт");
        assert_eq!(server.closed(), 1, "соединения каталога заняты и открыты");
        drop((held, catalog));
    }
}
