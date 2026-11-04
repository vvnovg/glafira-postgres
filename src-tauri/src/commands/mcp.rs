//! Команды раздела «LLM-ассистенты (MCP)» в «Настройках» (спец. P4 §11). Изменяющие команды возвращают статус
//! целиком — интерфейс обновляется без отдельного запроса.

use serde_json::Value;
use tauri::State;

use super::blocking;
use crate::error::{AppResult, ErrorCode};
use crate::mcp::{McpClientConfigs, McpStatus, client_configs, launch_path};
use crate::state::AppState;

/// Наибольшее число записей журнала MCP за один запрос (раздел показывает 200).
const MAX_AUDIT_TAIL: u32 = 1000;

/// Статус читает хранилище подключений (блокировку держит запись файла): в фоновом потоке.
#[tauri::command]
pub async fn get_mcp_status(state: State<'_, AppState>) -> AppResult<McpStatus> {
    let st = state.inner().clone();
    blocking(move || Ok(st.mcp.status())).await
}

#[tauri::command]
pub async fn set_mcp_enabled(state: State<'_, AppState>, on: bool) -> AppResult<McpStatus> {
    let st = state.inner().clone();
    let mcp = st.mcp.clone();
    mcp.set_enabled(st, on).await
}

#[tauri::command]
pub async fn answer_mcp_approval(
    state: State<'_, AppState>,
    session_id: String,
    approve: bool,
) -> AppResult<McpStatus> {
    let st = state.inner().clone();
    blocking(move || {
        st.mcp.answer_approval(&session_id, approve)?;
        Ok(st.mcp.status())
    })
    .await
}

#[tauri::command]
pub async fn revoke_mcp_client(state: State<'_, AppState>, name: String) -> AppResult<McpStatus> {
    let st = state.inner().clone();
    blocking(move || {
        st.mcp.revoke_client(&name)?;
        Ok(st.mcp.status())
    })
    .await
}

#[tauri::command]
pub async fn disconnect_mcp_sessions(
    state: State<'_, AppState>,
    session_id: Option<String>,
) -> AppResult<McpStatus> {
    let st = state.inner().clone();
    blocking(move || {
        st.mcp.disconnect(session_id.as_deref());
        Ok(st.mcp.status())
    })
    .await
}

#[tauri::command]
pub async fn get_mcp_audit(state: State<'_, AppState>, limit: u32) -> AppResult<Vec<Value>> {
    let st = state.inner().clone();
    blocking(move || Ok(st.mcp.audit_tail(limit.min(MAX_AUDIT_TAIL) as usize))).await
}

#[tauri::command]
pub fn get_mcp_client_configs() -> AppResult<McpClientConfigs> {
    let exe = std::env::current_exe().map_err(|e| {
        app_err!(
            ErrorCode::Internal,
            "errors.mcp.noExe",
            "Cannot find the Glafira executable: {{error}}",
            error = e
        )
    })?;
    Ok(client_configs(&launch_path(
        &exe,
        std::env::var_os("APPIMAGE"),
    )))
}

#[tauri::command]
pub fn reveal_mcp_audit(state: State<'_, AppState>) -> AppResult<()> {
    tauri_plugin_opener::reveal_item_in_dir(state.mcp.audit_path()).map_err(|e| {
        app_err!(
            ErrorCode::NotFound,
            "errors.mcp.auditMissing",
            "The MCP log is not available yet: {{error}}",
            error = e
        )
    })
}
