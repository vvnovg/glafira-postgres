use std::path::Path;
use std::sync::Arc;

use crate::conn::pool::{PoolRegistry, PooledClient};
use crate::error::AppResult;
use crate::license::LicenseManager;
use crate::secrets::SecretStore;
use crate::settings::SettingsStore;
use crate::storage::connections::{ConnectionProfile, ConnectionStore};
use crate::topology::runtime::{LiveSource, Runtimes};

#[derive(Clone)]
pub struct AppState {
    pub connections: Arc<ConnectionStore>,
    pub secrets: Arc<dyn SecretStore>,
    pub policy: Arc<crate::policy::Policy>,
    pub license: Arc<LicenseManager>,
    pub settings: Arc<SettingsStore>,
    /// Пулы соединений с узлами открытых подключений.
    pub pools: Arc<PoolRegistry>,
    /// Открытые подключения: топология, журнал событий, узел каталога.
    pub runtimes: Arc<Runtimes>,
}

impl AppState {
    pub fn new(data_dir: &Path, secrets: Arc<dyn SecretStore>) -> AppResult<Self> {
        std::fs::create_dir_all(data_dir)?;
        let pools = Arc::new(PoolRegistry::default());
        let runtimes = Runtimes::new(
            Arc::new(LiveSource::new(secrets.clone(), pools.clone())),
            pools.clone(),
        );
        Ok(Self {
            connections: Arc::new(ConnectionStore::open(data_dir)?),
            secrets,
            policy: Arc::new(crate::policy::Policy::default()),
            license: Arc::new(LicenseManager::open(
                data_dir,
                LicenseManager::embedded_key(),
            )),
            settings: Arc::new(SettingsStore::open(data_dir)),
            pools,
            runtimes,
        })
    }

    pub fn profile(&self, id: &str) -> AppResult<ConnectionProfile> {
        self.connections.get(id)
    }

    /// Соединение с узлом открытого подключения: `member = None` — узел каталога,
    /// `database = None` — БД по умолчанию профиля. Если запрос на этом соединении не удался из-за
    /// таймаута или обрыва, вызывающий закрывает его `PooledClient::discard`, а не возвращает в пул.
    pub async fn client(
        &self,
        connection_id: &str,
        member: Option<&str>,
        database: Option<&str>,
    ) -> AppResult<PooledClient> {
        let (m, profile) = self.runtimes.target(connection_id, member)?;
        let db = database
            .map(str::to_string)
            .unwrap_or_else(|| profile.pg.database.clone());
        crate::topology::connector::pooled_client(
            &self.pools,
            &profile,
            self.secrets.clone(),
            &m.name,
            &m.host,
            m.port,
            &db,
        )
        .await
    }

    /// Удаляет подключение и все его секреты (блокирующий вызов: keychain).
    pub fn delete_connection_full(&self, id: &str) -> AppResult<()> {
        self.runtimes.disconnect(id);
        self.connections.delete(id)?;
        for field in crate::secrets::CONNECTION_SECRET_FIELDS {
            self.secrets
                .delete(&crate::secrets::secret_key(id, field))?;
        }
        Ok(())
    }

    /// Политика администратора (читается при запуске в lib.rs).
    pub fn with_policy(mut self, policy: crate::policy::Policy) -> Self {
        self.license.set_policy_key(policy.license_key.clone());
        self.policy = Arc::new(policy);
        self
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::secrets::MemoryStore;

    #[test]
    fn policy_license_key_reaches_manager() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        st.license = Arc::new(LicenseManager::open(
            dir.path(),
            license_format::dev::verifying_key(),
        ));
        let policy = crate::policy::Policy {
            license_key: Some(license_format::dev::pro_token("ACME", None)),
            ..Default::default()
        };
        let st = st.with_policy(policy);
        assert_eq!(st.license.status().licensee.as_deref(), Some("ACME"));
    }

    /// Приёмник, запоминающий закрытия подключений.
    #[derive(Default)]
    pub(crate) struct Closed(pub std::sync::Mutex<Vec<String>>);

    impl crate::topology::runtime::RuntimeSink for Closed {
        fn state_changed(
            &self,
            id: &str,
            state: crate::topology::runtime::ConnState,
            _e: Option<&crate::error::AppError>,
        ) {
            if state == crate::topology::runtime::ConnState::Disconnected {
                self.0.lock().unwrap().push(id.to_string());
            }
        }
        fn topology_changed(
            &self,
            _id: &str,
            _v: &crate::topology::runtime::TopologyView,
            _e: &[crate::topology::journal::TopologyEvent],
        ) {
        }
    }

    #[test]
    fn deleting_a_connection_closes_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        let closed = Arc::new(Closed::default());
        state.runtimes.set_sink(closed.clone());
        state
            .connections
            .upsert(crate::conn::profile::sample_profile("a"))
            .unwrap();
        state.delete_connection_full("a").unwrap();
        assert_eq!(*closed.0.lock().unwrap(), vec!["a".to_string()]);
    }

    #[tokio::test]
    async fn client_needs_an_open_connection() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        let e = state.client("nope", None, None).await.err().unwrap();
        assert_eq!(e.key, "errors.topology.notConnected");
    }

    #[test]
    fn delete_connection_removes_all_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        state
            .connections
            .upsert(crate::conn::profile::sample_profile("a"))
            .unwrap();
        for f in crate::secrets::CONNECTION_SECRET_FIELDS {
            state
                .secrets
                .set(&crate::secrets::secret_key("a", f), "x")
                .unwrap();
        }
        state.delete_connection_full("a").unwrap();
        assert!(state.connections.get("a").is_err());
        for f in crate::secrets::CONNECTION_SECRET_FIELDS {
            assert_eq!(
                state
                    .secrets
                    .get(&crate::secrets::secret_key("a", f))
                    .unwrap(),
                None,
                "{f}"
            );
        }
    }
}
