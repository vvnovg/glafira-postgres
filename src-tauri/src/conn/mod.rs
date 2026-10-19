use crate::error::{AppError, AppResult};

pub mod connstr;
pub mod errors;
pub mod pg;
pub mod pgpass;
pub mod pool;
pub mod profile;
#[cfg(test)]
mod testutil;
pub mod tls;

/// Блокирующая работа (keychain, файлы) вне async-потоков. Работает и в runtime Tauri, и в `#[tokio::test]`.
pub async fn run_blocking<T, F>(f: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> AppResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::internal_with(e.to_string()))?
}
