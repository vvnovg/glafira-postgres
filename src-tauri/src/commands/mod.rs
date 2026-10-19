pub mod app;
pub mod connections;
pub mod folders;
pub mod license;
pub mod settings;

use crate::error::{AppResult, ErrorCode};

/// Блокирующие вызовы (keychain, файлы) не выполняются на async-потоках Tauri.
pub(crate) async fn blocking<T, F>(f: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> AppResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| {
        app_err!(
            ErrorCode::Internal,
            "errors.app.taskPanicked",
            "Background task failed: {{error}}",
            error = e
        )
    })?
}
