use std::path::Path;

use tauri::State;
use time::format_description::well_known::Rfc3339;

use super::blocking;
use crate::conn::profile::same_connection_settings;
use crate::error::{AppError, AppResult};
use crate::license::Tier;
use crate::state::AppState;
use crate::storage::connections::{ConnectionFolder, ConnectionProfile};
use crate::storage::exchange::{
    ExportSummary, ImportOptions, ImportPreview, build_export, parse_exchange, plan_import,
};
use crate::storage::fs::write_json;
use crate::storage::tree::TreeItemRef;

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
/// Секреты не читаются и не пишутся: пароли новых подключений вводит пользователь.
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

fn import_from_file(
    st: &AppState,
    path: &Path,
    target: Option<&str>,
    update_existing: bool,
    apply: bool,
) -> AppResult<ImportPreview> {
    let parsed = parse_exchange(&std::fs::read(path)?)?;
    let opts = ImportOptions {
        target,
        update_existing,
        // Те же правила Pro, что при сохранении формы: без лицензии prod-подключение
        // приходит только для чтения.
        is_pro: st.license.status().tier == Tier::Pro,
    };
    let file_exists = |p: &str| Path::new(p).is_file();
    if !apply {
        return Ok(plan_import(&st.connections.snapshot(), &parsed, &opts, &file_exists)?.preview);
    }
    let (preview, updated) = st.connections.update(|data| {
        let plan = plan_import(data, &parsed, &opts, &file_exists)?;
        // Профили обновлённых подключений: до и после (для решения, нужно ли закрывать соединение).
        let updated: Vec<(ConnectionProfile, ConnectionProfile)> = plan
            .preview
            .updated
            .iter()
            .filter_map(|item| {
                let before = data.connections.iter().find(|c| c.id == item.id)?;
                let after = plan.next.connections.iter().find(|c| c.id == item.id)?;
                Some((before.clone(), after.clone()))
            })
            .collect();
        *data = plan.next;
        Ok((plan.preview, updated))
    })?;
    // Как при сохранении формы: открытое подключение переоткрывается, только если изменились
    // настройки соединения; правка оформления обновляет профиль в нём.
    for (before, after) in &updated {
        if same_connection_settings(before, after) {
            st.runtimes.refresh_profile(after);
        } else {
            st.runtimes.disconnect(&after.id);
        }
    }
    Ok(preview)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::conn::profile::sample_profile;
    use crate::secrets::{MemoryStore, SecretStore, secret_key};
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
        assert_eq!(preview.needs_passwords, 1);
        // Free: prod-подключение станет только для чтения.
        assert_eq!(preview.warnings[0].key, "warnings.import.prodReadOnly");
        assert_eq!(std::fs::read(&conns).unwrap(), before);

        let applied = import_from_file(&st, &path, None, true, true).unwrap();
        assert_eq!(applied, preview);
        assert_eq!(st.connections.get("a").unwrap().hosts[0].host, "changed");
        assert!(st.connections.get("n").unwrap().read_only);
        let reopened = crate::storage::connections::ConnectionStore::open(dir.path()).unwrap();
        assert_eq!(reopened.list().len(), 2);
        // Секреты импорт не трогает: свой пароль остаётся, для нового ничего не записано.
        assert_eq!(
            secrets
                .get(&secret_key("a", crate::secrets::PG_PASSWORD))
                .unwrap()
                .as_deref(),
            Some("local-pw")
        );
        assert_eq!(
            secrets
                .get(&secret_key("n", crate::secrets::PG_PASSWORD))
                .unwrap(),
            None
        );
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
        let result = import_from_file(&st, &path, None, true, true);
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err());
        assert_eq!(std::fs::read(&conns).unwrap(), before);
        assert_eq!(st.connections.list().len(), 1);
        assert_eq!(st.connections.get("a").unwrap().hosts[0].host, "localhost");
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
