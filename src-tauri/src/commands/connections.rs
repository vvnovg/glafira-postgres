use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;
use tauri::State;

use super::blocking;
use crate::conn::profile::{
    ConnectionProfile, Environment, normalize_profile, same_connection_settings,
};
use crate::error::AppResult;
use crate::readiness::check::{TestEvent, TestReport};
use crate::secrets::{
    CONNECTION_SECRET_FIELDS, MemoryStore, OverlayStore, PATRONI_KEY_PASSWORD, PATRONI_PASSWORD,
    PG_KEY_PASSWORD, PG_PASSWORD, SecretStore, secret_key,
};
use crate::state::AppState;

/// Секреты из формы. У каждого поля: `null`/нет — оставить сохранённый, `""` — удалить, иное — сохранить.
#[derive(Default, Clone, Deserialize)]
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

/// Значения в отладочный вывод не попадают: видно только, что с полем делают (оставить, удалить, задать).
impl std::fmt::Debug for SecretsInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let op = |v: &Option<String>| match v {
            None => "keep",
            Some(s) if s.is_empty() => "delete",
            Some(_) => "***",
        };
        f.debug_struct("SecretsInput")
            .field("pg_password", &op(&self.pg_password))
            .field("pg_key_password", &op(&self.pg_key_password))
            .field("patroni_password", &op(&self.patroni_password))
            .field("patroni_key_password", &op(&self.patroni_key_password))
            .finish()
    }
}

impl SecretsInput {
    /// «Удалить все сохранённые секреты»: пустое значение у каждого поля.
    pub(crate) fn clear_all() -> Self {
        Self {
            pg_password: Some(String::new()),
            pg_key_password: Some(String::new()),
            patroni_password: Some(String::new()),
            patroni_key_password: Some(String::new()),
        }
    }

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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInput {
    pub profile: ConnectionProfile,
    #[serde(default)]
    pub secrets: SecretsInput,
}

// Профиль секретов не содержит; секреты — только отметками (см. `SecretsInput`).
impl std::fmt::Debug for ConnectionInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionInput")
            .field("profile", &self.profile)
            .field("secrets", &self.secrets)
            .finish()
    }
}

/// Что делать с секретом при сохранении.
pub(crate) enum SecretOp {
    Keep,
    Set(String),
    Delete,
}

