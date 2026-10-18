use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::error::{AppError, AppResult, ErrorCode};
use crate::sync::MutexExt;

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> AppResult<Option<String>>;
    fn set(&self, key: &str, value: &str) -> AppResult<()>;
    fn delete(&self, key: &str) -> AppResult<()>;
}

/// Пароль PostgreSQL.
pub const PG_PASSWORD: &str = "pgPassword";
/// Пароль клиентского ключа PostgreSQL (PKCS#12 или зашифрованный PEM).
pub const PG_KEY_PASSWORD: &str = "pgKeyPassword";
/// Пароль basic auth Patroni REST API.
pub const PATRONI_PASSWORD: &str = "patroniPassword";
/// Пароль клиентского ключа Patroni REST API.
pub const PATRONI_KEY_PASSWORD: &str = "patroniKeyPassword";

/// Все секреты подключения в keychain (для удаления).
pub const CONNECTION_SECRET_FIELDS: [&str; 4] = [
    PG_PASSWORD,
    PG_KEY_PASSWORD,
    PATRONI_PASSWORD,
    PATRONI_KEY_PASSWORD,
];

pub fn secret_key(connection_id: &str, field: &str) -> String {
    format!("{connection_id}/{field}")
}

/// OS keychain: macOS Keychain, Windows Credential Manager, Secret Service.
/// `service` — имя сервиса записей.
pub struct KeyringStore {
    pub service: &'static str,
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self {
            service: crate::product::KEYCHAIN_SERVICE,
        }
    }
}

fn keyring_err(e: keyring::Error) -> AppError {
    app_err!(
        ErrorCode::Internal,
        "errors.secrets.keychain",
        "Cannot access the system secret store"
    )
    .with_details(e.to_string())
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> AppResult<Option<String>> {
        let entry = keyring::Entry::new(self.service, key).map_err(keyring_err)?;
        match entry.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keyring_err(e)),
        }
    }

    fn set(&self, key: &str, value: &str) -> AppResult<()> {
        keyring::Entry::new(self.service, key)
            .and_then(|e| e.set_password(value))
            .map_err(keyring_err)
    }

    fn delete(&self, key: &str) -> AppResult<()> {
        let entry = keyring::Entry::new(self.service, key).map_err(keyring_err)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keyring_err(e)),
        }
    }
}

#[derive(Default)]
pub struct MemoryStore {
    values: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> AppResult<Option<String>> {
        Ok(self.values.lock_safe().get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> AppResult<()> {
        self.values
            .lock_safe()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) -> AppResult<()> {
        self.values.lock_safe().remove(key);
        Ok(())
    }
}

/// Несохранённые значения из формы поверх keychain (проверка подключения до сохранения).
/// `Some("")` в `overrides` — «пароль удалён».
pub struct OverlayStore {
    pub base: Arc<dyn SecretStore>,
    pub overrides: HashMap<String, String>,
}

impl SecretStore for OverlayStore {
    fn get(&self, key: &str) -> AppResult<Option<String>> {
        match self.overrides.get(key) {
            Some(v) if v.is_empty() => Ok(None),
            Some(v) => Ok(Some(v.clone())),
            None => self.base.get(key),
        }
    }

    fn set(&self, _: &str, _: &str) -> AppResult<()> {
        Err(app_err!(
            ErrorCode::Internal,
            "errors.secrets.readOnly",
            "OverlayStore is read-only"
        ))
    }

    fn delete(&self, _: &str) -> AppResult<()> {
        Err(app_err!(
            ErrorCode::Internal,
            "errors.secrets.readOnly",
            "OverlayStore is read-only"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip() {
        let s = MemoryStore::default();
        assert_eq!(s.get("a").unwrap(), None);
        s.set("a", "1").unwrap();
        assert_eq!(s.get("a").unwrap(), Some("1".to_string()));
        s.delete("a").unwrap();
        s.delete("a").unwrap(); // повторное удаление — не ошибка
        assert_eq!(s.get("a").unwrap(), None);
    }

    #[test]
    fn key_format() {
        assert_eq!(secret_key("c1", PG_PASSWORD), "c1/pgPassword");
    }

    #[test]
    fn overlay_prefers_overrides_and_empty_means_absent() {
        let base = Arc::new(MemoryStore::default());
        base.set("c/a", "base-a").unwrap();
        base.set("c/b", "base-b").unwrap();
        let mut overrides = HashMap::new();
        overrides.insert("c/a".to_string(), "new-a".to_string());
        overrides.insert("c/b".to_string(), String::new());
        let o = OverlayStore { base, overrides };
        assert_eq!(o.get("c/a").unwrap().as_deref(), Some("new-a"));
        assert_eq!(
            o.get("c/b").unwrap(),
            None,
            "пустое значение — пароль удалён"
        );
        assert_eq!(o.get("c/x").unwrap(), None);
    }
}
