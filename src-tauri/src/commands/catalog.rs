//! Каталог для дерева: базы, схемы, объекты, партиции, расширения, роли (спец. P1 §8), и вкладки объектов:
//! таблица, представление, функция, последовательность (§9). Запросы идут на узле каталога открытого
//! подключения.

use std::future::Future;
use std::time::Duration;

use futures_util::future::BoxFuture;
use tauri::State;
use tokio::time::{Instant, timeout_at};
use tokio_postgres::Client;

use crate::catalog::databases::DatabaseInfo;
use crate::catalog::extensions::ExtensionInfo;
use crate::catalog::function::FunctionDetails;
use crate::catalog::objects::{ObjectKind, ObjectPage};
use crate::catalog::roles::RoleInfo;
use crate::catalog::schemas::SchemaInfo;
use crate::catalog::sequence::SequenceDetails;
use crate::catalog::table::TableDetails;
use crate::catalog::view::ViewDetails;
use crate::catalog::{
    databases, extensions, function, objects, roles, schemas, sequence, table, view,
};
use crate::conn::pool::PooledClient;
use crate::conn::profile::ConnectionProfile;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::AppState;
use crate::topology::probe::{COLLECT_TIMEOUT, no_answer};

/// Ошибка говорит об обрыве или молчании соединения, а не об ответе сервера: такое соединение закрывают
/// (`PooledClient::discard`), а не возвращают в пул. Функции каталога отдают уже `AppError`, поэтому
/// признак восстанавливается по коду: ошибки без ответа сервера (`conn::errors::connection_suspect`)
/// всегда становятся `CONNECTION_FAILED` или `TLS_ERROR`, а ответы сервера (SQLSTATE) — другими кодами,
/// кроме 53300 (`too_many_connections` → `CONNECTION_FAILED`) и 57P03 (`cannot_connect_now` →
/// `NODE_UNAVAILABLE`): они возможны лишь при подключении. `NODE_UNAVAILABLE` посреди запроса — остановка
/// сервера (57P01, 57P02): после неё соединение всё равно мертво, его закрывают сразу.
/// Лишнее закрытие здорового соединения дешевле, чем возврат негодного в пул. Это допущение закреплено
/// тестом `server_errors_never_look_like_a_lost_connection` против `classify_sqlstate`.
fn connection_lost(e: &AppError) -> bool {
    matches!(
        e.code,
        ErrorCode::ConnectionFailed | ErrorCode::TlsError | ErrorCode::NodeUnavailable
    )
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

/// Предел на получение соединения и запрос вместе: таймаут подключения профиля + [`COLLECT_TIMEOUT`],
/// как у `PooledConnector::facts_within`. Холодный пул сначала подключается (до `connect_timeout_s`),
/// и один `COLLECT_TIMEOUT` обрывал бы такое подключение, которое ещё успевает.
pub(crate) fn catalog_limit(profile: &ConnectionProfile) -> Duration {
    Duration::from_secs(profile.pg.connect_timeout_s) + COLLECT_TIMEOUT
}

/// Соединение от `acquire` не позже `deadline`; соединения ещё нет — закрывать нечего.
async fn acquire_by(
    deadline: Instant,
    limit: Duration,
    acquire: impl Future<Output = AppResult<PooledClient>>,
) -> AppResult<PooledClient> {
    timeout_at(deadline, acquire)
        .await
        .map_err(|_| no_answer(limit))?
}

/// Запрос на выданном соединении до `deadline`; таймаут и обрыв закрывают соединение, а не возвращают в пул.
async fn query_by<T, F>(
    c: PooledClient,
    deadline: Instant,
    limit: Duration,
    query: F,
) -> AppResult<T>
where
    T: Send,
    F: for<'a> FnOnce(&'a Client) -> BoxFuture<'a, AppResult<T>> + Send,
{
    let (result, discard) = bounded(deadline, limit, query(&c)).await;
    if discard {
        c.discard();
    }
    result
}

/// Запрос каталога на соединении, которое выдаёт `acquire`, не дольше `limit` на получение (в том числе
/// ожидание свободного места в пуле и подключение) и запрос вместе: серверный `statement_timeout`
/// не защищает от молчащего пути. Таймаут и обрыв закрывают соединение, а не возвращают в пул.
/// Предел и получение соединения передаются снаружи, чтобы проверять границы без стенда.
pub(crate) async fn on_catalog_node_within<T, A, F>(
    limit: Duration,
    acquire: A,
    query: F,
) -> AppResult<T>
where
    T: Send,
    A: Future<Output = AppResult<PooledClient>> + Send,
    F: for<'a> FnOnce(&'a Client) -> BoxFuture<'a, AppResult<T>> + Send,
{
    let deadline = Instant::now() + limit;
    let c = acquire_by(deadline, limit, acquire).await?;
    query_by(c, deadline, limit, query).await
}

