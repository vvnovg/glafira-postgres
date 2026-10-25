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

/// Предпросмотр отмены или завершения: свежие строки цели с правами (ничего не меняет).
#[tauri::command]
pub async fn preview_session_action(
    state: State<'_, AppState>,
    id: String,
    request: crate::actions::sessions::SessionActionRequest,
) -> AppResult<crate::actions::sessions::SessionActionPreview> {
    crate::actions::sessions::preview(&state, &id, &request).await
}

/// Отмена или завершение сессий из предпросмотра; защита read-only и prod — в Rust.
#[tauri::command]
pub async fn execute_session_action(
    state: State<'_, AppState>,
    id: String,
    request: crate::actions::sessions::SessionActionExecute,
    confirmation: crate::actions::Confirmation,
) -> AppResult<crate::actions::sessions::SessionActionResult> {
    crate::actions::sessions::execute(&state, &id, &request, &confirmation).await
}

/// Предпросмотр удаления слота: свежие данные слота узла с отметкой Patroni (ничего не меняет).
#[tauri::command]
pub async fn preview_drop_slot(
    state: State<'_, AppState>,
    id: String,
    request: crate::actions::slots::SlotDropRequest,
) -> AppResult<crate::actions::slots::SlotDropPreview> {
    crate::actions::slots::preview(&state, &id, &request).await
}

/// Удаление неактивного слота; защита read-only и prod и ввод имени слота — в Rust.
#[tauri::command]
pub async fn drop_slot(
    state: State<'_, AppState>,
    id: String,
    request: crate::actions::slots::SlotDropRequest,
    confirmation: crate::actions::Confirmation,
) -> AppResult<crate::actions::slots::ActionResult> {
    crate::actions::slots::drop(&state, &id, &request, &confirmation).await
}

/// Предпросмотр действия Patroni: кандидаты с готовностью, сессии лидера, участник restart.
#[tauri::command]
pub async fn preview_patroni_action(
    state: State<'_, AppState>,
    id: String,
    action: crate::actions::patroni::PatroniAction,
) -> AppResult<crate::actions::patroni::PatroniActionPreview> {
    crate::actions::patroni::preview(&state, &id, &action).await
}
