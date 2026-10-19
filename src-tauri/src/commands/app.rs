use tauri::{AppHandle, Manager, State};

use super::blocking;
use crate::diagnostics::AppInfo;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::AppState;

fn dir(app: &AppHandle, which: &str) -> String {
    let p = match which {
        "data" => app.path().app_data_dir(),
        _ => app.path().app_log_dir(),
    };
    p.map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "—".into())
}

pub fn app_info(app: &AppHandle) -> AppInfo {
    AppInfo {
        name: crate::product::PRODUCT_NAME.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        commit: env!("GIT_COMMIT").into(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        data_dir: dir(app, "data"),
        log_dir: dir(app, "log"),
    }
}

#[tauri::command]
pub fn get_app_info(app: AppHandle) -> AppResult<AppInfo> {
    Ok(app_info(&app))
}

#[tauri::command]
pub async fn export_diagnostics(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> AppResult<()> {
    if path.trim().is_empty() {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.diagnostics.noFile",
            "No file selected for the archive"
        ));
    }
    let st = state.inner().clone();
    let info = app_info(&app);
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| AppError::internal_with(e.to_string()))?;
    blocking(move || {
        crate::diagnostics::write_archive(
            std::path::Path::new(&path),
            &info,
            &st.connections.list(),
            &st.policy.status,
            &st.license.status(),
            &crate::logging::log_files(&log_dir),
        )
    })
    .await
}
