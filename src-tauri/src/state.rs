use std::path::Path;
use std::sync::Arc;

use crate::error::AppResult;
use crate::license::LicenseManager;
use crate::secrets::SecretStore;
use crate::settings::SettingsStore;
use crate::storage::connections::{ConnectionProfile, ConnectionStore};

#[derive(Clone)]
pub struct AppState {
    pub connections: Arc<ConnectionStore>,
    pub secrets: Arc<dyn SecretStore>,
    pub policy: Arc<crate::policy::Policy>,
    pub license: Arc<LicenseManager>,
    pub settings: Arc<SettingsStore>,
}

impl AppState {
    pub fn new(data_dir: &Path, secrets: Arc<dyn SecretStore>) -> AppResult<Self> {
        std::fs::create_dir_all(data_dir)?;
        Ok(Self {
            connections: Arc::new(ConnectionStore::open(data_dir)?),
            secrets,
            policy: Arc::new(crate::policy::Policy::default()),
            license: Arc::new(LicenseManager::open(
                data_dir,
                LicenseManager::embedded_key(),
            )),
            settings: Arc::new(SettingsStore::open(data_dir)),
        })
    }

    pub fn profile(&self, id: &str) -> AppResult<ConnectionProfile> {
        self.connections.get(id)
    }

    /// Удаляет подключение и все его секреты (блокирующий вызов: keychain).
    pub fn delete_connection_full(&self, id: &str) -> AppResult<()> {
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
mod tests {
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
