//! Команды диагностики (спец. P2 §9).

use tauri::State;

use crate::diag::{DiagReport, DiagRequest};
use crate::error::AppResult;
use crate::state::AppState;

#[tauri::command]
pub async fn run_diagnostic(
    state: State<'_, AppState>,
    id: String,
    request: DiagRequest,
) -> AppResult<DiagReport> {
    crate::diag::run(&state, &id, request).await
}
