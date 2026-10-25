//! Соединение с PostgreSQL: параметры из профиля и keychain, TCP, TLS, таймаут (спец. P1 §7).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use openssl::ssl::{SslConnector, SslVerifyMode};
use openssl::x509::X509VerifyResult;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::task::AbortHandle;
use tokio_postgres::{Client, NoTls};

use crate::conn::profile::{ConnectionProfile, SslMode};
use crate::conn::{errors, pgpass, tls};
use crate::error::{AppError, AppResult};
use crate::secrets::{PG_KEY_PASSWORD, PG_PASSWORD, SecretStore, secret_key};
use crate::sync::MutexExt;

/// Параметры сессии диагностики: только чтение, короткие таймауты и «нейтральный» текст определений.
/// `search_path=pg_catalog`: `pg_get_constraintdef`, `pg_get_triggerdef`, `pg_get_indexdef`, `pg_get_viewdef`
/// и `pg_get_expr` опускают схему у видимых в пути имён; без этого объекты из `public` попадали бы в DDL
/// без схемы и работали бы только при том же `search_path`. Запросы приложения читают лишь объекты
/// `pg_catalog`. `DateStyle=ISO`: границы партиций и значения по умолчанию печатаются в ISO (`'2026-03-01'`),
/// а не в стиле сеанса (`'01/03/2026'` при `SQL, DMY`).
pub const SESSION_OPTIONS: &str = "-c default_transaction_read_only=on -c statement_timeout=5000 -c lock_timeout=1000 -c search_path=pg_catalog -c DateStyle=ISO";

/// Сессия действия (спец. P2 §8.2): те же пределы, но без `default_transaction_read_only`.
pub const ACTION_SESSION_OPTIONS: &str =
    "-c statement_timeout=5000 -c lock_timeout=1000 -c search_path=pg_catalog -c DateStyle=ISO";

/// Отсечка молчащего соединения на уровне TCP: после 60 с тишины зонды раз в 10 с. После failover
/// полуоткрытое соединение иначе живёт в пуле, пока его не попробуют использовать.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(60);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);

/// TCP-соединение с узлом: без задержки Нейгла и с keepalive (ошибки настройки сокета не фатальны).
pub(crate) async fn tcp_connect(host: &str, port: u16) -> std::io::Result<TcpStream> {
    let stream = TcpStream::connect((host, port)).await?;
    let _ = stream.set_nodelay(true);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(KEEPALIVE_IDLE)
        .with_interval(KEEPALIVE_INTERVAL);
    let _ = socket2::SockRef::from(&stream).set_tcp_keepalive(&keepalive);
    Ok(stream)
}

/// Всё для нового соединения с узлом. Собирается один раз на пул: keychain, ~/.pgpass, файлы TLS.
#[derive(Clone)]
pub struct ConnectSpec {
    /// Куда открывать TCP напрямую (без туннеля).
    pub tcp_host: String,
    pub tcp_port: u16,
    /// Исходный адрес узла: имя для проверки сертификата и поиска в ~/.pgpass; через туннель его разрешает бастион.
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub database: String,
    pub ssl_mode: SslMode,
    pub connector: Option<SslConnector>,
    pub timeout: Duration,
    /// Порт SOCKS5 туннеля ssh (`127.0.0.1:<порт>`): TCP-поток идёт через него.
    pub socks: Option<SocketAddr>,
    /// Цель ssh туннеля — для сообщения о сбое проброса.
    pub ssh_target: Option<String>,
    /// Параметры сессии (`-c …`): `SESSION_OPTIONS` у пулов, `ACTION_SESSION_OPTIONS` у действий.
    pub options: &'static str,
}

