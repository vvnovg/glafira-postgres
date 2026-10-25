//! Команды действий P2b (спец. P2 §9).

use tauri::State;

use crate::actions::journal::ActionRecord;
use crate::conn::run_blocking;
use crate::error::AppResult;
use crate::state::AppState;

/// Записи журнала действий подключения (или всех), новые первыми; `limit` не больше 1000.
#[tauri::command]
pub async fn list_actions(
    state: State<'_, AppState>,
    connection_id: Option<String>,
    limit: usize,
) -> AppResult<Vec<ActionRecord>> {
    let journal = state.actions.clone();
    run_blocking(move || Ok(journal.list(connection_id.as_deref(), limit))).await
}
