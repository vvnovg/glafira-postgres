//! Соединение с PostgreSQL: параметры из профиля и keychain, TCP, TLS, таймаут (спец. P1 §7).

use std::time::Duration;

use openssl::ssl::SslConnector;
use tokio::net::TcpStream;
use tokio::task::AbortHandle;
use tokio_postgres::{Client, NoTls};

use crate::conn::profile::{ConnectionProfile, SslMode};
use crate::conn::{errors, pgpass, tls};
use crate::error::AppResult;
use crate::secrets::{PG_KEY_PASSWORD, PG_PASSWORD, SecretStore, secret_key};

/// Параметры сессии диагностики: только чтение и короткие таймауты.
pub const SESSION_OPTIONS: &str =
    "-c default_transaction_read_only=on -c statement_timeout=5000 -c lock_timeout=1000";

/// Всё для нового соединения с узлом. Собирается один раз на пул: keychain, ~/.pgpass, файлы TLS.
#[derive(Clone)]
pub struct ConnectSpec {
    /// Куда открывать TCP (P1b: при туннеле — локальный порт).
    pub tcp_host: String,
    pub tcp_port: u16,
    /// Исходный адрес узла: имя для проверки сертификата и поиска в ~/.pgpass.
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub database: String,
    pub ssl_mode: SslMode,
    pub connector: Option<SslConnector>,
    pub timeout: Duration,
}

impl std::fmt::Debug for ConnectSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectSpec")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("ssl_mode", &self.ssl_mode)
            .finish_non_exhaustive()
    }
}

impl ConnectSpec {
    /// Блокирующий вызов: keychain, ~/.pgpass, файлы сертификатов.
    pub fn build(
        profile: &ConnectionProfile,
        host: &str,
        port: u16,
        database: &str,
        secrets: &dyn SecretStore,
    ) -> AppResult<Self> {
        let user = profile.pg.username.clone();
        let password = match secrets.get(&secret_key(&profile.id, PG_PASSWORD))? {
            Some(p) if !p.is_empty() => Some(p),
            _ => pgpass::find_password(host, port, database, &user),
        };
        let key_password = secrets.get(&secret_key(&profile.id, PG_KEY_PASSWORD))?;
        let connector = tls::pg_connector(
            profile.pg.ssl_mode,
            profile.pg.tls.ca_path.as_deref(),
            profile.pg.tls.client_cert.as_ref(),
            key_password.as_deref(),
        )?;
        Ok(Self {
            tcp_host: host.to_string(),
            tcp_port: port,
            host: host.to_string(),
            port,
            user,
            password,
            database: database.to_string(),
            ssl_mode: profile.pg.ssl_mode,
            connector,
            timeout: Duration::from_secs(profile.pg.connect_timeout_s),
        })
    }

    pub fn pg_config(&self) -> tokio_postgres::Config {
        use tokio_postgres::config::SslMode as Wire;
        let mut c = tokio_postgres::Config::new();
        c.user(&self.user)
            .dbname(&self.database)
            .application_name(crate::product::PG_APPLICATION_NAME)
            .options(SESSION_OPTIONS)
            .ssl_mode(match self.ssl_mode {
                SslMode::Disable => Wire::Disable,
                SslMode::Prefer => Wire::Prefer,
                SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => Wire::Require,
            });
        if let Some(p) = &self.password {
            c.password(p);
        }
        c
    }

    /// Новое соединение. Задача соединения запускается в текущем runtime tokio.
    pub async fn connect(&self) -> AppResult<Client> {
        Ok(self.connect_with_handle().await?.0)
    }

