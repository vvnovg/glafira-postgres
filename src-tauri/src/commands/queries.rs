//! Команды инструмента «Запросы» и карточки запроса (спец. P3 §10).

use pg_advisor::queries::QueryKey;
use tauri::State;

use crate::diag::queries::QueryText;
use crate::error::AppResult;
use crate::state::AppState;

/// Тексты запросов по ключам (ленивая подгрузка видимых строк).
#[tauri::command]
pub async fn query_texts(
    state: State<'_, AppState>,
    id: String,
    member: Option<String>,
    keys: Vec<QueryKey>,
) -> AppResult<Vec<QueryText>> {
    crate::diag::queries::texts(&state, &id, member.as_deref(), keys).await
}
