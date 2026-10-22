use std::collections::HashSet;
use std::path::Path;

use tauri::State;
use time::format_description::well_known::Rfc3339;

use super::blocking;
use super::connections::delete_stale_secrets;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::AppState;
use crate::storage::connections::{ConnectionFolder, ConnectionProfile};
use crate::storage::exchange::{
    ExportSummary, ImportOptions, ImportPlan, ImportPreview, build_export,
    existing_certificate_files, parse_exchange, plan_import, read_import_file,
};
use crate::storage::fs::write_json;
use crate::storage::tree::{TreeItemKind, TreeItemRef};

/// Как `list_connections`: блокировку хранилища ждёт фоновый поток, а не главный.
#[tauri::command]
pub async fn list_folders(state: State<'_, AppState>) -> AppResult<Vec<ConnectionFolder>> {
    let st = state.inner().clone();
    blocking(move || Ok(st.connections.list_folders())).await
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
/// а у обновлённых подключений с новым адресом или настройками безопасности сохранённые пароли удаляются
/// (до записи хранилища — см. `import_from_file`).
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

/// Сколько раз импорт пересчитывает план, если хранилище изменилось, пока удалялись секреты.
const IMPORT_ATTEMPTS: usize = 3;

/// Подключения, чьи секреты удаляются до записи импорта: обновляемые с другими настройками соединения
/// (`ImportPlan::secrets_reset`) и новые. У новых секретов быть не должно, но запись могла остаться под тем же
/// id (удалённое старой версией подключение): файл недоверенный, и она не должна уйти на адрес из файла.
fn ids_to_clear(plan: &ImportPlan) -> Vec<String> {
    let added = plan
        .preview
        .added
        .iter()
        .filter(|item| item.kind == TreeItemKind::Connection)
        .map(|item| item.id.clone());
    plan.secrets_reset.iter().cloned().chain(added).collect()
}

/// Импорт записан, но пароли части подключений уже удалены: пользователю остаётся ввести их заново.
fn not_saved(error: AppError, cleared: usize) -> AppError {
    tracing::warn!(
        "import not saved after the secrets of {cleared} connections were removed: {}",
        error.message
    );
    app_err!(
        ErrorCode::Internal,
        "errors.import.notSaved",
        "The import was not saved. Saved passwords of {{count}} connections it would change have already been removed — enter them again",
        count = cleared
    )
    .with_details(error.details.clone().unwrap_or(error.message))
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
    };
    // Диск проверяется здесь, до блокировки хранилища: пути берутся из недоверенного файла, и
    // обращение к ним (особенно к сетевым) может быть долгим. Планировщик диск не трогает.
    let existing = existing_certificate_files(&parsed);
    let file_exists = |p: &str| existing.contains(p);
    if !apply {
        return Ok(plan_import(&st.connections.snapshot(), &parsed, &opts, &file_exists)?.preview);
    }
    // Файл недоверенный и мог перенаправить подключение на другой сервер. Секреты удаляются раньше, чем
    // в хранилище появится новый адрес: ни подключение в промежутке, ни падение процесса, ни сбой записи
    // не свяжут сохранённый пароль с адресом из файла. Неудача в обратную сторону безопасна: пароли уже
    // удалены, профиль ещё прежний — пользователь вводит пароли заново. Keychain — вне блокировки хранилища.
    let mut cleared: HashSet<String> = HashSet::new();
    for _ in 0..IMPORT_ATTEMPTS {
        let plan = plan_import(&st.connections.snapshot(), &parsed, &opts, &file_exists)?;
        for id in ids_to_clear(&plan) {
            if cleared.contains(&id) {
                continue;
            }
            let profile = plan
                .next
                .connections
                .iter()
                .find(|c| c.id == id)
                .expect("planned connection exists");
            // Ошибка keychain прерывает импорт до записи: хранилище не изменилось.
            delete_stale_secrets(st.secrets.as_ref(), profile, true)?;
            cleared.insert(id);
        }
        // План пересчитывается на свежих данных под блокировкой записи. Если хранилище успело измениться и
        // секреты нужно удалить ещё у кого-то, запись откладывается: удаление — снова вне блокировки.
        let written = st.connections.update_if(|data| {
            let plan = plan_import(data, &parsed, &opts, &file_exists)?;
            if !ids_to_clear(&plan).iter().all(|id| cleared.contains(id)) {
                return Ok(None);
            }
            let updated: Vec<(ConnectionProfile, bool)> = plan
                .preview
                .updated
                .iter()
                .filter_map(|item| {
                    let after = plan.next.connections.iter().find(|c| c.id == item.id)?;
                    Some((after.clone(), plan.secrets_reset.contains(&item.id)))
                })
                .collect();
            *data = plan.next;
            Ok(Some((plan.preview, updated)))
        });
        let (preview, updated) = match written {
            Ok(Some(done)) => done,
            Ok(None) => continue,
            Err(e) if cleared.is_empty() => return Err(e),
            Err(e) => return Err(not_saved(e, cleared.len())),
        };
        for (after, reset) in &updated {
            if *reset {
                // Как при сохранении формы: открытое подключение переоткроется уже без паролей.
                st.runtimes.disconnect(&after.id);
            } else {
                // Уборка, как при сохранении формы: ненужные профилю секреты. Безопасности она не касается,
                // поэтому ошибка keychain только пишется в журнал.
                if let Err(e) = delete_stale_secrets(st.secrets.as_ref(), after, false) {
                    tracing::warn!("failed to delete unused secrets of {}: {e}", after.id);
                }
                // Правка оформления обновляет профиль в открытом подключении.
                st.runtimes.refresh_profile(after);
            }
        }
        return Ok(preview);
    }
    Err(app_err!(
        ErrorCode::Internal,
        "errors.import.storeChanged",
        "Connections changed while the import was being applied: try again"
    ))
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
        // У `a` меняется адрес: пароль получателя будет удалён.
        let keys: Vec<_> = preview.warnings.iter().map(|w| w.key.as_str()).collect();
        assert_eq!(keys, ["warnings.import.passwordsReset"]);
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
        assert!(
            !st.connections.get("n").unwrap().read_only,
            "readOnly как в файле"
        );
        let reopened = crate::storage::connections::ConnectionStore::open(dir.path()).unwrap();
        assert_eq!(reopened.list().len(), 2);
        // Файл поменял адрес `a`: пароль получателя не должен уйти на новый адрес. Для нового
        // подключения секреты из файла не записываются никогда.
        assert_eq!(secrets.get(&secret_key("a", PG_PASSWORD)).unwrap(), None);
        assert_eq!(secrets.get(&secret_key("n", PG_PASSWORD)).unwrap(), None);
    }

    #[test]
    fn imported_prod_profile_keeps_read_only_from_file_and_can_change_it() {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        let input = tempfile::tempdir().unwrap();
        let path = write_file(input.path(), "changed");
        let preview = import_from_file(&st, &path, None, true, true).unwrap();
        assert!(
            preview
                .warnings
                .iter()
                .all(|w| w.key != "warnings.import.prodReadOnly")
        );
        let mut imported = st.connections.get("n").unwrap();
        assert!(!imported.read_only, "readOnly как в файле");
        // Флажок можно поставить и снять: тарифов нет.
        imported.read_only = true;
        crate::commands::connections::save_connection_blocking(
            &st,
            crate::commands::connections::ConnectionInput {
                profile: imported,
                secrets: Default::default(),
            },
        )
        .unwrap();
        assert!(st.connections.get("n").unwrap().read_only);
        let mut back = st.connections.get("n").unwrap();
        back.read_only = false;
        crate::commands::connections::save_connection_blocking(
            &st,
            crate::commands::connections::ConnectionInput {
                profile: back,
                secrets: Default::default(),
            },
        )
        .unwrap();
        assert!(!st.connections.get("n").unwrap().read_only);
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
    fn failed_store_write_keeps_profiles_and_reports_removed_passwords() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let secrets = Arc::new(MemoryStore::default());
        let st = AppState::new(&data, secrets.clone()).unwrap();
        st.connections.upsert(sample_profile("a")).unwrap();
        secrets.set(&secret_key("a", PG_PASSWORD), "pw").unwrap();
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
        let err = result.unwrap_err();
        // Профили прежние; пароли подключения, которое файл перенаправлял, уже удалены — безопасная сторона:
        // пользователю остаётся ввести их заново, и ошибка говорит об этом.
        assert_eq!(err.key, "errors.import.notSaved");
        assert_eq!(err.params["count"], "2", "`a` с новым адресом и новое `n`");
        assert!(err.details.is_some());
        assert_eq!(std::fs::read(&conns).unwrap(), before);
        assert_eq!(st.connections.list().len(), 1);
        assert_eq!(st.connections.get("a").unwrap().hosts[0].host, "localhost");
        assert_eq!(secrets.get(&secret_key("a", PG_PASSWORD)).unwrap(), None);
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
    fn keychain_delete_failure_stops_the_import_before_the_store_is_written() {
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
        let conns = dir.path().join("connections.json");
        let before_file = std::fs::read(&conns).unwrap();
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
        assert_eq!(err.details.as_deref(), Some("keychain unavailable"));
        // Секреты не удалились — хранилище не тронуто: ни профилей, ни папок, ни файла на диске; пароль на
        // месте и привязан к прежнему адресу.
        assert_eq!(st.connections.list(), before_profiles);
        assert_eq!(st.connections.list_folders(), before_folders);
        assert_eq!(std::fs::read(&conns).unwrap(), before_file);
        assert_eq!(st.connections.get("a").unwrap().hosts[0].host, "localhost");
        assert!(st.connections.get("n").is_err());
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

    /// Keychain, который при каждом удалении записывает, что в этот момент видно в хранилище подключений,
    /// и может выполнить «чужую» правку хранилища (параллельное сохранение формы).
    struct Watching {
        inner: MemoryStore,
        store: std::sync::OnceLock<Arc<crate::storage::connections::ConnectionStore>>,
        /// (ключ, адрес подключения в хранилище в момент удаления, держалась ли блокировка записи хранилища).
        seen: std::sync::Mutex<Vec<(String, Option<String>, bool)>>,
        on_delete: std::sync::Mutex<Option<OnDelete>>,
    }

    /// Что сделать при удалении секрета (параллельная правка хранилища).
    type OnDelete = Box<dyn FnMut(&str) + Send>;

    impl Watching {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                inner: MemoryStore::default(),
                store: Default::default(),
                seen: Default::default(),
                on_delete: Default::default(),
            })
        }
    }

    impl SecretStore for Watching {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            self.inner.get(key)
        }
        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            self.inner.set(key, value)
        }
        fn delete(&self, key: &str) -> AppResult<()> {
            let store = self.store.get().unwrap();
            let locked = store.write_locked();
            let id = key.split('/').next().unwrap();
            // Под блокировкой записи хранилище не читается (заблокировалось бы): тест упадёт по `locked`.
            let host = (!locked)
                .then(|| store.get(id).ok().map(|c| c.hosts[0].host.clone()))
                .flatten();
            self.seen.lock().unwrap().push((key.into(), host, locked));
            if !locked && let Some(f) = self.on_delete.lock().unwrap().as_mut() {
                f(key);
            }
            self.inner.delete(key)
        }
    }

    #[test]
    fn secrets_are_deleted_before_the_store_gets_the_new_address() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Watching::new();
        let st = AppState::new(dir.path(), secrets.clone()).unwrap();
        let _ = secrets.store.set(st.connections.clone());
        st.connections.upsert(sample_profile("a")).unwrap();
        secrets.set(&secret_key("a", PG_PASSWORD), "pw").unwrap();
        let input = tempfile::tempdir().unwrap();
        import_from_file(&st, &write_file(input.path(), "changed"), None, true, true).unwrap();

        let seen = secrets.seen.lock().unwrap().clone();
        let of_a: Vec<_> = seen
            .iter()
            .filter(|(k, _, _)| k.starts_with("a/"))
            .collect();
        assert_eq!(of_a.len(), 4, "все секреты `a`: {seen:?}");
        for (key, host, locked) in &of_a {
            assert!(
                !locked,
                "{key}: keychain вызван под блокировкой записи хранилища"
            );
            assert_eq!(
                host.as_deref(),
                Some("localhost"),
                "{key}: секрет удалён, пока в хранилище прежний адрес"
            );
        }
        // Новое подключение из файла не подхватывает записи keychain под своим id.
        assert!(
            seen.iter()
                .any(|(k, _, _)| k == &secret_key("n", PG_PASSWORD)),
            "{seen:?}"
        );
        assert!(seen.iter().all(|(_, _, locked)| !locked));
        assert_eq!(st.connections.get("a").unwrap().hosts[0].host, "changed");
        assert_eq!(secrets.get(&secret_key("a", PG_PASSWORD)).unwrap(), None);
    }

    #[test]
    fn orphaned_secrets_under_an_imported_id_are_not_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemoryStore::default());
        let st = AppState::new(dir.path(), secrets.clone()).unwrap();
        // Пароль остался от подключения, удалённого старой версией; файл приносит подключение с тем же id.
        set_all_secrets(secrets.as_ref(), "n");
        let input = tempfile::tempdir().unwrap();
        import_from_file(&st, &write_file(input.path(), "changed"), None, true, true).unwrap();
        assert!(st.connections.get("n").is_ok());
        assert!(secrets_left(secrets.as_ref(), "n").is_empty());
    }

    #[test]
    fn store_changed_during_secret_deletion_is_replanned() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Watching::new();
        let st = AppState::new(dir.path(), secrets.clone()).unwrap();
        let _ = secrets.store.set(st.connections.clone());
        st.connections.upsert(sample_profile("a")).unwrap();
        st.connections.upsert(sample_profile("b")).unwrap();
        set_all_secrets(secrets.as_ref(), "b");
        // Файл: у `a` другой адрес, `b` — как в хранилище (не меняется, пароли остаются).
        let mut moved = json_of(&sample_profile("a"));
        moved["hosts"] = serde_json::json!([{"host": "evil.example", "port": 5432}]);
        let input = tempfile::tempdir().unwrap();
        let path = write_exchange(
            input.path(),
            vec![],
            vec![moved, json_of(&sample_profile("b"))],
        );
        // Пока удаляются секреты `a`, пользователь сохраняет `b` с другим адресом: теперь файл меняет и `b`,
        // и его пароли нужно удалить до записи — импорт пересчитывает план.
        let store = st.connections.clone();
        let mut once = true;
        *secrets.on_delete.lock().unwrap() = Some(Box::new(move |key: &str| {
            if once && key.starts_with("a/") {
                once = false;
                let mut b = sample_profile("b");
                b.hosts[0].host = "moved-by-user".into();
                store.upsert(b).unwrap();
            }
        }));

        let preview = import_from_file(&st, &path, None, true, true).unwrap();
        assert_eq!(preview.updated.len(), 2, "{preview:?}");
        assert_eq!(st.connections.get("b").unwrap().hosts[0].host, "localhost");
        assert!(secrets_left(&secrets.inner, "b").is_empty());
        let seen = secrets.seen.lock().unwrap().clone();
        for (key, host, locked) in seen.iter().filter(|(k, _, _)| k.starts_with("b/")) {
            assert!(!locked, "{key}");
            assert_eq!(
                host.as_deref(),
                Some("moved-by-user"),
                "{key}: удалён до записи импорта"
            );
        }
    }

    #[test]
    fn endless_concurrent_changes_give_up_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Watching::new();
        let st = AppState::new(dir.path(), secrets.clone()).unwrap();
        let _ = secrets.store.set(st.connections.clone());
        // Каждое удаление секретов приносит новое изменённое подключение, которое файл тоже меняет.
        let file: Vec<serde_json::Value> = (0..10)
            .map(|i| {
                let id = format!("c{i}");
                st.connections.upsert(sample_profile(&id)).unwrap();
                json_of(&sample_profile(&id))
            })
            .collect();
        let mut moved = json_of(&sample_profile("a"));
        moved["hosts"] = serde_json::json!([{"host": "evil.example", "port": 5432}]);
        st.connections.upsert(sample_profile("a")).unwrap();
        let input = tempfile::tempdir().unwrap();
        let path = write_exchange(
            input.path(),
            vec![],
            std::iter::once(moved).chain(file).collect(),
        );
        let store = st.connections.clone();
        let mut next = 0;
        *secrets.on_delete.lock().unwrap() = Some(Box::new(move |key: &str| {
            if key.ends_with(PATRONI_KEY_PASSWORD) && next < 10 {
                let mut c = sample_profile(&format!("c{next}"));
                c.hosts[0].host = "moved-by-user".into();
                store.upsert(c).unwrap();
                next += 1;
            }
        }));
        let before = std::fs::read(dir.path().join("connections.json")).unwrap();
        let err = import_from_file(&st, &path, None, true, true).unwrap_err();
        assert_eq!(err.key, "errors.import.storeChanged");
        assert_eq!(st.connections.get("a").unwrap().hosts[0].host, "localhost");
        // Файл менялся только «чужими» сохранениями, импорт его не писал: `a` прежний.
        assert_ne!(
            std::fs::read(dir.path().join("connections.json")).unwrap(),
            before
        );
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
