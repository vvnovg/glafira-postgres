use std::path::Path;

use tauri::State;
use time::format_description::well_known::Rfc3339;

use super::blocking;
use super::connections::delete_stale_secrets;
use crate::error::{AppError, AppResult};
use crate::license::Tier;
use crate::state::AppState;
use crate::storage::connections::{ConnectionFolder, ConnectionProfile};
use crate::storage::exchange::{
    ExportSummary, ImportOptions, ImportPreview, build_export, existing_certificate_files,
    parse_exchange, plan_import, read_import_file,
};
use crate::storage::fs::write_json;
use crate::storage::tree::{TreeItemKind, TreeItemRef};

#[tauri::command]
pub fn list_folders(state: State<'_, AppState>) -> AppResult<Vec<ConnectionFolder>> {
    Ok(state.connections.list_folders())
}

#[tauri::command]
pub async fn create_folder(
    state: State<'_, AppState>,
    name: String,
    parent_id: Option<String>,
) -> AppResult<ConnectionFolder> {
    let st = state.inner().clone();
    blocking(move || st.connections.create_folder(&name, parent_id.as_deref())).await
}

#[tauri::command]
pub async fn rename_folder(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> AppResult<ConnectionFolder> {
    let st = state.inner().clone();
    blocking(move || st.connections.rename_folder(&id, &name)).await
}

#[tauri::command]
pub async fn delete_folder(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let st = state.inner().clone();
    blocking(move || st.connections.delete_folder(&id)).await
}

#[tauri::command]
pub async fn move_items(
    state: State<'_, AppState>,
    items: Vec<TreeItemRef>,
    parent_id: Option<String>,
    before: Option<TreeItemRef>,
) -> AppResult<()> {
    let st = state.inner().clone();
    blocking(move || {
        st.connections
            .move_items(&items, parent_id.as_deref(), before.as_ref())
    })
    .await
}

/// Пишет файл `glafira-postgres-connections` (всё дерево или папку `folder_id`) без секретов.
#[tauri::command]
pub async fn export_connections(
    state: State<'_, AppState>,
    path: String,
    folder_id: Option<String>,
) -> AppResult<ExportSummary> {
    let st = state.inner().clone();
    blocking(move || {
        let now = time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|e| AppError::internal_with(format!("exportedAt: {e}")))?;
        export_to_file(&st, Path::new(&path), folder_id.as_deref(), &now)
    })
    .await
}

fn export_to_file(
    st: &AppState,
    path: &Path,
    folder_id: Option<&str>,
    now: &str,
) -> AppResult<ExportSummary> {
    let out = build_export(
        &st.connections,
        st.secrets.as_ref(),
        folder_id,
        env!("CARGO_PKG_VERSION"),
        now,
    )?;
    write_json(path, &out)?;
    Ok(ExportSummary {
        connections: out.connections.len(),
        folders: out.folders.len(),
    })
}

/// Предпросмотр импорта файла `path` под папку `target_folder_id` (или в корень); ничего не меняет.
#[tauri::command]
pub async fn preview_import(
    state: State<'_, AppState>,
    path: String,
    target_folder_id: Option<String>,
    update_existing: bool,
) -> AppResult<ImportPreview> {
    let st = state.inner().clone();
    blocking(move || {
        import_from_file(
            &st,
            Path::new(&path),
            target_folder_id.as_deref(),
            update_existing,
            false,
        )
    })
    .await
}

/// Применяет импорт: план пересчитывается на свежих данных и пишется одной записью.
/// Секреты из файла не читаются и не пишутся: пароли новых подключений вводит пользователь,
/// а у обновлённых подключений с новым адресом или настройками безопасности сохранённые пароли удаляются.
#[tauri::command]
pub async fn apply_import(
    state: State<'_, AppState>,
    path: String,
    target_folder_id: Option<String>,
    update_existing: bool,
) -> AppResult<ImportPreview> {
    let st = state.inner().clone();
    blocking(move || {
        import_from_file(
            &st,
            Path::new(&path),
            target_folder_id.as_deref(),
            update_existing,
            true,
        )
    })
    .await
}

/// Обновлённое импортом подключение.
struct UpdatedConnection {
    before: ConnectionProfile,
    after: ConnectionProfile,
    /// Настройки соединения изменились: сохранённые пароли удаляются, открытое подключение закрывается.
    reset: bool,
}

