//! Каталог для дерева: базы, схемы, объекты, партиции, расширения, роли (спец. P1 §8).
//! Запросы идут на узле каталога открытого подключения.

use std::future::Future;
use std::time::Duration;

use futures_util::future::BoxFuture;
use tauri::State;
use tokio::time::{Instant, timeout_at};
use tokio_postgres::Client;

use crate::catalog::databases::DatabaseInfo;
use crate::catalog::extensions::ExtensionInfo;
use crate::catalog::objects::{ObjectKind, ObjectPage};
use crate::catalog::roles::RoleInfo;
use crate::catalog::schemas::SchemaInfo;
use crate::catalog::{databases, extensions, objects, roles, schemas};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::AppState;
use crate::topology::probe::{COLLECT_TIMEOUT, no_answer};

/// Ошибка говорит об обрыве или молчании соединения, а не об ответе сервера: такое соединение закрывают
/// (`PooledClient::discard`), а не возвращают в пул. Функции каталога отдают уже `AppError`, поэтому
/// признак восстанавливается по коду: ошибки без ответа сервера (`conn::errors::connection_suspect`)
/// всегда становятся `CONNECTION_FAILED` или `TLS_ERROR`. Лишнее закрытие здорового соединения
/// (`CONNECTION_FAILED` по SQLSTATE 53300) дешевле, чем возврат негодного в пул.
fn connection_lost(e: &AppError) -> bool {
    matches!(e.code, ErrorCode::ConnectionFailed | ErrorCode::TlsError)
}

/// Выполняет `fut` до `deadline`. Второй элемент ответа — нужно ли закрыть соединение, на котором шёл запрос:
/// по таймауту запрос брошен на полуслове, ответ может не прийти никогда.
async fn bounded<T>(
    deadline: Instant,
    limit: Duration,
    fut: impl Future<Output = AppResult<T>>,
) -> (AppResult<T>, bool) {
    match timeout_at(deadline, fut).await {
        Ok(Ok(v)) => (Ok(v), false),
        Ok(Err(e)) => {
            let discard = connection_lost(&e);
            (Err(e), discard)
        }
        Err(_) => (Err(no_answer(limit)), true),
    }
}

/// Запрос каталога на соединении узла каталога (`db = None` — БД профиля). Получение соединения (в том
/// числе ожидание свободного места в пуле) и сам запрос вместе укладываются в [`COLLECT_TIMEOUT`]:
/// серверный `statement_timeout` не защищает от молчащего пути. Таймаут и обрыв закрывают соединение.
async fn on_catalog_node<T, F>(st: &AppState, id: &str, db: Option<&str>, query: F) -> AppResult<T>
where
    T: Send,
    F: for<'a> FnOnce(&'a Client) -> BoxFuture<'a, AppResult<T>> + Send,
{
    let limit = COLLECT_TIMEOUT;
    let deadline = Instant::now() + limit;
    // Соединения ещё нет — закрывать нечего.
    let c = timeout_at(deadline, st.client(id, None, db))
        .await
        .map_err(|_| no_answer(limit))??;
    let (result, discard) = bounded(deadline, limit, query(&c)).await;
    if discard {
        c.discard();
    }
    result
}

