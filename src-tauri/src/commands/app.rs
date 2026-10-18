use tauri::{AppHandle, Manager};

use crate::diagnostics::AppInfo;
use crate::error::AppResult;

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
