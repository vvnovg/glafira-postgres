//! Соединение с PostgreSQL стенда (`GPG_IT_SINGLE=localhost:5460`).

mod common;

use std::time::{Duration, Instant};

use glafira_pg_lib::conn::errors::from_pg;
use glafira_pg_lib::conn::pg::ConnectSpec;
use glafira_pg_lib::conn::pool::PgPool;
use glafira_pg_lib::conn::profile::SslMode;
use glafira_pg_lib::error::ErrorCode;
use glafira_pg_lib::secrets::{MemoryStore, PG_PASSWORD, SecretStore, secret_key};

fn single() -> Option<String> {
    common::env("GPG_IT_SINGLE")
}

fn spec(addr: &str, db: &str, mode: SslMode, password: &str) -> ConnectSpec {
    let mut p = common::server_profile("it", addr);
    p.pg.ssl_mode = mode;
    let s = MemoryStore::default();
    s.set(&secret_key("it", PG_PASSWORD), password).unwrap();
    let (h, port) = common::host_port(addr);
    ConnectSpec::build(&p, &h, port, db, &s).unwrap()
}

#[tokio::test]
async fn session_is_read_only_with_timeouts() {
    let Some(addr) = single() else { return };
    let c = spec(
        &addr,
        "postgres",
        SslMode::Prefer,
        common::PG_PASSWORD_VALUE,
    )
    .connect()
    .await
    .unwrap();
    let row = c
        .query_one(
            "SELECT current_setting('default_transaction_read_only'), current_setting('application_name'), current_setting('statement_timeout')",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "on");
    assert_eq!(row.get::<_, String>(1), "glafira-pg");
    assert_eq!(row.get::<_, String>(2), "5s");
    let err = c
        .batch_execute("CREATE TABLE glafira_must_not_exist ()")
        .await
        .unwrap_err();
    assert!(
        from_pg(&err).details.unwrap().contains("25006"),
        "read-only транзакция"
    );
}

#[tokio::test]
async fn auth_database_and_tls_errors_are_classified() {
    let Some(addr) = single() else { return };
    let e = spec(&addr, "postgres", SslMode::Disable, "wrong")
        .connect()
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::AuthFailed);
    let e = spec(
        &addr,
        "no_such_db",
        SslMode::Disable,
        common::PG_PASSWORD_VALUE,
    )
    .connect()
    .await
    .unwrap_err();
    assert_eq!(e.key, "errors.pg.databaseNotFound");
    // На стенде TLS выключен.
    let e = spec(
        &addr,
        "postgres",
        SslMode::Require,
        common::PG_PASSWORD_VALUE,
    )
    .connect()
    .await
    .unwrap_err();
    assert_eq!(e.key, "errors.pg.tlsNotSupported");
}

#[tokio::test]
async fn closed_port_and_statement_timeout() {
    let Some(addr) = single() else { return };
    let e = spec("127.0.0.1:1", "postgres", SslMode::Disable, "x")
        .connect()
        .await
        .unwrap_err();
    assert_eq!(e.key, "errors.pg.tcpFailed");
    let c = spec(
        &addr,
        "postgres",
        SslMode::Disable,
        common::PG_PASSWORD_VALUE,
    )
    .connect()
    .await
    .unwrap();
    let err = c.simple_query("SELECT pg_sleep(6)").await.unwrap_err();
    assert_eq!(from_pg(&err).key, "errors.pg.statementTimeout");
}

async fn pid(c: &tokio_postgres::Client) -> i32 {
    c.query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0)
}

#[tokio::test]
async fn pool_reuses_limits_and_closes_idle() {
    let Some(addr) = single() else { return };
    let pool = PgPool::new(spec(
        &addr,
        "postgres",
        SslMode::Disable,
        common::PG_PASSWORD_VALUE,
    ));
    let a = pool.get().await.unwrap();
    let first = pid(&a).await;
    drop(a);
    assert_eq!(pool.idle_count(), 1);
    let again = pool.get().await.unwrap();
    assert_eq!(pid(&again).await, first, "соединение переиспользуется");

    let b = pool.get().await.unwrap();
    let c = pool.get().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), pool.get())
            .await
            .is_err(),
        "четвёртое соединение ждёт"
    );
    drop(b);
    let d = tokio::time::timeout(Duration::from_secs(2), pool.get())
        .await
        .unwrap()
        .unwrap();
    drop((again, c, d));
    assert_eq!(pool.idle_count(), 3);
    assert_eq!(pool.close_idle(Duration::ZERO), 3);
    assert_eq!(pool.idle_count(), 0);
}

#[tokio::test]
async fn terminated_connection_is_replaced() {
    let Some(addr) = single() else { return };
    let pool = PgPool::new(spec(
        &addr,
        "postgres",
        SslMode::Disable,
        common::PG_PASSWORD_VALUE,
    ));
    let a = pool.get().await.unwrap();
    let victim = pid(&a).await;
    drop(a);
    let killer = spec(
        &addr,
        "postgres",
        SslMode::Disable,
        common::PG_PASSWORD_VALUE,
    )
    .connect()
    .await
    .unwrap();
    killer
        .execute("SELECT pg_terminate_backend($1)", &[&victim])
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let b = pool.get().await.unwrap();
    assert_ne!(pid(&b).await, victim);
}

/// Простаивающее соединение проверяется пустым запросом; живое — переиспользуется.
#[tokio::test]
async fn live_idle_connection_passes_validation() {
    let Some(addr) = single() else { return };
    let pool = PgPool::with_validate_after(
        spec(
            &addr,
            "postgres",
            SslMode::Disable,
            common::PG_PASSWORD_VALUE,
        ),
        Duration::ZERO,
    );
    let a = pool.get().await.unwrap();
    let first = pid(&a).await;
    drop(a);
    let b = pool.get().await.unwrap();
    assert_eq!(pid(&b).await, first, "проверка прошла, соединение то же");
}

/// Молчащий путь (ни ответа, ни RST): проверка простаивающего соединения упирается в таймаут
/// подключения, `get` не виснет, мёртвое соединение закрывается и заменяется новым.
///
/// Прокси замораживает только уже открытые соединения, новые он пропускает, поэтому замена
/// детерминированно удаётся: `get` занимает ≈ `spec.timeout` (проверка) + время подключения.
#[tokio::test]
async fn stalled_idle_connection_is_dropped_by_validation_timeout() {
    let Some(addr) = single() else { return };
    let proxy = common::PausableProxy::start(&addr).await;
    let mut s = spec(
        &addr,
        "postgres",
        SslMode::Disable,
        common::PG_PASSWORD_VALUE,
    );
    s.tcp_host = "127.0.0.1".into();
    s.tcp_port = proxy.port();
    s.timeout = Duration::from_secs(1);
    let pool = PgPool::with_validate_after(s, Duration::ZERO);

    let a = pool.get().await.unwrap();
    let first = pid(&a).await;
    drop(a);
    assert_eq!(pool.idle_count(), 1);

    proxy.freeze_existing();
    let started = Instant::now();
    let b = tokio::time::timeout(Duration::from_secs(5), pool.get())
        .await
        .expect("get не должен висеть на молчащем соединении")
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(900),
        "проверка должна была дойти до таймаута, прошло {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(2500),
        "get занял {elapsed:?}: таймаут проверки + подключение"
    );
    assert_ne!(pid(&b).await, first, "выдано новое соединение");
    assert_eq!(
        pool.idle_count(),
        0,
        "мёртвое соединение не вернулось в пул"
    );
    drop(b);
    assert_eq!(pool.idle_count(), 1);
}
