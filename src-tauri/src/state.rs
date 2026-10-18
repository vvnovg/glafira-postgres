use std::path::Path;
use std::sync::Arc;

use crate::error::AppResult;
use crate::license::LicenseManager;
use crate::secrets::SecretStore;
use crate::settings::SettingsStore;

#[derive(Clone)]
pub struct AppState {
    pub secrets: Arc<dyn SecretStore>,
    pub policy: Arc<crate::policy::Policy>,
    pub license: Arc<LicenseManager>,
    pub settings: Arc<SettingsStore>,
}

impl AppState {
    pub fn new(data_dir: &Path, secrets: Arc<dyn SecretStore>) -> AppResult<Self> {
        std::fs::create_dir_all(data_dir)?;
        Ok(Self {
            secrets,
            policy: Arc::new(crate::policy::Policy::default()),
            license: Arc::new(LicenseManager::open(
                data_dir,
                LicenseManager::embedded_key(),
            )),
            settings: Arc::new(SettingsStore::open(data_dir)),
        })
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
}