/// Что импорт изменил в хранилище: по этому разбираются секреты и открытые подключения,
/// и по этому же импорт откатывается, если keychain не принял удаление.
struct Applied {
    updated: Vec<UpdatedConnection>,
    added_connections: Vec<String>,
    added_folders: Vec<String>,
}

/// Возвращает хранилище в состояние до импорта: убирает добавленное и восстанавливает профили
/// обновлённых подключений. Правки, сделанные в хранилище после импорта, остаются.
fn undo_import(st: &AppState, applied: &Applied) -> AppResult<()> {
    st.connections.update(|data| {
        data.connections
            .retain(|c| !applied.added_connections.contains(&c.id));
        data.folders
            .retain(|f| !applied.added_folders.contains(&f.id));
        for u in &applied.updated {
            if let Some(slot) = data.connections.iter_mut().find(|c| c.id == u.before.id) {
                *slot = u.before.clone();
            }
        }
        Ok(())
    })
}

/// Файл недоверенный и мог перенаправить подключение на другой сервер: сохранённые пароли
/// получателя не должны уйти по новому адресу, поэтому у подключений с изменившимися настройками
/// соединения удаляются все секреты, а ошибка keychain прерывает импорт. У остальных обновлённых
/// подключений удаляются только ненужные профилю секреты (как при сохранении формы); это уборка,
/// безопасности она не касается, поэтому ошибка keychain там только пишется в журнал.
fn delete_secrets(st: &AppState, applied: &Applied) -> AppResult<()> {
    for u in &applied.updated {
        match delete_stale_secrets(st.secrets.as_ref(), &u.after, u.reset) {
            Err(e) if u.reset => return Err(e),
            Err(e) => tracing::warn!("failed to delete unused secrets of {}: {e}", u.after.id),
            Ok(()) => {}
        }
    }
    Ok(())
}

