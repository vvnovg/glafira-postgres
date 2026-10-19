//! Соединение с PostgreSQL: параметры из профиля и keychain, TCP, TLS, таймаут (спец. P1 §7).

use std::time::Duration;

use openssl::ssl::SslConnector;
use tokio::net::TcpStream;
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
                    spawn_connection(conn);
                    Ok(client)
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
                    spawn_connection(conn);
                    Ok(client)
                }
            }
        };
        tokio::time::timeout(self.timeout, attempt)
            .await
            .map_err(|_| errors::connect_timeout(&self.host, self.port, self.timeout.as_secs()))?
    }
}

fn spawn_connection<F>(conn: F)
where
    F: std::future::Future<Output = Result<(), tokio_postgres::Error>> + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!("pg connection ended: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::sample_profile;
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
}
