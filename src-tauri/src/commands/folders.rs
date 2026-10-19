use tauri::State;

use super::blocking;
use crate::error::AppResult;
use crate::state::AppState;
use crate::storage::connections::ConnectionFolder;
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
