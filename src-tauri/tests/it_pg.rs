//! Соединение с PostgreSQL стенда (`GPG_IT_SINGLE=localhost:5460`).

mod common;

use glafira_pg_lib::conn::errors::from_pg;
use glafira_pg_lib::conn::pg::ConnectSpec;
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
