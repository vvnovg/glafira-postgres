use std::path::Path;
use std::sync::Arc;

use crate::conn::pool::{PoolRegistry, PooledClient};
use crate::error::AppResult;
use crate::secrets::SecretStore;
use crate::settings::SettingsStore;
use crate::storage::connections::{ConnectionProfile, ConnectionStore};
use crate::topology::runtime::{LiveSource, Runtimes};

#[derive(Clone)]
pub struct AppState {
    pub connections: Arc<ConnectionStore>,
    pub secrets: Arc<dyn SecretStore>,
    pub policy: Arc<crate::policy::Policy>,
    pub settings: Arc<SettingsStore>,
    /// Пулы соединений с узлами открытых подключений.
    pub pools: Arc<PoolRegistry>,
    /// Открытые подключения: топология, журнал событий, узел каталога.
    pub runtimes: Arc<Runtimes>,
}

impl AppState {
    pub fn new(data_dir: &Path, secrets: Arc<dyn SecretStore>) -> AppResult<Self> {
        std::fs::create_dir_all(data_dir)?;
        crate::migrate::remove_legacy_license_file(data_dir);
        let pools = Arc::new(PoolRegistry::default());
        let runtimes = Runtimes::new(
            Arc::new(LiveSource::new(secrets.clone(), pools.clone())),
            pools.clone(),
        );
        Ok(Self {
            connections: Arc::new(ConnectionStore::open(data_dir)?),
            secrets,
            policy: Arc::new(crate::policy::Policy::default()),
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

    /// Удаляет подключение и все его секреты (блокирующий вызов: keychain). Сначала секреты, потом профиль:
    /// если keychain откажет, подключение останется в дереве и удаление можно повторить, а не останутся
    /// «ничьи» пароли, которые подхватило бы подключение с тем же id из импортируемого файла. Отсутствующая
    /// запись keychain ошибкой не считается.
    pub fn delete_connection_full(&self, id: &str) -> AppResult<()> {
        self.runtimes.disconnect(id);
        for field in crate::secrets::CONNECTION_SECRET_FIELDS {
            self.secrets
                .delete(&crate::secrets::secret_key(id, field))?;
        }
        self.connections.delete(id)
    }

    /// Политика администратора (читается при запуске в lib.rs).
    pub fn with_policy(mut self, policy: crate::policy::Policy) -> Self {
        self.policy = Arc::new(policy);
        self
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::secrets::MemoryStore;

    #[test]
    fn legacy_license_file_is_removed_on_start() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("license.json"), "{}").unwrap();
        AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        assert!(!dir.path().join("license.json").exists());
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

    /// Keychain, который не умеет удалять.
    struct NoDelete(MemoryStore);

    impl SecretStore for NoDelete {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            self.0.get(key)
        }
        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            self.0.set(key, value)
        }
        fn delete(&self, _: &str) -> AppResult<()> {
            Err(crate::error::AppError::internal_with(
                "keychain unavailable",
            ))
        }
    }

    #[test]
    fn keychain_failure_keeps_the_connection_so_no_secret_is_orphaned() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path(), Arc::new(NoDelete(MemoryStore::default()))).unwrap();
        state
            .connections
            .upsert(crate::conn::profile::sample_profile("a"))
            .unwrap();
        state
            .secrets
            .set(
                &crate::secrets::secret_key("a", crate::secrets::PG_PASSWORD),
                "pw",
            )
            .unwrap();
        assert!(state.delete_connection_full("a").is_err());
        assert!(
            state.connections.get("a").is_ok(),
            "профиль на месте: удаление можно повторить"
        );
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
