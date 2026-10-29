//! Команды планов (спец. P3 §10): EXPLAIN на узле и разбор вставленного плана без сервера.

use pg_advisor::plan::PlanTree;
use pg_advisor::plan::json::{from_json, from_psql_text};
use tauri::State;

use crate::error::{AppResult, ErrorCode};
use crate::explain::{ExplainRequest, ExplainResult};
use crate::state::AppState;

#[tauri::command]
pub async fn explain(
    state: State<'_, AppState>,
    id: String,
    request: ExplainRequest,
) -> AppResult<ExplainResult> {
    crate::explain::run::run(&state, &id, &request).await
}

/// Вставленный план: JSON, в том числе с обрамлением psql (P3b — и текст). Без сервера.
#[tauri::command]
pub fn parse_plan(text: String) -> AppResult<PlanTree> {
    let value = from_psql_text(&text).map_err(|e| {
        app_err!(
            ErrorCode::InvalidInput,
            "errors.plan.unparsed",
            "The plan could not be parsed (line {{line}}): {{message}}",
            line = e.line.unwrap_or(1),
            message = e.message
        )
    })?;
    from_json(&value).map_err(|e| {
        app_err!(
            ErrorCode::InvalidInput,
            "errors.plan.unparsed",
            "The plan could not be parsed (line {{line}}): {{message}}",
            line = e.line.unwrap_or(1),
            message = e.message
        )
    })
}