    /// Новое соединение вместе с `AbortHandle` его задачи. Закрыть молчащее соединение можно только
    /// так: tokio-postgres отправляет Terminate при drop клиента, лишь когда ждать ответов нечего,
    /// а соединение с неотвеченным запросом после drop остаётся открытым.
    pub async fn connect_with_handle(&self) -> AppResult<(Client, AbortHandle)> {
        let attempt = async {
            let stream = TcpStream::connect((self.tcp_host.as_str(), self.tcp_port))
                .await
                .map_err(|e| errors::tcp_error(&e, &self.host, self.port))?;
            let _ = stream.set_nodelay(true);
            let cfg = self.pg_config();
            match &self.connector {
                None => {
                    let (client, conn) = cfg
                        .connect_raw(stream, NoTls)
                        .await
                        .map_err(|e| errors::from_pg(&e))?;
                    Ok((client, spawn_connection(conn)))
                }
                Some(connector) => {
                    let mut ssl = connector.configure().map_err(errors::tls_setup)?;
                    if self.ssl_mode != SslMode::VerifyFull {
                        ssl.set_verify_hostname(false);
                    }
                    let tls = postgres_openssl::TlsConnector::new(ssl, &self.host);
                    let (client, conn) = cfg
                        .connect_raw(stream, tls)
                        .await
                        .map_err(|e| errors::from_pg(&e))?;
                    Ok((client, spawn_connection(conn)))
                }
            }
        };
        tokio::time::timeout(self.timeout, attempt)
            .await
            .map_err(|_| errors::connect_timeout(&self.host, self.port, self.timeout.as_secs()))?
    }
}