#[tauri::command]
pub async fn list_databases(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<Vec<DatabaseInfo>> {
    let filter = state.profile(&id)?.pg.databases;
    on_catalog_node(&state, &id, None, move |c| {
        Box::pin(async move { databases::list(c, filter.as_deref()).await })
    })
    .await
}

#[tauri::command]
pub async fn list_schemas(
    state: State<'_, AppState>,
    id: String,
    db: String,
    include_system: bool,
) -> AppResult<Vec<SchemaInfo>> {
    on_catalog_node(&state, &id, Some(&db), move |c| {
        Box::pin(schemas::list(c, include_system))
    })
    .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn list_objects(
    state: State<'_, AppState>,
    id: String,
    db: String,
    schema: String,
    kind: ObjectKind,
    filter: Option<String>,
    offset: i64,
    limit: i64,
) -> AppResult<ObjectPage> {
    on_catalog_node(&state, &id, Some(&db), move |c| {
        Box::pin(
            async move { objects::list(c, &schema, kind, filter.as_deref(), offset, limit).await },
        )
    })
    .await
}

#[tauri::command]
pub async fn list_partitions(
    state: State<'_, AppState>,
    id: String,
    db: String,
    schema: String,
    table: String,
    offset: i64,
    limit: i64,
) -> AppResult<ObjectPage> {
    on_catalog_node(&state, &id, Some(&db), move |c| {
        Box::pin(async move { objects::partitions(c, &schema, &table, offset, limit).await })
    })
    .await
}

#[tauri::command]
pub async fn list_extensions(
    state: State<'_, AppState>,
    id: String,
    db: String,
) -> AppResult<Vec<ExtensionInfo>> {
    on_catalog_node(&state, &id, Some(&db), |c| Box::pin(extensions::list(c))).await
}

#[tauri::command]
pub async fn list_roles(
    state: State<'_, AppState>,
    id: String,
    include_system: bool,
) -> AppResult<Vec<RoleInfo>> {
    on_catalog_node(&state, &id, None, move |c| {
        Box::pin(roles::list(c, include_system))
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::secrets::MemoryStore;

    fn err(code: ErrorCode) -> AppError {
        app_err!(code, "errors.test", "test")
    }

    #[test]
    fn only_silent_or_broken_connections_are_discarded() {
        for code in [ErrorCode::ConnectionFailed, ErrorCode::TlsError] {
            assert!(connection_lost(&err(code)), "{code:?}");
        }
        // Ответ сервера (в том числе statement_timeout, SQLSTATE 57014) соединение не портит.
        for code in [
            ErrorCode::Timeout,
            ErrorCode::NotAuthorized,
            ErrorCode::InvalidInput,
            ErrorCode::Internal,
        ] {
            assert!(!connection_lost(&err(code)), "{code:?}");
        }
        assert!(connection_lost(&no_answer(Duration::from_secs(1))));
    }

    #[tokio::test]
    async fn silent_query_times_out_and_asks_to_discard() {
        let limit = Duration::from_millis(30);
        let started = Instant::now();
        let (result, discard) = bounded(
            started + limit,
            limit,
            std::future::pending::<AppResult<()>>(),
        )
        .await;
        let e = result.unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::ConnectionFailed, "errors.pg.noAnswer")
        );
        assert!(discard);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn answered_queries_keep_the_connection_unless_it_broke() {
        let far = Instant::now() + Duration::from_secs(60);
        let limit = Duration::from_secs(60);
        let (r, discard) = bounded(far, limit, async { Ok(5) }).await;
        assert_eq!((r.unwrap(), discard), (5, false));
        // Ошибка запроса (например, неверная страница) — соединение здорово.
        let (r, discard) = bounded(far, limit, async {
            Err::<(), _>(err(ErrorCode::InvalidInput))
        })
        .await;
        assert_eq!(
            (r.unwrap_err().code, discard),
            (ErrorCode::InvalidInput, false)
        );
        // Обрыв посреди запроса — соединение закрывается.
        let (r, discard) = bounded(far, limit, async {
            Err::<(), _>(err(ErrorCode::ConnectionFailed))
        })
        .await;
        assert_eq!(
            (r.unwrap_err().code, discard),
            (ErrorCode::ConnectionFailed, true)
        );
    }

    #[tokio::test]
    async fn closed_connection_fails_before_any_query() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        let e = on_catalog_node(&state, "nope", Some("db"), |c| {
            Box::pin(extensions::list(c))
        })
        .await
        .unwrap_err();
        assert_eq!(e.key, "errors.topology.notConnected");
    }
}
