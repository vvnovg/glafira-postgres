use std::collections::HashMap;

use serde::Deserialize;
use tauri::State;

use super::blocking;
use crate::conn::profile::{ConnectionProfile, Environment, normalize_profile};
use crate::error::AppResult;
use crate::secrets::{
    CONNECTION_SECRET_FIELDS, PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, PG_KEY_PASSWORD, PG_PASSWORD,
    secret_key,
};
use crate::state::AppState;

/// Секреты из формы. У каждого поля: `null`/нет — оставить сохранённый, `""` — удалить, иное — сохранить.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsInput {
    #[serde(default)]
    pub pg_password: Option<String>,
    #[serde(default)]
    pub pg_key_password: Option<String>,
    #[serde(default)]
    pub patroni_password: Option<String>,
    #[serde(default)]
    pub patroni_key_password: Option<String>,
}

impl SecretsInput {
    fn get(&self, field: &str) -> Option<&String> {
        match field {
            PG_PASSWORD => self.pg_password.as_ref(),
            PG_KEY_PASSWORD => self.pg_key_password.as_ref(),
            PATRONI_PASSWORD => self.patroni_password.as_ref(),
            PATRONI_KEY_PASSWORD => self.patroni_key_password.as_ref(),
            _ => None,
        }
    }

    /// Значения формы поверх keychain (`OverlayStore`) для проверки несохранённого профиля.
    pub fn overrides(&self, connection_id: &str) -> HashMap<String, String> {
        CONNECTION_SECRET_FIELDS
            .iter()
            .filter_map(|f| {
                self.get(f)
                    .map(|v| (secret_key(connection_id, f), v.clone()))
            })
            .collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInput {
    pub profile: ConnectionProfile,
    #[serde(default)]
    pub secrets: SecretsInput,
}

/// Что делать с секретом при сохранении.
enum SecretOp {
    Keep,
    Set(String),
    Delete,
}

/// Используемые профилем секреты — по данным формы, неиспользуемые — удаляются.
fn secret_ops(
    profile: &ConnectionProfile,
    secrets: &SecretsInput,
) -> Vec<(&'static str, SecretOp)> {
    CONNECTION_SECRET_FIELDS
        .iter()
        .map(|&field| {
            let op = if !profile.uses_secret(field) {
                SecretOp::Delete
            } else {
                match secrets.get(field) {
                    None => SecretOp::Keep,
                    Some(v) if v.is_empty() => SecretOp::Delete,
                    Some(v) => SecretOp::Set(v.clone()),
                }
            };
            (field, op)
        })
        .collect()
}

#[tauri::command]
pub fn list_connections(state: State<'_, AppState>) -> AppResult<Vec<ConnectionProfile>> {
    Ok(state.connections.list())
}

#[tauri::command]
pub async fn save_connection(
    state: State<'_, AppState>,
    input: ConnectionInput,
) -> AppResult<ConnectionProfile> {
    let st = state.inner().clone();
    blocking(move || save_connection_blocking(&st, input)).await
}

/// Сохраняет профиль и секреты согласованно: если keychain не принял изменение,
/// профиль возвращается к прежнему состоянию (или удаляется, если он новый).
pub fn save_connection_blocking(
    st: &AppState,
    input: ConnectionInput,
) -> AppResult<ConnectionProfile> {
    let ConnectionInput {
        mut profile,
        secrets,
    } = input;
    if profile.id.is_empty() {
        profile.id = uuid::Uuid::new_v4().to_string();
    }
    normalize_profile(&mut profile);
    let previous = st.connections.get(&profile.id).ok();
    // Перевод в prod и снятие read-only с prod — функции Pro (как в Glafira for Kafka).
    let was_prod = previous.as_ref().map(|p| p.environment) == Some(Environment::Prod);
    let becomes_prod = profile.environment == Environment::Prod && !was_prod;
    let becomes_writable = profile.environment == Environment::Prod
        && was_prod
        && previous.as_ref().is_some_and(|p| p.read_only)
        && !profile.read_only;
    if becomes_prod || becomes_writable {
        st.license.require_pro()?;
    }
    let saved = st.connections.upsert(profile)?;
    let result = secret_ops(&saved, &secrets)
        .into_iter()
        .try_for_each(|(field, op)| {
            let key = secret_key(&saved.id, field);
            match op {
                SecretOp::Keep => Ok(()),
                SecretOp::Set(v) => st.secrets.set(&key, &v),
                SecretOp::Delete => st.secrets.delete(&key),
            }
        });
    if let Err(e) = result {
        let rollback = match previous {
            Some(p) => st.connections.upsert(p).map(|_| ()),
            None => st.connections.delete(&saved.id),
        };
        if let Err(r) = rollback {
            tracing::error!("failed to roll back profile {}: {r}", saved.id);
        }
        return Err(e);
    }
    Ok(saved)
}

#[tauri::command]
pub async fn delete_connection(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let st = state.inner().clone();
    blocking(move || st.delete_connection_full(&id)).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::conn::profile::{CertFormat, ClientCert, sample_profile};
    use crate::error::{AppError, ErrorCode};
    use crate::secrets::{MemoryStore, SecretStore};

    /// Keychain, в который ничего нельзя записать.
    struct BrokenKeychain;

    impl SecretStore for BrokenKeychain {
        fn get(&self, _: &str) -> AppResult<Option<String>> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &str) -> AppResult<()> {
            Err(AppError::internal_with("keychain unavailable"))
        }
        fn delete(&self, _: &str) -> AppResult<()> {
            Err(AppError::internal_with("keychain unavailable"))
        }
    }

    fn state_with(secrets: Arc<dyn SecretStore>) -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), secrets).unwrap();
        (dir, st)
    }

    fn input(profile: ConnectionProfile, secrets: SecretsInput) -> ConnectionInput {
        ConnectionInput { profile, secrets }
    }

    #[test]
    fn new_profile_gets_id_and_password() {
        let (_d, st) = state_with(Arc::new(MemoryStore::default()));
        let mut p = sample_profile("");
        p.id = String::new();
        let saved = save_connection_blocking(
            &st,
            input(
                p,
                SecretsInput {
                    pg_password: Some("pw".into()),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        assert!(!saved.id.is_empty());
        assert_eq!(
            st.secrets
                .get(&secret_key(&saved.id, PG_PASSWORD))
                .unwrap()
                .as_deref(),
            Some("pw")
        );
    }

    #[test]
    fn empty_secret_deletes_and_none_keeps() {
        let (_d, st) = state_with(Arc::new(MemoryStore::default()));
        let p = sample_profile("a");
        save_connection_blocking(
            &st,
            input(
                p.clone(),
                SecretsInput {
                    pg_password: Some("pw".into()),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        save_connection_blocking(&st, input(p.clone(), SecretsInput::default())).unwrap();
        assert_eq!(
            st.secrets
                .get(&secret_key("a", PG_PASSWORD))
                .unwrap()
                .as_deref(),
            Some("pw")
        );
        save_connection_blocking(
            &st,
            input(
                p,
                SecretsInput {
                    pg_password: Some("".into()),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        assert_eq!(st.secrets.get(&secret_key("a", PG_PASSWORD)).unwrap(), None);
    }

    #[test]
    fn unused_secrets_are_deleted() {
        let (_d, st) = state_with(Arc::new(MemoryStore::default()));
        let mut p = sample_profile("a");
        p.pg.tls.client_cert = Some(ClientCert {
            format: CertFormat::Pkcs12,
            certificate_path: if cfg!(windows) {
                "C:\\c.p12".into()
            } else {
                "/c.p12".into()
            },
            key_path: None,
        });
        save_connection_blocking(
            &st,
            input(
                p.clone(),
                SecretsInput {
                    pg_key_password: Some("k".into()),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        assert!(
            st.secrets
                .get(&secret_key("a", PG_KEY_PASSWORD))
                .unwrap()
                .is_some()
        );
        p.pg.tls.client_cert = None;
        save_connection_blocking(&st, input(p, SecretsInput::default())).unwrap();
        assert_eq!(
            st.secrets.get(&secret_key("a", PG_KEY_PASSWORD)).unwrap(),
            None
        );
    }

    #[test]
    fn keychain_failure_rolls_back_new_profile() {
        let (_d, st) = state_with(Arc::new(BrokenKeychain));
        let err = save_connection_blocking(
            &st,
            input(
                sample_profile("a"),
                SecretsInput {
                    pg_password: Some("pw".into()),
                    ..Default::default()
                },
            ),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(st.connections.get("a").is_err(), "новый профиль удалён");
    }

    #[test]
    fn prod_requires_pro() {
        let (_d, st) = state_with(Arc::new(MemoryStore::default()));
        let mut p = sample_profile("a");
        p.environment = Environment::Prod;
        p.read_only = true;
        let err = save_connection_blocking(&st, input(p, SecretsInput::default())).unwrap_err();
        assert_eq!(err.code, ErrorCode::ProRequired);
    }

    #[test]
    fn overrides_cover_only_given_fields() {
        let s = SecretsInput {
            pg_password: Some("".into()),
            patroni_password: Some("x".into()),
            ..Default::default()
        };
        let o = s.overrides("id");
        assert_eq!(o.get("id/pgPassword").map(String::as_str), Some(""));
        assert_eq!(o.get("id/patroniPassword").map(String::as_str), Some("x"));
        assert!(!o.contains_key("id/pgKeyPassword"));
    }
}