fn import_from_file(
    st: &AppState,
    path: &Path,
    target: Option<&str>,
    update_existing: bool,
    apply: bool,
) -> AppResult<ImportPreview> {
    let parsed = parse_exchange(&read_import_file(path)?)?;
    let opts = ImportOptions {
        target,
        update_existing,
        // Те же правила Pro, что при сохранении формы: без лицензии prod-подключение
        // приходит только для чтения.
        is_pro: st.license.status().tier == Tier::Pro,
    };
    // Диск проверяется здесь, до блокировки хранилища: пути берутся из недоверенного файла, и
    // обращение к ним (особенно к сетевым) может быть долгим. Планировщик диск не трогает.
    let existing = existing_certificate_files(&parsed);
    let file_exists = |p: &str| existing.contains(p);
    if !apply {
        return Ok(plan_import(&st.connections.snapshot(), &parsed, &opts, &file_exists)?.preview);
    }
    let (preview, applied) = st.connections.update(|data| {
        let plan = plan_import(data, &parsed, &opts, &file_exists)?;
        // Профили обновлённых подключений: до и после.
        let updated = plan
            .preview
            .updated
            .iter()
            .filter_map(|item| {
                let before = data.connections.iter().find(|c| c.id == item.id)?;
                let after = plan.next.connections.iter().find(|c| c.id == item.id)?;
                Some(UpdatedConnection {
                    before: before.clone(),
                    after: after.clone(),
                    reset: plan.secrets_reset.contains(&item.id),
                })
            })
            .collect();
        let added = |kind: TreeItemKind| -> Vec<String> {
            plan.preview
                .added
                .iter()
                .filter(|item| item.kind == kind)
                .map(|item| item.id.clone())
                .collect()
        };
        let applied = Applied {
            updated,
            added_connections: added(TreeItemKind::Connection),
            added_folders: added(TreeItemKind::Folder),
        };
        *data = plan.next;
        Ok((plan.preview, applied))
    })?;
    // Как в `save_connection_blocking`: если keychain не принял удаление, импорт откатывается целиком,
    // а подключения остаются со старыми адресами.
    if let Err(e) = delete_secrets(st, &applied) {
        if let Err(r) = undo_import(st, &applied) {
            tracing::error!("failed to roll back the import: {r}");
        }
        return Err(e);
    }
    // Как при сохранении формы: открытое подключение переоткрывается, только если изменились
    // настройки соединения; правка оформления обновляет профиль в нём.
    for u in &applied.updated {
        if u.reset {
            st.runtimes.disconnect(&u.after.id);
        } else {
            st.runtimes.refresh_profile(&u.after);
        }
    }
    Ok(preview)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::conn::profile::{
        ApiTls, CertFormat, ClientCert, ConnectionKind, PatroniSettings, SslMode, sample_profile,
    };
    use crate::secrets::{
        CONNECTION_SECRET_FIELDS, MemoryStore, PATRONI_KEY_PASSWORD, PATRONI_PASSWORD,
        PG_KEY_PASSWORD, PG_PASSWORD, SecretStore, secret_key,
    };
    use crate::state::tests::Closed;

    /// Состояние с бессрочной Pro-лицензией (для проверки импорта без ограничений Free).
    fn pro_state(dir: &Path) -> AppState {
        let mut st = AppState::new(dir, Arc::new(MemoryStore::default())).unwrap();
        st.license = Arc::new(crate::license::pro_for_tests(dir));
        st
    }

    #[test]
    fn export_writes_file_without_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemoryStore::default());
        secrets
            .set(&secret_key("a", crate::secrets::PG_PASSWORD), "s3cr3t")
            .unwrap();
        let st = AppState::new(dir.path(), secrets).unwrap();
        let f = st.connections.create_folder("ИФТ", None).unwrap();
        let mut p = sample_profile("a");
        p.folder_id = Some(f.id.clone());
        st.connections.upsert(p).unwrap();
        st.connections.upsert(sample_profile("b")).unwrap();

        let path = dir.path().join("out.glafira-postgres-connections.json");
        let sum = export_to_file(&st, &path, Some(&f.id), "2026-10-17T12:00:00Z").unwrap();
        assert_eq!(
            sum,
            ExportSummary {
                connections: 1,
                folders: 1
            }
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("s3cr3t"));
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["format"], "glafira-postgres-connections");
        assert_eq!(v["appVersion"], env!("CARGO_PKG_VERSION"));
        assert_eq!(v["connections"][0]["id"], "a");
        assert_eq!(v["connections"][0]["needsPassword"]["pg"], true);

        let err = export_to_file(&st, &path, Some("nope"), "now").unwrap_err();
        assert_eq!(err.key, "errors.folders.notFound");
    }

    fn write_file(dir: &Path, changed_host: &str) -> std::path::PathBuf {
        let mut p = serde_json::to_value(sample_profile("a")).unwrap();
        p["hosts"] = serde_json::json!([{"host": changed_host, "port": 5432}]);
        let mut n = serde_json::to_value(sample_profile("n")).unwrap();
        n["environment"] = "prod".into();
        n["needsPassword"] = serde_json::json!({"pg": true});
        let path = dir.join("in.glafira-postgres-connections.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "format": "glafira-postgres-connections", "version": 1,
                "folders": [], "connections": [p, n],
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    #[test]
    fn preview_does_not_write_and_apply_writes_once() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemoryStore::default());
        let st = AppState::new(dir.path(), secrets.clone()).unwrap();
        st.connections.upsert(sample_profile("a")).unwrap();
        secrets
            .set(&secret_key("a", crate::secrets::PG_PASSWORD), "local-pw")
            .unwrap();
        let input = tempfile::tempdir().unwrap();
        let path = write_file(input.path(), "changed");
        let conns = dir.path().join("connections.json");
        let before = std::fs::read(&conns).unwrap();

        let preview = import_from_file(&st, &path, None, true, false).unwrap();
        assert_eq!(preview.updated.len(), 1);
        assert_eq!(preview.added.len(), 1);
        // Новое подключение с паролем у отправителя и обновлённое с новым адресом.
        assert_eq!(preview.needs_passwords, 2);
        // Free: prod-подключение станет только для чтения; у `a` меняется адрес.
        let keys: Vec<_> = preview.warnings.iter().map(|w| w.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "warnings.import.prodReadOnly",
                "warnings.import.passwordsReset"
            ]
        );
        assert_eq!(std::fs::read(&conns).unwrap(), before);
        // Предпросмотр секреты не трогает.
        assert_eq!(
            secrets
                .get(&secret_key("a", PG_PASSWORD))
                .unwrap()
                .as_deref(),
            Some("local-pw")
        );

        let applied = import_from_file(&st, &path, None, true, true).unwrap();
        assert_eq!(applied, preview);
        assert_eq!(st.connections.get("a").unwrap().hosts[0].host, "changed");
        assert!(st.connections.get("n").unwrap().read_only);
        let reopened = crate::storage::connections::ConnectionStore::open(dir.path()).unwrap();
        assert_eq!(reopened.list().len(), 2);
        // Файл поменял адрес `a`: пароль получателя не должен уйти на новый адрес. Для нового
        // подключения секреты из файла не записываются никогда.
        assert_eq!(secrets.get(&secret_key("a", PG_PASSWORD)).unwrap(), None);
        assert_eq!(secrets.get(&secret_key("n", PG_PASSWORD)).unwrap(), None);
    }

    #[test]
    fn pro_keeps_prod_writable_on_import() {
        let dir = tempfile::tempdir().unwrap();
        let st = pro_state(dir.path());
        let input = tempfile::tempdir().unwrap();
        let path = write_file(input.path(), "changed");
        let preview = import_from_file(&st, &path, None, true, true).unwrap();
        assert!(preview.warnings.is_empty());
        assert!(!st.connections.get("n").unwrap().read_only);
    }

    #[test]
    fn free_cannot_lift_read_only_of_imported_prod_profile() {
        // Во Free импорт кладёт prod только для чтения, а сохранение формы снять защиту не даёт.
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        let input = tempfile::tempdir().unwrap();
        let path = write_file(input.path(), "changed");
        import_from_file(&st, &path, None, true, true).unwrap();
        let mut imported = st.connections.get("n").unwrap();
        assert!(imported.read_only);
        imported.read_only = false;
        let err = crate::commands::connections::save_connection_blocking(
            &st,
            crate::commands::connections::ConnectionInput {
                profile: imported,
                secrets: Default::default(),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, crate::error::ErrorCode::ProRequired);
        assert!(st.connections.get("n").unwrap().read_only);
    }

    #[test]
    fn update_closes_the_open_connection_only_when_settings_change() {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        st.connections.upsert(sample_profile("a")).unwrap();
        let input = tempfile::tempdir().unwrap();

        // Меняется только имя: подключение остаётся.
        let closed = Arc::new(Closed::default());
        st.runtimes.set_sink(closed.clone());
        let mut renamed = serde_json::to_value(sample_profile("a")).unwrap();
        renamed["name"] = "renamed".into();
        let path = input.path().join("rename.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "format": "glafira-postgres-connections", "version": 1,
                "folders": [], "connections": [renamed],
            }))
            .unwrap(),
        )
        .unwrap();
        let preview = import_from_file(&st, &path, None, true, true).unwrap();
        assert_eq!(preview.updated.len(), 1);
        assert_eq!(st.connections.get("a").unwrap().name, "renamed");
        assert!(closed.0.lock().unwrap().is_empty());

        // Меняется адрес: открытое подключение закрывается.
        let path = write_file(input.path(), "changed");
        import_from_file(&st, &path, None, true, true).unwrap();
        assert_eq!(*closed.0.lock().unwrap(), vec!["a".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn failed_apply_changes_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let st = AppState::new(&data, Arc::new(MemoryStore::default())).unwrap();
        st.connections.upsert(sample_profile("a")).unwrap();
        let path = write_file(dir.path(), "changed");
        let conns = data.join("connections.json");
        let before = std::fs::read(&conns).unwrap();

        // Каталог данных только для чтения: временный файл не создать, запись падает.
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Root (и процесс с CAP_DAC_OVERRIDE) пишет в каталог без прав: сбоя записи не получить,
        // и тест ничего не проверял бы. Определяем это пробной записью — это надёжнее, чем смотреть uid.
        if std::fs::File::create(data.join(".probe")).is_ok() {
            std::fs::remove_file(data.join(".probe")).unwrap();
            std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o755)).unwrap();
            eprintln!("пропущено: процесс игнорирует права каталога (root)");
            return;
        }
        let result = import_from_file(&st, &path, None, true, true);
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err());
        assert_eq!(std::fs::read(&conns).unwrap(), before);
        assert_eq!(st.connections.list().len(), 1);
        assert_eq!(st.connections.get("a").unwrap().hosts[0].host, "localhost");
    }

    // ---- импорт недоверенного файла и сохранённые пароли ----

    /// Все четыре секрета подключения в keychain.
    fn set_all_secrets(secrets: &dyn SecretStore, id: &str) {
        for f in CONNECTION_SECRET_FIELDS {
            secrets
                .set(&secret_key(id, f), &format!("{id}-{f}"))
                .unwrap();
        }
    }

    fn secrets_left(secrets: &dyn SecretStore, id: &str) -> Vec<&'static str> {
        CONNECTION_SECRET_FIELDS
            .into_iter()
            .filter(|f| secrets.get(&secret_key(id, f)).unwrap().is_some())
            .collect()
    }

    /// Подключение Patroni, которому нужны все четыре секрета (пароль и клиентский сертификат у PG и у API).
    fn patroni_with_all_secrets(id: &str, dir: &Path) -> ConnectionProfile {
        let cert = |name: &str| ClientCert {
            format: CertFormat::Pkcs12,
            certificate_path: dir.join(name).to_string_lossy().into_owned(),
            key_path: None,
        };
        let mut p = sample_profile(id);
        p.kind = ConnectionKind::Patroni;
        p.hosts.clear();
        p.pg.tls.client_cert = Some(cert("pg.p12"));
        p.patroni = Some(PatroniSettings {
            urls: vec!["https://pg-1:8008".into()],
            username: Some("patroni".into()),
            tls: Some(ApiTls {
                ca_path: None,
                verify_hostname: true,
                client_cert: Some(cert("api.p12")),
            }),
            host_overrides: vec![],
        });
        for f in CONNECTION_SECRET_FIELDS {
            assert!(p.uses_secret(f), "предпосылка: профиль использует {f}");
        }
        p
    }

    /// Файл обмена с подключениями `conns` (JSON) и папками `folders`.
    fn write_exchange(
        dir: &Path,
        folders: Vec<serde_json::Value>,
        conns: Vec<serde_json::Value>,
    ) -> std::path::PathBuf {
        let path = dir.join("exchange.glafira-postgres-connections.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "format": "glafira-postgres-connections", "version": 1,
                "folders": folders, "connections": conns,
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    fn json_of(p: &ConnectionProfile) -> serde_json::Value {
        serde_json::to_value(p).unwrap()
    }

    #[test]
    fn imported_address_change_deletes_saved_secrets_and_asks_for_passwords() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemoryStore::default());
        let st = AppState::new(dir.path(), secrets.clone()).unwrap();
        let certs = tempfile::tempdir().unwrap();
        let server = sample_profile("host");
        let patroni = patroni_with_all_secrets("url", certs.path());
        let mut downgrade = sample_profile("ssl");
        downgrade.pg.ssl_mode = SslMode::VerifyFull;
        let untouched = sample_profile("keep");
        for p in [&server, &patroni, &downgrade, &untouched] {
            st.connections.upsert(p.clone()).unwrap();
            set_all_secrets(secrets.as_ref(), &p.id);
        }
        // Файл отправляет те же подключения, но с другим адресом сервера, адресом Patroni API
        // и пониженной защитой TLS; «keep» в файле нет.
        let mut moved = json_of(&server);
        moved["hosts"] = serde_json::json!([{"host": "evil.example", "port": 5432}]);
        let mut api_moved = json_of(&patroni);
        api_moved["patroni"]["urls"] = serde_json::json!(["https://evil.example:8008"]);
        let mut weaker = json_of(&downgrade);
        weaker["pg"]["sslMode"] = "prefer".into();
        let path = write_exchange(certs.path(), vec![], vec![moved, api_moved, weaker]);
        let closed = Arc::new(Closed::default());
        st.runtimes.set_sink(closed.clone());

        let preview = import_from_file(&st, &path, None, true, false).unwrap();
        assert_eq!(preview.updated.len(), 3);
        assert_eq!(preview.needs_passwords, 3);
        let reset = preview
            .warnings
            .iter()
            .find(|w| w.key == "warnings.import.passwordsReset")
            .expect("предупреждение о сбросе паролей");
        assert_eq!(reset.params["count"], "3");
        // Предпросмотр ничего не удаляет.
        for id in ["host", "url", "ssl", "keep"] {
            assert_eq!(secrets_left(secrets.as_ref(), id).len(), 4, "{id}");
        }

        let applied = import_from_file(&st, &path, None, true, true).unwrap();
        assert_eq!(applied, preview);
        assert_eq!(
            st.connections.get("host").unwrap().hosts[0].host,
            "evil.example"
        );
        assert_eq!(
            st.connections.get("url").unwrap().patroni.unwrap().urls,
            ["https://evil.example:8008"]
        );
        assert_eq!(
            st.connections.get("ssl").unwrap().pg.ssl_mode,
            SslMode::Prefer
        );
        // Пароли перенаправленных подключений удалены целиком, чужие подключения не тронуты.
        for id in ["host", "url", "ssl"] {
            assert!(secrets_left(secrets.as_ref(), id).is_empty(), "{id}");
        }
        assert_eq!(secrets_left(secrets.as_ref(), "keep").len(), 4);
        // Следующий экспорт честно отмечает, что паролей у этих подключений нет.
        let out = dir.path().join("out.json");
        export_to_file(&st, &out, None, "now").unwrap();
        let exported: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        for c in exported["connections"].as_array().unwrap() {
            let flags = &c["needsPassword"];
            let any = flags.as_object().unwrap().values().any(|v| v == true);
            assert_eq!(any, c["id"] == "keep", "{c}");
        }
        // Открытые подключения закрыты.
        let mut closed_ids = closed.0.lock().unwrap().clone();
        closed_ids.sort();
        assert_eq!(closed_ids, ["host", "ssl", "url"]);
    }

    /// Пароли удаляются раньше, чем закрывается открытое подключение: переподключение не должно
    /// успеть взять старый пароль для нового адреса.
    #[test]
    fn secrets_are_deleted_before_the_open_connection_is_closed() {
        use crate::topology::runtime::{ConnState, RuntimeSink, TopologyView};

        struct SecretsAtClose {
            secrets: Arc<MemoryStore>,
            seen: std::sync::Mutex<Vec<Option<String>>>,
        }
        impl RuntimeSink for SecretsAtClose {
            fn state_changed(&self, id: &str, state: ConnState, _: Option<&AppError>) {
                if state == ConnState::Disconnected {
                    let left = self.secrets.get(&secret_key(id, PG_PASSWORD)).unwrap();
                    self.seen.lock().unwrap().push(left);
                }
            }
            fn topology_changed(
                &self,
                _: &str,
                _: &TopologyView,
                _: &[crate::topology::journal::TopologyEvent],
            ) {
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemoryStore::default());
        let st = AppState::new(dir.path(), secrets.clone()).unwrap();
        st.connections.upsert(sample_profile("a")).unwrap();
        secrets.set(&secret_key("a", PG_PASSWORD), "pw").unwrap();
        let sink = Arc::new(SecretsAtClose {
            secrets: secrets.clone(),
            seen: Default::default(),
        });
        st.runtimes.set_sink(sink.clone());
        let input = tempfile::tempdir().unwrap();
        import_from_file(&st, &write_file(input.path(), "changed"), None, true, true).unwrap();
        assert_eq!(*sink.seen.lock().unwrap(), vec![None]);
    }

    #[test]
    fn imported_cosmetic_changes_keep_saved_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemoryStore::default());
        let st = AppState::new(dir.path(), secrets.clone()).unwrap();
        let certs = tempfile::tempdir().unwrap();
        let server = sample_profile("a");
        let patroni = patroni_with_all_secrets("p", certs.path());
        for p in [&server, &patroni] {
            st.connections.upsert(p.clone()).unwrap();
            set_all_secrets(secrets.as_ref(), &p.id);
        }
        let mut renamed = json_of(&server);
        renamed["name"] = "renamed".into();
        renamed["color"] = "#ff0000".into();
        renamed["environment"] = "stage".into();
        let mut recoloured = json_of(&patroni);
        recoloured["color"] = "#00ff00".into();
        recoloured["readOnly"] = true.into();
        let path = write_exchange(certs.path(), vec![], vec![renamed, recoloured]);
        let closed = Arc::new(Closed::default());
        st.runtimes.set_sink(closed.clone());

        let preview = import_from_file(&st, &path, None, true, true).unwrap();
        assert_eq!(preview.updated.len(), 2);
        assert_eq!(preview.needs_passwords, 0);
        assert!(
            preview
                .warnings
                .iter()
                .all(|w| w.key != "warnings.import.passwordsReset"),
            "{:?}",
            preview.warnings
        );
        assert_eq!(st.connections.get("a").unwrap().name, "renamed");
        // Используемые секреты на месте; неиспользуемые профилем (у сервера — всё, кроме пароля PG) убраны.
        assert_eq!(secrets_left(secrets.as_ref(), "p").len(), 4);
        assert_eq!(secrets_left(secrets.as_ref(), "a"), [PG_PASSWORD]);
        // Подключение при правке оформления не закрывается.
        assert!(closed.0.lock().unwrap().is_empty());
    }

    #[test]
    fn update_deletes_secrets_the_profile_no_longer_uses() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemoryStore::default());
        let st = AppState::new(dir.path(), secrets.clone()).unwrap();
        st.connections.upsert(sample_profile("a")).unwrap();
        // Устаревшие записи: сервер без Patroni и клиентского сертификата их не использует.
        for f in [
            PG_PASSWORD,
            PG_KEY_PASSWORD,
            PATRONI_PASSWORD,
            PATRONI_KEY_PASSWORD,
        ] {
            secrets.set(&secret_key("a", f), "x").unwrap();
        }
        let mut renamed = json_of(&sample_profile("a"));
        renamed["name"] = "renamed".into();
        let input = tempfile::tempdir().unwrap();
        let path = write_exchange(input.path(), vec![], vec![renamed]);
        import_from_file(&st, &path, None, true, true).unwrap();
        assert_eq!(secrets_left(secrets.as_ref(), "a"), [PG_PASSWORD]);

        // Адрес сменился и заодно пропал клиентский сертификат: удаляются все секреты.
        let certs = tempfile::tempdir().unwrap();
        let p = patroni_with_all_secrets("b", certs.path());
        st.connections.upsert(p.clone()).unwrap();
        set_all_secrets(secrets.as_ref(), "b");
        let mut without_cert = json_of(&p);
        without_cert["pg"]["tls"] = serde_json::json!({});
        without_cert["patroni"]["tls"] = serde_json::Value::Null;
        let path = write_exchange(input.path(), vec![], vec![without_cert]);
        import_from_file(&st, &path, None, true, true).unwrap();
        assert!(secrets_left(secrets.as_ref(), "b").is_empty());
    }

    /// Keychain, в который можно писать и из которого можно читать, но нельзя удалять.
    #[derive(Default)]
    struct NoDelete(MemoryStore);

    impl SecretStore for NoDelete {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            self.0.get(key)
        }
        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            self.0.set(key, value)
        }
        fn delete(&self, _: &str) -> AppResult<()> {
            Err(AppError::internal_with("keychain unavailable"))
        }
    }

    #[test]
    fn keychain_delete_failure_rolls_the_import_back() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(NoDelete::default());
        let st = AppState::new(dir.path(), secrets.clone()).unwrap();
        let folder = st.connections.create_folder("Local", None).unwrap();
        let mut local = sample_profile("a");
        local.folder_id = Some(folder.id.clone());
        st.connections.upsert(local).unwrap();
        st.connections.upsert(sample_profile("b")).unwrap();
        // Не все order нулевые: иначе при повторном открытии файла сработает миграция старых файлов.
        st.connections.upsert(sample_profile("c")).unwrap();
        secrets.set(&secret_key("a", PG_PASSWORD), "pw").unwrap();
        let before_profiles = st.connections.list();
        let before_folders = st.connections.list_folders();
        let closed = Arc::new(Closed::default());
        st.runtimes.set_sink(closed.clone());

        // Файл: другой адрес у `a`, новая папка, новое подключение в ней и переименование `b`.
        let mut moved = json_of(&sample_profile("a"));
        moved["hosts"] = serde_json::json!([{"host": "evil.example", "port": 5432}]);
        let mut renamed = json_of(&sample_profile("b"));
        renamed["name"] = "renamed".into();
        let mut fresh = json_of(&sample_profile("n"));
        fresh["folderId"] = "nf".into();
        let input = tempfile::tempdir().unwrap();
        let path = write_exchange(
            input.path(),
            vec![serde_json::json!({"id": "nf", "name": "New", "parentId": null, "order": 0})],
            vec![moved, renamed, fresh],
        );

        let err = import_from_file(&st, &path, None, true, true).unwrap_err();
        assert_eq!(err.code, crate::error::ErrorCode::Internal);
        // Профили, папки и файл на диске — как до импорта; пароль на месте и снова привязан к старому адресу.
        assert_eq!(st.connections.list(), before_profiles);
        assert_eq!(st.connections.list_folders(), before_folders);
        assert_eq!(st.connections.get("a").unwrap().hosts[0].host, "localhost");
        assert!(st.connections.get("n").is_err());
        let reopened = crate::storage::connections::ConnectionStore::open(dir.path()).unwrap();
        assert_eq!(reopened.list(), before_profiles);
        assert_eq!(reopened.list_folders(), before_folders);
        assert_eq!(
            secrets
                .get(&secret_key("a", PG_PASSWORD))
                .unwrap()
                .as_deref(),
            Some("pw")
        );
        // Неудачный импорт открытые подключения не трогает.
        assert!(closed.0.lock().unwrap().is_empty());
    }

    #[test]
    fn cosmetic_update_does_not_depend_on_keychain_cleanup() {
        // Убрать ненужные секреты не удалось — на безопасность это не влияет, импорт проходит.
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(NoDelete::default());
        let st = AppState::new(dir.path(), secrets.clone()).unwrap();
        st.connections.upsert(sample_profile("a")).unwrap();
        secrets.set(&secret_key("a", PG_PASSWORD), "pw").unwrap();
        let mut renamed = json_of(&sample_profile("a"));
        renamed["name"] = "renamed".into();
        let path = write_exchange(dir.path(), vec![], vec![renamed]);
        let preview = import_from_file(&st, &path, None, true, true).unwrap();
        assert_eq!(preview.updated.len(), 1);
        assert_eq!(st.connections.get("a").unwrap().name, "renamed");
        assert_eq!(
            secrets
                .get(&secret_key("a", PG_PASSWORD))
                .unwrap()
                .as_deref(),
            Some("pw")
        );
    }

    #[test]
    fn rollback_keeps_edits_made_after_the_import_started() {
        // Откат убирает только то, что добавил импорт, и не затирает чужие правки хранилища.
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        st.connections.upsert(sample_profile("a")).unwrap();
        let before = st.connections.get("a").unwrap();
        let applied = Applied {
            updated: vec![UpdatedConnection {
                before: before.clone(),
                after: before,
                reset: true,
            }],
            added_connections: vec!["n".into()],
            added_folders: vec![],
        };
        st.connections.upsert(sample_profile("n")).unwrap();
        st.connections.upsert(sample_profile("other")).unwrap();
        undo_import(&st, &applied).unwrap();
        let ids: Vec<_> = st.connections.list().into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["a", "other"]);
    }

    #[test]
    fn oversized_import_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        let path = dir.path().join("big.json");
        // Разреженный файл на один байт больше предела: на диске почти ничего не занимает.
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(crate::storage::exchange::MAX_IMPORT_BYTES + 1)
            .unwrap();
        for apply in [false, true] {
            let err = import_from_file(&st, &path, None, true, apply).unwrap_err();
            assert_eq!(err.code, crate::error::ErrorCode::InvalidInput);
            assert_eq!(err.key, "errors.import.tooLarge");
            assert_eq!(err.params["limit_mb"], "10");
        }
    }

    /// Сетевые пути и пути устройств к сертификатам (PG и Patroni API) из файла не импортируются: иначе
    /// раскрытие подключения в дереве (даже без пароля) открыло бы SMB-соединение к адресу из файла.
    #[test]
    fn network_certificate_paths_are_not_imported() {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        st.connections.upsert(sample_profile("a")).unwrap();
        // Абсолютные пути и на Unix, и на Windows; сервера по этим адресам нет.
        let mut moved = json_of(&sample_profile("a"));
        moved["pg"]["tls"] = serde_json::json!({"caPath": "//glafira-test.invalid/share/ca.pem"});
        let mut fresh = json_of(&patroni_with_all_secrets("p", dir.path()));
        fresh["patroni"]["tls"]["caPath"] = r"\\?\UNC\glafira-test.invalid\share\ca.pem".into();
        let path = write_exchange(dir.path(), vec![], vec![moved, fresh]);
        for apply in [false, true] {
            let preview = import_from_file(&st, &path, None, true, apply).unwrap();
            assert!(
                preview.added.is_empty() && preview.updated.is_empty(),
                "{preview:?}"
            );
            let reasons: Vec<_> = preview
                .skipped
                .iter()
                .map(|s| s.reason.key.as_str())
                .collect();
            assert_eq!(
                reasons,
                [
                    "errors.connections.pathNetwork",
                    "errors.connections.pathNetwork"
                ]
            );
        }
        assert_eq!(st.connections.get("a").unwrap().pg.tls.ca_path, None);
        assert!(st.connections.get("p").is_err());
    }

    #[test]
    fn foreign_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        let path = dir.path().join("x.json");
        std::fs::write(&path, br#"{"format":"other","version":1}"#).unwrap();
        let err = import_from_file(&st, &path, None, true, false).unwrap_err();
        assert_eq!(err.key, "errors.import.notGlafiraFile");
        // Файл Glafira for Kafka — тоже чужой.
        std::fs::write(
            &path,
            br#"{"format":"glafira-connections","version":1,"folders":[],"connections":[]}"#,
        )
        .unwrap();
        let err = import_from_file(&st, &path, None, true, false).unwrap_err();
        assert_eq!(err.key, "errors.import.notGlafiraFile");
    }
}