impl std::fmt::Debug for ConnectSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectSpec")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("ssl_mode", &self.ssl_mode)
            .field("socks", &self.socks)
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
        Self::build_via(profile, host, port, database, secrets, None)
    }

    /// То же через туннель: `socks` — порт SOCKS5 туннеля подключения (`TunnelRegistry::route_for`). TLS и
    /// `~/.pgpass` — по исходному имени узла. Блокирующий вызов.
    pub fn build_via(
        profile: &ConnectionProfile,
        host: &str,
        port: u16,
        database: &str,
        secrets: &dyn SecretStore,
        socks: Option<SocketAddr>,
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
            socks,
            ssh_target: socks.and(profile.ssh.as_ref().map(|s| s.target.clone())),
            options: SESSION_OPTIONS,
        })
    }

    /// То же соединение для действия (P2b): запись разрешена, пределы и `search_path` те же.
    pub fn for_action(mut self) -> Self {
        self.options = ACTION_SESSION_OPTIONS;
        self
    }

    pub fn pg_config(&self) -> tokio_postgres::Config {
        use tokio_postgres::config::SslMode as Wire;
        let mut c = tokio_postgres::Config::new();
        c.user(&self.user)
            .dbname(&self.database)
            .application_name(crate::product::PG_APPLICATION_NAME)
            .options(self.options)
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
            match self.socks {
                None => {
                    let tcp = tcp_connect(&self.tcp_host, self.tcp_port)
                        .await
                        .map_err(|e| errors::tcp_error(&e, &self.host, self.port))?;
                    self.handshake(tcp).await
                }
                // Через туннель имя узла разрешает бастион; TLS ниже проверяется по тому же имени.
                Some(proxy) => {
                    let stream = crate::net::ssh::socks::connect(
                        proxy,
                        self.ssh_target.as_deref().unwrap_or_default(),
                        &self.host,
                        self.port,
                    )
                    .await?;
                    self.handshake(stream).await
                }
            }
        };
        tokio::time::timeout(self.timeout, attempt)
            .await
            .map_err(|_| errors::connect_timeout(&self.host, self.port, self.timeout.as_secs()))?
    }

    /// TLS (если нужен) и вход поверх открытого потока: TCP напрямую или SOCKS5 туннеля. Обобщённая функция, а
    /// не `Box<dyn …>`: с потоком-объектом компилятор не доказывает `Send` будущего рукопожатия tokio-postgres.
    async fn handshake<S>(&self, stream: S) -> AppResult<(Client, AbortHandle)>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let cfg = self.pg_config();
        match &self.connector {
            None => {
                let (client, conn) = cfg
                    .connect_raw(stream, NoTls)
                    .await
                    .map_err(|e| self.wire_error(&e, errors::from_pg(&e)))?;
                Ok((client, spawn_connection(conn)))
            }
            Some(connector) => {
                let mut ssl = connector.configure().map_err(errors::tls_setup)?;
                if self.ssl_mode != SslMode::VerifyFull {
                    ssl.set_verify_hostname(false);
                }
                // Причину отказа разбирать по тексту ненадёжно: OpenSSL пишет общее «certificate verify
                // failed», пояснение крейт openssl дописывает в свободной форме. Код причины берём из
                // обратного вызова проверки, чтобы отличить просроченный сертификат от чужого CA. Режим
                // проверки не меняется. Без проверки (`prefer`, `require` без файла CA) отказ не
                // прерывает рукопожатие и ни о чём не говорит: там не запоминаем.
                let verify_error: Arc<Mutex<Option<X509VerifyResult>>> = Arc::default();
                let mode = ssl.verify_mode();
                if mode.contains(SslVerifyMode::PEER) {
                    let slot = verify_error.clone();
                    ssl.set_verify_callback(mode, move |ok, ctx| {
                        if !ok {
                            *slot.lock_safe() = Some(ctx.error());
                        }
                        ok
                    });
                }
                let tls = postgres_openssl::TlsConnector::new(ssl, &self.host);
                let (client, conn) = cfg.connect_raw(stream, tls).await.map_err(|e| {
                    let mapped = errors::from_handshake(&e, verify_error.lock_safe().take());
                    self.wire_error(&e, mapped)
                })?;
                Ok((client, spawn_connection(conn)))
            }
        }
    }

    /// Через туннель недоступный узел виден как конец потока сразу после успеха SOCKS: `ssh -D` отвечает успехом
    /// до открытия канала, а когда бастион не смог подключиться к узлу, закрывает соединение. Такой обрыв до
    /// ответа сервера — сбой проброса (`forwardFailed`), а не ошибка PostgreSQL.
    fn wire_error(&self, e: &tokio_postgres::Error, mapped: AppError) -> AppError {
        if self.socks.is_some() && e.as_db_error().is_none() && errors::closed_early(e) {
            crate::net::ssh::socks::forward_failed(
                self.ssh_target.as_deref().unwrap_or_default(),
                &self.host,
                self.port,
                &errors::chain(e),
            )
        } else {
            mapped
        }
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
        // Сами параметры сеанса: чтение, таймауты, нейтральные `search_path` и `DateStyle` для определений в DDL.
        for opt in [
            "-c default_transaction_read_only=on",
            "-c statement_timeout=5000",
            "-c lock_timeout=1000",
            "-c search_path=pg_catalog",
            "-c DateStyle=ISO",
        ] {
            assert!(SESSION_OPTIONS.contains(opt), "{opt}");
        }
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

    #[test]
    fn action_spec_is_read_write_with_the_same_limits() {
        let spec = loopback_spec(5432, "localhost", SslMode::Disable, None);
        assert_eq!(spec.pg_config().get_options(), Some(SESSION_OPTIONS));
        let action = spec.for_action();
        let opts = action.pg_config().get_options().map(str::to_string);
        assert_eq!(opts.as_deref(), Some(ACTION_SESSION_OPTIONS));
        assert!(!ACTION_SESSION_OPTIONS.contains("default_transaction_read_only"));
        for o in [
            "statement_timeout=5000",
            "lock_timeout=1000",
            "search_path=pg_catalog",
            "DateStyle=ISO",
        ] {
            assert!(ACTION_SESSION_OPTIONS.contains(o), "{o}");
        }
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
            socks: None,
            ssh_target: None,
            options: SESSION_OPTIONS,
        }
    }

    /// Через SOCKS-прокси: TCP — на прокси, узел `host:5432` — по имени, TLS — по тому же имени.
    fn loopback_spec_via(
        proxy: std::net::SocketAddr,
        host: &str,
        mode: SslMode,
        ca: Option<&str>,
    ) -> ConnectSpec {
        let mut p = sample_profile("a");
        p.pg.ssl_mode = mode;
        p.pg.tls.ca_path = ca.map(str::to_string);
        p.ssh = Some(crate::conn::profile::SshSettings {
            target: "bastion".into(),
            port: None,
            user: None,
            auth: crate::conn::profile::SshAuth::Agent,
            key_path: None,
        });
        let spec = ConnectSpec::build_via(
            &p,
            host,
            5432,
            "postgres",
            &MemoryStore::default(),
            Some(proxy),
        )
        .unwrap();
        assert_eq!(
            (spec.socks, spec.ssh_target.as_deref()),
            (Some(proxy), Some("bastion"))
        );
        spec
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

    /// libpq: `require` с корневым сертификатом — то же, что `verify-ca`: сертификат проверяется, имя узла — нет.
    #[tokio::test]
    async fn require_with_ca_verifies_the_certificate_like_verify_ca() {
        let dir = tempfile::tempdir().unwrap();
        let ca = testutil::make_ca("test-ca");
        let ca_path = write_pem(dir.path(), "ca.pem", &ca.cert);
        let server = testutil::tls_server(&testutil::make_server_cert(&ca, "localhost"));
        let spec = loopback_spec(server.port, "127.0.0.1", SslMode::Require, Some(&ca_path));
        assert_handshake_ok(spec.connect().await.unwrap_err());

        let other = testutil::make_ca("other-ca");
        let other_path = write_pem(dir.path(), "other.pem", &other.cert);
        let server = testutil::tls_server(&testutil::make_server_cert(&ca, "localhost"));
        let spec = loopback_spec(
            server.port,
            "localhost",
            SslMode::Require,
            Some(&other_path),
        );
        let e = spec.connect().await.unwrap_err();
        assert_eq!(e.code, ErrorCode::TlsError);
        assert_eq!(e.key, "errors.pg.caUntrusted", "{e:?}");

        let server = testutil::tls_server(&testutil::make_self_signed_server("localhost"));
        let spec = loopback_spec(server.port, "localhost", SslMode::Require, Some(&ca_path));
        assert_eq!(spec.connect().await.unwrap_err().code, ErrorCode::TlsError);
    }

    /// Через SOCKS TLS проверяется по исходному имени узла, а не по адресу прокси (спец. P1 §7).
    #[tokio::test]
    async fn verify_full_via_socks_checks_the_original_host_name() {
        let dir = tempfile::tempdir().unwrap();
        let ca = testutil::make_ca("test-ca");
        let ca_path = write_pem(dir.path(), "ca.pem", &ca.cert);
        let server = testutil::tls_server(&testutil::make_server_cert(&ca, "pg-single"));
        let proxy = testutil::socks_proxy_to(("127.0.0.1", server.port)).await;
        let ok = loopback_spec_via(proxy, "pg-single", SslMode::VerifyFull, Some(&ca_path));
        assert_handshake_ok(ok.connect().await.unwrap_err());
        // Заглушка принимает одно соединение: для второго — своя.
        let server = testutil::tls_server(&testutil::make_server_cert(&ca, "pg-single"));
        let proxy = testutil::socks_proxy_to(("127.0.0.1", server.port)).await;
        let bad = loopback_spec_via(proxy, "other-name", SslMode::VerifyFull, Some(&ca_path));
        let e = bad.connect().await.unwrap_err();
        assert_eq!(e.key, "errors.pg.hostnameMismatch", "{e:?}");
    }

    /// Без туннеля спецификация прежняя: ни прокси, ни цели ssh, даже если у профиля есть ssh.
    #[test]
    fn build_without_socks_has_no_tunnel() {
        let mut p = sample_profile("a");
        p.ssh = Some(crate::conn::profile::SshSettings {
            target: "bastion".into(),
            port: None,
            user: None,
            auth: crate::conn::profile::SshAuth::Agent,
            key_path: None,
        });
        let spec = ConnectSpec::build(&p, "h", 5432, "postgres", &MemoryStore::default()).unwrap();
        assert_eq!((spec.socks, spec.ssh_target.as_deref()), (None, None));
        assert!(format!("{spec:?}").contains("socks: None"));
    }

    #[tokio::test]
    async fn tcp_connection_has_keepalive_and_nodelay() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stream = tcp_connect("127.0.0.1", port).await.unwrap();
        let sock = socket2::SockRef::from(&stream);
        assert!(sock.keepalive().unwrap());
        // Windows прочитать время keepalive не даёт.
        #[cfg(not(windows))]
        assert_eq!(sock.tcp_keepalive_time().unwrap(), Duration::from_secs(60));
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert_eq!(
            sock.tcp_keepalive_interval().unwrap(),
            Duration::from_secs(10)
        );
        assert!(stream.nodelay().unwrap());
    }

    /// Просроченный сертификат отличается от чужого CA по коду причины из обратного вызова проверки,
    /// а не по тексту ошибки.
    #[tokio::test]
    async fn expired_server_certificate_is_reported_as_expired() {
        let dir = tempfile::tempdir().unwrap();
        let ca = testutil::make_ca("test-ca");
        let ca_path = write_pem(dir.path(), "ca.pem", &ca.cert);
        let server = testutil::tls_server(&testutil::make_expired_server_cert(&ca, "localhost"));
        let spec = loopback_spec(server.port, "localhost", SslMode::VerifyCa, Some(&ca_path));
        let e = spec.connect().await.unwrap_err();
        assert_eq!(e.code, ErrorCode::TlsError);
        assert_eq!(e.key, "errors.pg.certExpired", "{e:?}");
        let details = e.details.unwrap();
        assert_eq!(
            details.matches("certificate has expired").count(),
            1,
            "причина в подробностях ровно один раз: {details}"
        );

        // Чужой CA по-прежнему «недоверенный», а не «просроченный»: цепочка не строится раньше проверки срока.
        let other = testutil::make_ca("other-ca");
        let other_path = write_pem(dir.path(), "other.pem", &other.cert);
        let server = testutil::tls_server(&testutil::make_expired_server_cert(&ca, "localhost"));
        let e = loopback_spec(
            server.port,
            "localhost",
            SslMode::VerifyCa,
            Some(&other_path),
        )
        .connect()
        .await
        .unwrap_err();
        assert_eq!(e.key, "errors.pg.caUntrusted", "{e:?}");
    }

    #[tokio::test]
    async fn certificate_from_the_future_is_reported_as_not_yet_valid() {
        let dir = tempfile::tempdir().unwrap();
        let ca = testutil::make_ca("test-ca");
        let ca_path = write_pem(dir.path(), "ca.pem", &ca.cert);
        let server =
            testutil::tls_server(&testutil::make_not_yet_valid_server_cert(&ca, "localhost"));
        let spec = loopback_spec(
            server.port,
            "localhost",
            SslMode::VerifyFull,
            Some(&ca_path),
        );
        let e = spec.connect().await.unwrap_err();
        assert_eq!(e.code, ErrorCode::TlsError);
        assert_eq!(e.key, "errors.pg.certNotYetValid", "{e:?}");
    }

    /// `prefer` сертификат не проверяет даже с файлом CA: отказ проверки, которого не было, не подменяет
    /// настоящую ошибку (здесь — ответ сервера после рукопожатия).
    #[tokio::test]
    async fn prefer_with_ca_ignores_an_expired_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let ca = testutil::make_ca("test-ca");
        let ca_path = write_pem(dir.path(), "ca.pem", &ca.cert);
        let server = testutil::tls_server(&testutil::make_expired_server_cert(&ca, "localhost"));
        let spec = loopback_spec(server.port, "localhost", SslMode::Prefer, Some(&ca_path));
        assert_handshake_ok(spec.connect().await.unwrap_err());
    }

    /// То же без ответа сервера: после рукопожатия соединение просто закрыто. Причина отказа проверки, которой
    /// при `prefer` не было (обратный вызов ставится только при `PEER`), не подменяет обрыв на «срок истёк».
    #[tokio::test]
    async fn prefer_with_ca_and_closed_connection_is_not_reported_as_expired() {
        let dir = tempfile::tempdir().unwrap();
        let ca = testutil::make_ca("test-ca");
        let ca_path = write_pem(dir.path(), "ca.pem", &ca.cert);
        let server = testutil::tls_server_closing_after_handshake(
            &testutil::make_expired_server_cert(&ca, "localhost"),
        );
        let spec = loopback_spec(server.port, "localhost", SslMode::Prefer, Some(&ca_path));
        let e = spec.connect().await.unwrap_err();
        assert_ne!(e.key, "errors.pg.certExpired", "{e:?}");
        assert_eq!(e.code, ErrorCode::ConnectionFailed, "{e:?}");
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

    #[tokio::test]
    async fn error_of_a_closed_connection_is_suspect() {
        let server = testutil::stalling_pg_server();
        let spec = loopback_spec(server.port, "127.0.0.1", SslMode::Disable, None);
        let (client, abort) = spec.connect_with_handle().await.unwrap();
        abort.abort();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !client.is_closed() {
            assert!(
                std::time::Instant::now() < deadline,
                "соединение не закрылось"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let e = client.simple_query("SELECT 1").await.unwrap_err();
        assert!(errors::connection_suspect(&e), "{e:?}");
    }
}