/// Используемые профилем секреты — по данным формы, неиспользуемые — удаляются.
pub(crate) fn secret_ops(
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

/// Применяет операции над секретами подключения `id` в keychain; первая ошибка прерывает работу.
pub(crate) fn apply_secret_ops(
    store: &dyn SecretStore,
    id: &str,
    ops: Vec<(&'static str, SecretOp)>,
) -> AppResult<()> {
    ops.into_iter().try_for_each(|(field, op)| {
        let key = secret_key(id, field);
        match op {
            SecretOp::Keep => Ok(()),
            SecretOp::Set(v) => store.set(&key, &v),
            SecretOp::Delete => store.delete(&key),
        }
    })
}

/// Удаляет из keychain секреты, которые подключению не нужны; с `reset` — все секреты подключения
/// (импорт файла поменял его адрес или настройки безопасности, пароли получателя за новым адресом
/// оставлять нельзя). Остальное не трогает.
pub(crate) fn delete_stale_secrets(
    store: &dyn SecretStore,
    profile: &ConnectionProfile,
    reset: bool,
) -> AppResult<()> {
    let input = if reset {
        SecretsInput::clear_all()
    } else {
        SecretsInput::default()
    };
    apply_secret_ops(store, &profile.id, secret_ops(profile, &input))
}

/// Менял ли сохранённый секрет хоть одно используемое профилем поле (`Set` или `Delete`): тогда открытое
/// подключение работает со старым значением и должно быть переоткрыто. Неиспользуемые поля удаляются при
/// каждом сохранении — на подключение это не влияет.
fn secrets_changed(profile: &ConnectionProfile, ops: &[(&'static str, SecretOp)]) -> bool {
    ops.iter()
        .any(|(field, op)| profile.uses_secret(field) && !matches!(op, SecretOp::Keep))
}

/// Нужно ли закрыть открытое подключение после сохранения: изменились настройки соединения
/// (или профиль новый) либо секрет, которым пользуется профиль. Имя, цвет, окружение, папка и порядок
/// соединения не касаются.
fn needs_reconnect(
    previous: Option<&ConnectionProfile>,
    saved: &ConnectionProfile,
    secrets_changed: bool,
) -> bool {
    secrets_changed || previous.is_none_or(|p| !same_connection_settings(p, saved))
}

/// Синхронная команда Tauri выполняется в главном потоке, а блокировку хранилища держит запись файла
/// (с fsync): чтение ждёт её в фоновом потоке, не замораживая окно.
#[tauri::command]
pub async fn list_connections(state: State<'_, AppState>) -> AppResult<Vec<ConnectionProfile>> {
    let st = state.inner().clone();
    blocking(move || Ok(st.connections.list())).await
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
    let ops = secret_ops(&saved, &secrets);
    let secrets_changed = secrets_changed(&saved, &ops);
    let result = apply_secret_ops(st.secrets.as_ref(), &saved.id, ops);
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
    if needs_reconnect(previous.as_ref(), &saved, secrets_changed) {
        // Настройки соединения или секреты изменились: подключение переоткроется при следующем раскрытии.
        st.runtimes.disconnect(&saved.id);
    } else {
        // Правка оформления: подключение, журнал и узел каталога остаются, обновляется только профиль.
        st.runtimes.refresh_profile(&saved);
    }
    Ok(saved)
}

#[tauri::command]
pub fn parse_connection_string(text: String) -> AppResult<crate::conn::connstr::ParsedConnection> {
    crate::conn::connstr::parse_connection_string(&text)
}

#[tauri::command]
pub async fn delete_connection(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let st = state.inner().clone();
    blocking(move || st.delete_connection_full(&id)).await
}

/// Секреты для проверки: значения формы поверх keychain. Несохранённому профилю (`id` пуст) keychain не
/// нужен вовсе: он получает случайный id и пустую основу — иначе проверка подхватила бы записи подключения,
/// чей настоящий id совпал с подставленным (импорт или сохранение могут дать подключению любой id).
fn test_secrets(
    keychain: Arc<dyn SecretStore>,
    profile: &mut ConnectionProfile,
    secrets: &SecretsInput,
) -> Arc<dyn SecretStore> {
    let base: Arc<dyn SecretStore> = if profile.id.is_empty() {
        profile.id = format!("unsaved-{}", uuid::Uuid::new_v4());
        Arc::new(MemoryStore::default())
    } else {
        keychain
    };
    Arc::new(OverlayStore {
        base,
        overrides: secrets.overrides(&profile.id),
    })
}

/// Проверяет и несохранённый профиль: секреты формы накладываются поверх keychain.
#[tauri::command]
pub async fn test_connection(
    state: State<'_, AppState>,
    input: ConnectionInput,
    channel: tauri::ipc::Channel<TestEvent>,
) -> AppResult<TestReport> {
    let ConnectionInput {
        mut profile,
        secrets,
    } = input;
    let overlay = test_secrets(state.secrets.clone(), &mut profile, &secrets);
    crate::readiness::check::run(profile, overlay, &|e| {
        let _ = channel.send(e);
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::conn::profile::{CertFormat, ClientCert, sample_profile};
    use crate::error::{AppError, ErrorCode};
    use crate::secrets::{MemoryStore, SecretStore};
    use crate::state::tests::Closed;
    use crate::storage::tree::{TreeItemKind, TreeItemRef};
    use crate::topology::BoxFuture;
    use crate::topology::model::{Role, Topology, member, topo};
    use crate::topology::runtime::{ConnState, Runtimes, TopologySource};

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
    fn debug_output_hides_secrets() {
        let secrets = SecretsInput {
            pg_password: Some("s3cr3t-pg".into()),
            pg_key_password: Some(String::new()),
            patroni_password: Some("s3cr3t-api".into()),
            patroni_key_password: None,
        };
        let input = ConnectionInput {
            profile: sample_profile("a"),
            secrets: secrets.clone(),
        };
        for shown in [
            format!("{secrets:?}"),
            format!("{input:?}"),
            format!("{input:#?}"),
        ] {
            assert!(!shown.contains("s3cr3t"), "{shown}");
            assert!(
                shown.contains("***") && shown.contains("delete") && shown.contains("keep"),
                "{shown}"
            );
        }
    }

    #[test]
    fn unsaved_profile_never_reads_saved_secrets() {
        let keychain: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
        // Настоящее подключение с id «unsaved» (раньше это был подставной id проверки) и его пароль.
        keychain
            .set(&secret_key("unsaved", PG_PASSWORD), "someone-else")
            .unwrap();
        let mut p = sample_profile("");
        p.id = String::new();
        let store = test_secrets(keychain.clone(), &mut p, &SecretsInput::default());
        assert!(p.id.starts_with("unsaved-") && p.id.len() > "unsaved-".len());
        assert_eq!(store.get(&secret_key(&p.id, PG_PASSWORD)).unwrap(), None);
        assert_eq!(
            store.get(&secret_key("unsaved", PG_PASSWORD)).unwrap(),
            None,
            "keychain не читается вовсе"
        );
        // Значение из формы используется.
        let mut q = sample_profile("");
        q.id = String::new();
        let typed = SecretsInput {
            pg_password: Some("typed".into()),
            ..Default::default()
        };
        let store = test_secrets(keychain.clone(), &mut q, &typed);
        assert_eq!(
            store
                .get(&secret_key(&q.id, PG_PASSWORD))
                .unwrap()
                .as_deref(),
            Some("typed")
        );
        assert_ne!(p.id, q.id, "id случайный");
        // Сохранённый профиль проверяется с паролем из keychain, если форма его не меняла.
        let mut saved = sample_profile("unsaved");
        let store = test_secrets(keychain, &mut saved, &SecretsInput::default());
        assert_eq!(
            store
                .get(&secret_key("unsaved", PG_PASSWORD))
                .unwrap()
                .as_deref(),
            Some("someone-else")
        );
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
    fn stale_secrets_cleanup_keeps_used_and_reset_removes_all() {
        let store = MemoryStore::default();
        let p = sample_profile("a");
        for f in CONNECTION_SECRET_FIELDS {
            store.set(&secret_key("a", f), "x").unwrap();
        }
        let left = |store: &MemoryStore| -> Vec<&str> {
            CONNECTION_SECRET_FIELDS
                .into_iter()
                .filter(|f| store.get(&secret_key("a", f)).unwrap().is_some())
                .collect()
        };
        // Сервер без Patroni и сертификата использует только пароль PG.
        delete_stale_secrets(&store, &p, false).unwrap();
        assert_eq!(left(&store), [PG_PASSWORD]);
        delete_stale_secrets(&store, &p, true).unwrap();
        assert!(left(&store).is_empty());
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
    fn saving_closes_the_open_connection() {
        let (_d, st) = state_with(Arc::new(MemoryStore::default()));
        let closed = Arc::new(crate::state::tests::Closed::default());
        st.runtimes.set_sink(closed.clone());
        save_connection_blocking(&st, input(sample_profile("a"), SecretsInput::default())).unwrap();
        assert_eq!(*closed.0.lock().unwrap(), vec!["a".to_string()]);
        // Неудачное сохранение подключение не трогает.
        let (_d, st) = state_with(Arc::new(BrokenKeychain));
        let closed = Arc::new(crate::state::tests::Closed::default());
        st.runtimes.set_sink(closed.clone());
        let secrets = SecretsInput {
            pg_password: Some("pw".into()),
            ..Default::default()
        };
        save_connection_blocking(&st, input(sample_profile("a"), secrets)).unwrap_err();
        assert!(closed.0.lock().unwrap().is_empty());
    }

    /// Источник с готовой топологией из двух узлов: открытое подключение не ходит в сеть.
    struct Fixed;

    impl TopologySource for Fixed {
        fn observe<'a>(
            &'a self,
            _p: &'a ConnectionProfile,
            _prev: Option<&'a Topology>,
        ) -> BoxFuture<'a, AppResult<Topology>> {
            Box::pin(async {
                Ok(topo(vec![
                    member("localhost:5432", Role::Primary),
                    member("b", Role::Replica),
                ]))
            })
        }
    }

    /// Состояние с сохранённым и открытым подключением `a`; узел каталога выбран вручную (`b`).
    async fn open_connection() -> (tempfile::TempDir, AppState, Arc<Closed>, ConnectionProfile) {
        let (dir, mut st) = state_with(Arc::new(MemoryStore::default()));
        st.runtimes =
            Runtimes::with_interval(Arc::new(Fixed), st.pools.clone(), Duration::from_secs(60));
        let saved = save_connection_blocking(
            &st,
            input(
                sample_profile("a"),
                SecretsInput {
                    pg_password: Some("pw".into()),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        st.runtimes.connect(saved.clone()).await.unwrap();
        st.runtimes
            .set_catalog_member("a", Some("b".into()))
            .unwrap();
        let closed = Arc::new(Closed::default());
        st.runtimes.set_sink(closed.clone());
        (dir, st, closed, saved)
    }

    fn still_open(st: &AppState) -> bool {
        let v = st.runtimes.view("a");
        v.state == ConnState::Connected && v.catalog_member.as_deref() == Some("b")
    }

    #[tokio::test]
    async fn saving_cosmetic_changes_keeps_the_open_connection() {
        let (_d, st, closed, saved) = open_connection().await;
        let mut p = saved.clone();
        p.name = "renamed".into();
        p.color = "#000000".into();
        p.environment = Environment::Test;
        p.read_only = true;
        // Поле секрета, которым профиль не пользуется, удаляется при каждом сохранении: не повод закрывать.
        let secrets = SecretsInput {
            patroni_password: Some("x".into()),
            ..Default::default()
        };
        save_connection_blocking(&st, input(p, secrets)).unwrap();
        assert!(closed.0.lock().unwrap().is_empty());
        assert!(still_open(&st), "узел каталога и состояние сохранились");
        assert_eq!(
            st.runtimes.target("a", None).unwrap().1.name,
            "renamed",
            "профиль в открытом подключении обновлён"
        );
        assert_eq!(
            st.secrets
                .get(&secret_key("a", PG_PASSWORD))
                .unwrap()
                .as_deref(),
            Some("pw")
        );
    }

    #[tokio::test]
    async fn moving_in_the_tree_does_not_reopen_the_connection() {
        // Перетаскивание меняет папку и порядок прямо в хранилище, подключение не закрывая;
        // следом раскрытие узла дерева снова вызывает `connect` с профилем из хранилища.
        let (_d, st, closed, _saved) = open_connection().await;
        let folder = st.connections.create_folder("F", None).unwrap();
        let item = TreeItemRef {
            kind: TreeItemKind::Connection,
            id: "a".into(),
        };
        st.connections
            .move_items(&[item], Some(&folder.id), None)
            .unwrap();
        let moved = st.profile("a").unwrap();
        assert_eq!(moved.folder_id.as_deref(), Some(folder.id.as_str()));

        st.runtimes.connect(moved.clone()).await.unwrap();
        assert!(closed.0.lock().unwrap().is_empty());
        assert!(still_open(&st), "узел каталога и состояние сохранились");
        // И сохранение формы после перетаскивания его не закрывает.
        save_connection_blocking(&st, input(moved, SecretsInput::default())).unwrap();
        assert!(closed.0.lock().unwrap().is_empty());
        assert!(still_open(&st));
    }

    #[tokio::test]
    async fn saving_new_connection_settings_closes_the_open_connection() {
        for edit in [
            (|p: &mut ConnectionProfile| p.hosts[0].host = "elsewhere".into())
                as fn(&mut ConnectionProfile),
            |p| p.pg.database = "orders".into(),
            |p| p.pg.username = "other".into(),
            |p| p.target_server_type = crate::conn::profile::TargetServerType::Standby,
        ] {
            let (_d, st, closed, saved) = open_connection().await;
            let mut p = saved;
            edit(&mut p);
            save_connection_blocking(&st, input(p, SecretsInput::default())).unwrap();
            assert_eq!(*closed.0.lock().unwrap(), vec!["a".to_string()]);
            assert_eq!(st.runtimes.view("a").state, ConnState::Disconnected);
        }
    }

    #[tokio::test]
    async fn saving_a_new_or_cleared_password_closes_the_open_connection() {
        for password in ["new-password", ""] {
            let (_d, st, closed, saved) = open_connection().await;
            let secrets = SecretsInput {
                pg_password: Some(password.into()),
                ..Default::default()
            };
            save_connection_blocking(&st, input(saved, secrets)).unwrap();
            assert_eq!(
                *closed.0.lock().unwrap(),
                vec!["a".to_string()],
                "пароль {password:?}"
            );
            assert_eq!(st.runtimes.view("a").state, ConnState::Disconnected);
        }
    }

    #[test]
    fn reconnect_decision() {
        let p = sample_profile("a");
        assert!(needs_reconnect(None, &p, false), "новый профиль");
        assert!(needs_reconnect(Some(&p), &p, true), "секрет изменён");
        assert!(!needs_reconnect(Some(&p), &p, false));
        let ops = secret_ops(&p, &SecretsInput::default());
        assert!(
            !secrets_changed(&p, &ops),
            "оставленный секрет и удаление неиспользуемых — не изменение"
        );
        let ops = secret_ops(
            &p,
            &SecretsInput {
                pg_key_password: Some("k".into()),
                ..Default::default()
            },
        );
        assert!(
            !secrets_changed(&p, &ops),
            "пароль ключа не используется профилем без сертификата"
        );
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