fn spawn_connection<F>(conn: F) -> AbortHandle
where
    F: std::future::Future<Output = Result<(), tokio_postgres::Error>> + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!("pg connection ended: {e}");
        }
    })
    .abort_handle()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::sample_profile;
    use crate::conn::testutil;
    use crate::error::ErrorCode;
    use crate::secrets::MemoryStore;

    #[test]
    fn spec_takes_password_from_keychain_and_sets_session() {
        let secrets = MemoryStore::default();
        secrets.set(&secret_key("a", PG_PASSWORD), "pw").unwrap();
        let mut p = sample_profile("a");
        p.pg.ssl_mode = SslMode::Disable;
        p.pg.connect_timeout_s = 7;
        let spec = ConnectSpec::build(&p, "db1", 5433, "orders", &secrets).unwrap();
        assert_eq!((spec.tcp_host.as_str(), spec.tcp_port), ("db1", 5433));
        assert_eq!(spec.password.as_deref(), Some("pw"));
        assert!(spec.connector.is_none());
        assert_eq!(spec.timeout, Duration::from_secs(7));
        let cfg = spec.pg_config();
        assert_eq!(cfg.get_user(), Some("glafira_diag"));
        assert_eq!(cfg.get_dbname(), Some("orders"));
        assert_eq!(cfg.get_application_name(), Some("glafira-pg"));
        assert_eq!(cfg.get_options(), Some(SESSION_OPTIONS));
        assert_eq!(cfg.get_password(), Some(b"pw".as_slice()));
        assert_eq!(cfg.get_ssl_mode(), tokio_postgres::config::SslMode::Disable);
    }

    #[test]
    fn verify_modes_require_tls_on_the_wire() {
        for (mode, wire) in [
            (SslMode::Prefer, tokio_postgres::config::SslMode::Prefer),
            (SslMode::Require, tokio_postgres::config::SslMode::Require),
            (SslMode::VerifyCa, tokio_postgres::config::SslMode::Require),
            (
                SslMode::VerifyFull,
                tokio_postgres::config::SslMode::Require,
            ),
        ] {
            let mut p = sample_profile("a");
            p.pg.ssl_mode = mode;
            let spec =
                ConnectSpec::build(&p, "h", 5432, "postgres", &MemoryStore::default()).unwrap();
            assert_eq!(spec.pg_config().get_ssl_mode(), wire, "{mode:?}");
            assert!(spec.connector.is_some());
        }
    }

    #[test]
    fn debug_hides_password() {
        let secrets = MemoryStore::default();
        secrets
            .set(&secret_key("a", PG_PASSWORD), "s3cr3t")
            .unwrap();
        let spec =
            ConnectSpec::build(&sample_profile("a"), "h", 5432, "postgres", &secrets).unwrap();
        assert!(!format!("{spec:?}").contains("s3cr3t"));
    }

    /// Спецификация для заглушки на loopback: TCP — на `port`, имя для проверки сертификата — `host`.
    fn loopback_spec(port: u16, host: &str, mode: SslMode, ca: Option<&str>) -> ConnectSpec {
        ConnectSpec {
            tcp_host: "127.0.0.1".into(),
            tcp_port: port,
            host: host.into(),
            port,
            user: "u".into(),
            password: None,
            database: "postgres".into(),
            ssl_mode: mode,
            connector: tls::pg_connector(mode, ca, None, None).unwrap(),
            timeout: Duration::from_secs(10),
        }
    }

    fn write_pem(dir: &std::path::Path, name: &str, cert: &openssl::x509::X509) -> String {
        let p = dir.join(name);
        std::fs::write(&p, cert.to_pem().unwrap()).unwrap();
        p.to_string_lossy().into_owned()
    }

    /// Рукопожатие прошло: заглушка ответила `ErrorResponse` 28000 на стартовое сообщение.
    fn assert_handshake_ok(e: crate::error::AppError) {
        assert_eq!(e.key, "errors.pg.authFailed", "{e:?}");
        assert!(e.details.unwrap().contains("harness"));
    }

    #[tokio::test]
    async fn verify_full_with_pinned_ca_accepts_matching_host() {
        let dir = tempfile::tempdir().unwrap();
        let ca = testutil::make_ca("test-ca");
        let server = testutil::tls_server(&testutil::make_server_cert(&ca, "localhost"));
        let ca_path = write_pem(dir.path(), "ca.pem", &ca.cert);
        let spec = loopback_spec(
            server.port,
            "localhost",
            SslMode::VerifyFull,
            Some(&ca_path),
        );
        assert_handshake_ok(spec.connect().await.unwrap_err());
    }

    #[tokio::test]
    async fn verify_full_rejects_host_missing_from_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let ca = testutil::make_ca("test-ca");
        // В сертификате только DNS:localhost, а проверяется IP-адрес.
        let server = testutil::tls_server(&testutil::make_server_cert(&ca, "localhost"));
        let ca_path = write_pem(dir.path(), "ca.pem", &ca.cert);
        let spec = loopback_spec(
            server.port,
            "127.0.0.1",
            SslMode::VerifyFull,
            Some(&ca_path),
        );
        let e = spec.connect().await.unwrap_err();
        assert_eq!(e.code, ErrorCode::TlsError);
        assert_eq!(e.key, "errors.pg.hostnameMismatch", "{e:?}");
    }

    #[tokio::test]
    async fn verify_ca_does_not_check_the_host_name() {
        let dir = tempfile::tempdir().unwrap();
        let ca = testutil::make_ca("test-ca");
        let server = testutil::tls_server(&testutil::make_server_cert(&ca, "localhost"));
        let ca_path = write_pem(dir.path(), "ca.pem", &ca.cert);
        let spec = loopback_spec(server.port, "127.0.0.1", SslMode::VerifyCa, Some(&ca_path));
        assert_handshake_ok(spec.connect().await.unwrap_err());
    }

    #[tokio::test]
    async fn verify_full_with_another_ca_is_untrusted() {
        let dir = tempfile::tempdir().unwrap();
        let ca = testutil::make_ca("test-ca");
        let other = testutil::make_ca("other-ca");
        let server = testutil::tls_server(&testutil::make_server_cert(&ca, "localhost"));
        let other_path = write_pem(dir.path(), "other.pem", &other.cert);
        let spec = loopback_spec(
            server.port,
            "localhost",
            SslMode::VerifyFull,
            Some(&other_path),
        );
        let e = spec.connect().await.unwrap_err();
        assert_eq!(e.code, ErrorCode::TlsError);
        assert_eq!(e.key, "errors.pg.caUntrusted", "{e:?}");
    }

    #[tokio::test]
    async fn verify_full_without_ca_uses_system_roots_only() {
        let ca = testutil::make_ca("test-ca");
        let server = testutil::tls_server(&testutil::make_server_cert(&ca, "localhost"));
        let spec = loopback_spec(server.port, "localhost", SslMode::VerifyFull, None);
        let e = spec.connect().await.unwrap_err();
        assert_eq!(e.code, ErrorCode::TlsError);
        assert_eq!(e.key, "errors.pg.caUntrusted", "{e:?}");
    }

    #[tokio::test]
    async fn require_without_ca_accepts_self_signed_certificate() {
        let server = testutil::tls_server(&testutil::make_self_signed_server("localhost"));
        let spec = loopback_spec(server.port, "127.0.0.1", SslMode::Require, None);
        assert_handshake_ok(spec.connect().await.unwrap_err());
    }

    #[tokio::test]
    async fn silent_server_gives_connection_failed_after_timeout() {
        let server = testutil::silent_server();
        let mut spec = loopback_spec(server.port, "127.0.0.1", SslMode::Prefer, None);
        spec.timeout = Duration::from_millis(300);
        let started = std::time::Instant::now();
        let e = spec.connect().await.unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        // §13: таймаут подключения — CONNECTION_FAILED, а не TIMEOUT.
        assert_eq!(e.code, ErrorCode::ConnectionFailed);
        assert_eq!(e.key, "errors.pg.connectTimeout");
        assert!(e.retryable);
    }

    #[tokio::test]
    async fn refused_connection_is_tcp_failed() {
        // Порт освобождается: слушатель закрыт до подключения.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let spec = loopback_spec(port, "127.0.0.1", SslMode::Disable, None);
        let e = spec.connect().await.unwrap_err();
        assert_eq!(e.code, ErrorCode::ConnectionFailed);
        assert_eq!(e.key, "errors.pg.tcpFailed");
    }

    /// Запрос ушёл на сервер, ответа нет: ответ ждёт соединение, а не тот, кто спрашивал.
    async fn stall(client: &Client) {
        tokio::time::timeout(Duration::from_millis(200), client.simple_query("SELECT 1"))
            .await
            .expect_err("сервер не отвечает");
    }

    /// tokio-postgres отправляет Terminate при drop клиента, только когда ответов не ждёт. Молчащее
    /// соединение с неотвеченным запросом так не закрыть: нужен `AbortHandle` задачи соединения.
    #[tokio::test]
    async fn abort_handle_closes_a_connection_with_pending_response() {
        let server = testutil::stalling_pg_server();
        let spec = loopback_spec(server.port, "127.0.0.1", SslMode::Disable, None);

        let (client, _abort) = spec.connect_with_handle().await.unwrap();
        stall(&client).await;
        drop(client);
        assert!(
            !server.wait_closed(1, Duration::from_millis(400)).await,
            "drop клиента с неотвеченным запросом не закрывает сокет — ради этого нужен abort"
        );

        let (client, abort) = spec.connect_with_handle().await.unwrap();
        stall(&client).await;
        abort.abort();
        assert!(server.wait_closed(1, Duration::from_secs(2)).await);
        assert_eq!(server.accepted(), 2);
        assert!(client.is_closed(), "клиент видит закрытое соединение");
    }

    #[tokio::test]
    async fn connect_keeps_its_signature_and_works() {
        let server = testutil::stalling_pg_server();
        let spec = loopback_spec(server.port, "127.0.0.1", SslMode::Disable, None);
        let client: Client = spec.connect().await.unwrap();
        assert!(!client.is_closed());
        assert_eq!(server.accepted(), 1);
    }
}