/// То же с раздельными пределами: `acquire_limit` — на получение соединения (ожидание места в пуле и холодное
/// подключение, в том числе по медленной сети), `query_limit` — на один запрос, отсчитанный от выданного
/// соединения. Долгое подключение не съедает срок запроса, а молчащий узел всё равно не держит запрос дольше
/// `query_limit`, и такое соединение закрывается, а не возвращается в пул.
pub(crate) async fn on_catalog_node_split<T, A, F>(
    acquire_limit: Duration,
    query_limit: Duration,
    acquire: A,
    query: F,
) -> AppResult<T>
where
    T: Send,
    A: Future<Output = AppResult<PooledClient>> + Send,
    F: for<'a> FnOnce(&'a Client) -> BoxFuture<'a, AppResult<T>> + Send,
{
    let c = acquire_by(Instant::now() + acquire_limit, acquire_limit, acquire).await?;
    query_by(c, Instant::now() + query_limit, query_limit, query).await
}

/// Запрос каталога на соединении узла каталога (`db = None` — БД профиля) открытого подключения `id`.
async fn on_catalog_node<T, F>(st: &AppState, id: &str, db: Option<&str>, query: F) -> AppResult<T>
where
    T: Send,
    F: for<'a> FnOnce(&'a Client) -> BoxFuture<'a, AppResult<T>> + Send,
{
    // Профиль открытого подключения (его таймаут подключения); не открыто — `notConnected`.
    let (_, profile) = st.runtimes.target(id, None)?;
    on_catalog_node_within(catalog_limit(&profile), st.client(id, None, db), query).await
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

/// Вкладка таблицы (обычной, партиционированной, внешней).
#[tauri::command]
pub async fn get_table(
    state: State<'_, AppState>,
    id: String,
    db: String,
    schema: String,
    name: String,
) -> AppResult<TableDetails> {
    on_catalog_node(&state, &id, Some(&db), move |c| {
        Box::pin(async move { table::get(c, &schema, &name).await })
    })
    .await
}

/// Вкладка представления или материализованного представления.
#[tauri::command]
pub async fn get_view(
    state: State<'_, AppState>,
    id: String,
    db: String,
    schema: String,
    name: String,
) -> AppResult<ViewDetails> {
    on_catalog_node(&state, &id, Some(&db), move |c| {
        Box::pin(async move { view::get(c, &schema, &name).await })
    })
    .await
}

/// Вкладка функции или процедуры (по OID: имена перегружены).
#[tauri::command]
pub async fn get_function(
    state: State<'_, AppState>,
    id: String,
    db: String,
    oid: i64,
) -> AppResult<FunctionDetails> {
    on_catalog_node(&state, &id, Some(&db), move |c| {
        Box::pin(function::get(c, oid))
    })
    .await
}

/// Вкладка последовательности.
#[tauri::command]
pub async fn get_sequence(
    state: State<'_, AppState>,
    id: String,
    db: String,
    schema: String,
    name: String,
) -> AppResult<SequenceDetails> {
    on_catalog_node(&state, &id, Some(&db), move |c| {
        Box::pin(async move { sequence::get(c, &schema, &name).await })
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio_postgres::error::SqlState;

    use super::*;
    use crate::conn::errors::classify_sqlstate;
    use crate::conn::pool::{MAX_CONNECTIONS, PoolKey, PoolPurpose, PoolRegistry};
    use crate::conn::profile::{SslMode, sample_profile};
    use crate::conn::testutil;
    use crate::secrets::MemoryStore;
    use crate::topology::connector::pooled_client;

    const WAIT: Duration = Duration::from_secs(2);

    fn err(code: ErrorCode) -> AppError {
        app_err!(code, "errors.test", "test")
    }

    #[test]
    fn only_silent_or_broken_connections_are_discarded() {
        // Остановка сервера (57P01, 57P02) — `NodeUnavailable`: соединение после неё всё равно мертво.
        for code in [
            ErrorCode::ConnectionFailed,
            ErrorCode::TlsError,
            ErrorCode::NodeUnavailable,
        ] {
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

    #[test]
    fn server_errors_never_look_like_a_lost_connection() {
        // Допущение `connection_lost`: ответ сервера (SQLSTATE) выглядит обрывом только для 53300
        // (возможно лишь при подключении), остановки сервера 57P01 и 57P02 (после неё соединение
        // мертво) и 57P03 (`NodeUnavailable`, тоже лишь при подключении). Тест сверяет допущение
        // с `classify_sqlstate`: появится другое соответствие — он упадёт.
        const LOST: [&str; 4] = ["53300", "57P01", "57P02", "57P03"];
        let named = [
            "28P01", "28000", "3D000", "42501", "57014", "53300", "57P01", "57P02", "57P03",
            "55P03",
        ];
        // Несколько кодов, которых классификатор не знает, в том числе из класса подключения (08):
        // они приходят от сервера и должны оставаться `Internal`.
        let others = [
            "08000", "08003", "08006", "0A000", "22012", "23505", "25006", "40001", "40P01",
            "42601", "42P01", "53100", "53200", "53400", "54000", "57000", "58030", "P0001",
            "XX000", "ZZZZZ",
        ];
        // Все классы SQLSTATE стандарта и PostgreSQL с общими кодами `…000` и `…001`.
        let classes = [
            "00", "01", "02", "03", "08", "09", "0A", "0B", "0F", "0L", "0P", "0Z", "20", "21",
            "22", "23", "24", "25", "26", "27", "28", "2B", "2D", "2F", "34", "38", "39", "3B",
            "3D", "3F", "40", "42", "44", "53", "54", "55", "57", "58", "72", "F0", "HV", "P0",
            "XX",
        ];
        let swept: Vec<String> = classes
            .iter()
            .flat_map(|c| [format!("{c}000"), format!("{c}001")])
            .collect();
        let mut seen = 0;
        for sqlstate in named
            .iter()
            .chain(&others)
            .copied()
            .chain(swept.iter().map(String::as_str))
        {
            let (code, text) = classify_sqlstate(&SqlState::from_code(sqlstate));
            let lost = connection_lost(&AppError::localized(code, text));
            assert_eq!(lost, LOST.contains(&sqlstate), "{sqlstate}: {code:?}");
            seen += 1;
        }
        assert!(seen > 100, "{seen}");
    }

    #[test]
    fn limit_covers_the_connect_timeout_of_the_profile() {
        let mut p = sample_profile("c");
        p.pg.connect_timeout_s = 30;
        // Холодное подключение дольше `COLLECT_TIMEOUT` не обрывается раньше времени.
        assert_eq!(catalog_limit(&p), Duration::from_secs(30) + COLLECT_TIMEOUT);
        p.pg.connect_timeout_s = 1;
        assert_eq!(catalog_limit(&p), Duration::from_secs(1) + COLLECT_TIMEOUT);
    }

    fn profile() -> ConnectionProfile {
        let mut p = sample_profile("c");
        p.pg.ssl_mode = SslMode::Disable;
        p.pg.connect_timeout_s = 1;
        p
    }

    fn key() -> PoolKey {
        PoolKey {
            connection_id: "c".into(),
            member: "m".into(),
            database: "postgres".into(),
            purpose: PoolPurpose::Catalog,
        }
    }

    /// Соединение из пула к заглушке: так `AppState::client` получает его на настоящем узле.
    async fn node_client(pools: &PoolRegistry, port: u16) -> AppResult<PooledClient> {
        pooled_client(
            pools,
            &profile(),
            Arc::new(MemoryStore::default()),
            "m",
            "127.0.0.1",
            port,
            "postgres",
            PoolPurpose::Catalog,
            None,
        )
        .await
    }

    #[tokio::test]
    async fn waiting_for_a_pool_permit_is_bounded_by_the_limit() {
        let server = testutil::stalling_pg_server();
        let pools = PoolRegistry::default();
        // Все места пула заняты: без предела запрос каталога ждал бы освобождения бесконечно.
        let mut held = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            held.push(node_client(&pools, server.port).await.unwrap());
        }
        let limit = Duration::from_millis(300);
        let started = Instant::now();
        let e = tokio::time::timeout(
            Duration::from_secs(5),
            on_catalog_node_within(limit, node_client(&pools, server.port), |c| {
                Box::pin(extensions::list(c))
            }),
        )
        .await
        .expect("запрос каталога не должен висеть на занятом пуле")
        .unwrap_err();
        assert_eq!(e.key, "errors.pg.noAnswer");
        // В ошибке — предел, который действовал (с округлением вверх), а не постоянная.
        assert_eq!(e.params["secs"], "1");
        let elapsed = started.elapsed();
        assert!(elapsed >= limit, "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
        // Занятые соединения не тронуты.
        assert_eq!(server.closed(), 0);
        drop(held);
    }

    /// Предел вышел посреди запроса: соединение с неотвеченным запросом закрывается, а не возвращается в пул.
    #[tokio::test]
    async fn query_past_the_limit_discards_the_connection() {
        let server = testutil::stalling_pg_server();
        let pools = PoolRegistry::default();
        let started = Instant::now();
        let e = tokio::time::timeout(
            Duration::from_secs(5),
            on_catalog_node_within(
                Duration::from_millis(300),
                node_client(&pools, server.port),
                |c| Box::pin(extensions::list(c)),
            ),
        )
        .await
        .expect("запрос на молчащем узле не должен висеть")
        .unwrap_err();
        assert_eq!(e.key, "errors.pg.noAnswer");
        assert!(started.elapsed() >= Duration::from_millis(300));
        let pool = pools.get(&key()).expect("пул создан");
        assert_eq!(
            pool.idle_count(),
            0,
            "зависшее соединение не вернулось в пул"
        );
        assert!(server.wait_closed(1, WAIT).await, "сокет закрыт");
        assert_eq!(server.accepted(), 1);
    }

    /// Долгое получение соединения не съедает срок запроса: отдельные пределы, запрос отсчитывается от выданного
    /// соединения (медленная сеть до узла).
    #[tokio::test]
    async fn slow_acquire_does_not_eat_the_query_limit() {
        let server = testutil::stalling_pg_server();
        let pools = PoolRegistry::default();
        let slow_node = async {
            tokio::time::sleep(Duration::from_millis(400)).await;
            node_client(&pools, server.port).await
        };
        let v = on_catalog_node_split(
            Duration::from_secs(5),
            Duration::from_millis(300),
            slow_node,
            |_| Box::pin(async { Ok(9) }),
        )
        .await
        .unwrap();
        assert_eq!(v, 9);
        assert_eq!(pools.get(&key()).unwrap().idle_count(), 1);
        assert_eq!(server.closed(), 0);
    }

    /// Получение соединения не вышло в свой предел — ошибка с этим пределом; запрос не начинался.
    #[tokio::test]
    async fn acquire_past_its_limit_fails_with_that_limit() {
        let ran = std::sync::atomic::AtomicBool::new(false);
        let started = Instant::now();
        let e = on_catalog_node_split(
            Duration::from_millis(200),
            Duration::from_secs(30),
            async {
                std::future::pending::<()>().await;
                unreachable!()
            },
            |_| {
                ran.store(true, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async { Ok(()) })
            },
        )
        .await
        .unwrap_err();
        assert_eq!(e.key, "errors.pg.noAnswer");
        assert_eq!(e.params["secs"], "1");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// Запрос на молчащем узле обрывается по своему пределу, а не по пределу получения; соединение закрывается.
    #[tokio::test]
    async fn silent_query_is_cut_by_the_query_limit_and_discards_the_connection() {
        let server = testutil::stalling_pg_server();
        let pools = PoolRegistry::default();
        let started = Instant::now();
        let e = tokio::time::timeout(
            Duration::from_secs(5),
            on_catalog_node_split(
                Duration::from_secs(30),
                Duration::from_millis(300),
                node_client(&pools, server.port),
                |c| Box::pin(extensions::list(c)),
            ),
        )
        .await
        .expect("запрос на молчащем узле не должен висеть до предела получения")
        .unwrap_err();
        assert_eq!(e.key, "errors.pg.noAnswer");
        assert_eq!(e.params["secs"], "1");
        assert!(started.elapsed() >= Duration::from_millis(300));
        assert_eq!(pools.get(&key()).unwrap().idle_count(), 0);
        assert!(server.wait_closed(1, WAIT).await, "сокет закрыт");
    }

    /// Парный случай: ответ получен вовремя — соединение возвращается в пул и остаётся открытым.
    #[tokio::test]
    async fn answered_query_returns_the_connection_to_the_pool() {
        let server = testutil::stalling_pg_server();
        let pools = PoolRegistry::default();
        let v = on_catalog_node_within(
            Duration::from_secs(5),
            node_client(&pools, server.port),
            |_| Box::pin(async { Ok(7) }),
        )
        .await
        .unwrap();
        assert_eq!(v, 7);
        assert_eq!(pools.get(&key()).unwrap().idle_count(), 1);
        assert_eq!(server.closed(), 0);
    }
}
