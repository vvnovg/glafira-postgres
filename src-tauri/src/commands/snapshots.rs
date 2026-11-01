//! Команды снимков pgss (спец. P3 §10).

use pg_advisor::queries::compare::Comparison;
use tauri::State;

use crate::error::AppResult;
use crate::snapshots::{self, SnapshotInfo};
use crate::state::AppState;

/// Снимок узла `member` подключения `id` — в файл `snapshots/<id>/<member>/<время>.json`.
#[tauri::command]
pub async fn snapshot_take(
    state: State<'_, AppState>,
    id: String,
    member: String,
) -> AppResult<SnapshotInfo> {
    snapshots::take(&state, &id, &member).await
}

/// Снимки подключения, новые первыми (подключение не обязано быть открытым).
#[tauri::command]
pub async fn snapshot_list(state: State<'_, AppState>, id: String) -> AppResult<Vec<SnapshotInfo>> {
    let store = state.snapshots.clone();
    crate::conn::run_blocking(move || store.list(&id)).await
}

/// `false` — файла уже не было.
#[tauri::command]
pub async fn snapshot_delete(
    state: State<'_, AppState>,
    id: String,
    file: String,
) -> AppResult<bool> {
    let store = state.snapshots.clone();
    crate::conn::run_blocking(move || store.delete(&id, &file)).await
}

#[tauri::command]
pub async fn snapshot_compare(
    state: State<'_, AppState>,
    id: String,
    from: String,
    to: String,
) -> AppResult<Comparison> {
    snapshots::compare(&state, &id, &from, &to).await
}
