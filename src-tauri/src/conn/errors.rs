//! Ошибки PostgreSQL → `AppError` (таблица §13 спеки P1).

use tokio_postgres::error::SqlState;

use crate::error::{AppError, ErrorCode};

/// Текст ошибки вместе с цепочкой причин (`error connecting to server: Connection refused`).
pub fn chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        let t = s.to_string();
        if !out.contains(&t) {
            out.push_str(": ");
            out.push_str(&t);
        }
        cur = s.source();
    }
    out
}

pub fn from_pg(e: &tokio_postgres::Error) -> AppError {
    if let Some(db) = e.as_db_error() {
        let code = db.code();
        let details = match db.detail() {
            Some(d) => format!("{}: {} ({d})", code.code(), db.message()),
            None => format!("{}: {}", code.code(), db.message()),
        };
        let err = if *code == SqlState::INVALID_PASSWORD
            || *code == SqlState::INVALID_AUTHORIZATION_SPECIFICATION
        {
            app_err!(
                ErrorCode::AuthFailed,
                "errors.pg.authFailed",
                "Authentication failed: check the user, the password and pg_hba.conf"
            )
        } else if *code == SqlState::INVALID_CATALOG_NAME {
            app_err!(
                ErrorCode::InvalidInput,
                "errors.pg.databaseNotFound",
                "The database does not exist"
            )
        } else if *code == SqlState::INSUFFICIENT_PRIVILEGE {
            app_err!(
                ErrorCode::NotAuthorized,
                "errors.pg.notAuthorized",
                "Not enough privileges for the query"
            )
        } else if *code == SqlState::QUERY_CANCELED {
            app_err!(
                ErrorCode::Timeout,
                "errors.pg.statementTimeout",
                "The query was interrupted by statement_timeout"
            )
        } else if *code == SqlState::TOO_MANY_CONNECTIONS {
            app_err!(
                ErrorCode::ConnectionFailed,
                "errors.pg.tooManyConnections",
                "The server has no free connections (max_connections)"
            )
        } else if *code == SqlState::CANNOT_CONNECT_NOW {
            app_err!(
                ErrorCode::NodeUnavailable,
                "errors.pg.cannotConnectNow",
                "The server is starting up or in recovery"
            )
        } else {
            app_err!(
                ErrorCode::Internal,
                "errors.pg.serverError",
                "PostgreSQL error {{code}}",
                code = code.code()
            )
        };
        return err.with_details(details);
    }
    let text = chain(e);
    let lower = text.to_lowercase();
    let err = if lower.contains("password missing") {
        app_err!(
            ErrorCode::AuthFailed,
            "errors.pg.passwordMissing",
            "The server asked for a password, but none is saved or found in ~/.pgpass"
        )
    } else if lower.contains("server does not support tls") {
        app_err!(
            ErrorCode::TlsError,
            "errors.pg.tlsNotSupported",
            "The server does not accept TLS connections: choose sslmode prefer or disable"
        )
    } else if lower.contains("hostname mismatch") || lower.contains("ip address mismatch") {
        app_err!(
            ErrorCode::TlsError,
            "errors.pg.hostnameMismatch",
            "The server host name does not match its certificate"
        )
    } else if lower.contains("certificate verify failed") {
        app_err!(
            ErrorCode::TlsError,
            "errors.pg.caUntrusted",
            "The server certificate is not signed by a trusted CA"
        )
    } else if lower.contains("tls") || lower.contains("ssl") || lower.contains("handshake") {
        app_err!(
            ErrorCode::TlsError,
            "errors.pg.tlsError",
            "TLS connection error"
        )
    } else if e.is_closed() {
        app_err!(
            ErrorCode::ConnectionFailed,
            "errors.pg.connectionClosed",
            "The connection to the server was closed"
        )
    } else {
        app_err!(
            ErrorCode::ConnectionFailed,
            "errors.pg.connectionFailed",
            "Could not connect to the server"
        )
    };
    err.with_details(text)
}

pub fn tcp_error(e: &std::io::Error, host: &str, port: u16) -> AppError {
    app_err!(
        ErrorCode::ConnectionFailed,
        "errors.pg.tcpFailed",
        "Could not reach {{host}}:{{port}}",
        host = host,
        port = port
    )
    .with_details(e.to_string())
}

pub fn connect_timeout(host: &str, port: u16, secs: u64) -> AppError {
    app_err!(
        ErrorCode::Timeout,
        "errors.pg.connectTimeout",
        "No answer from {{host}}:{{port}} within {{secs}} s",
        host = host,
        port = port,
        secs = secs
    )
}

pub fn tls_setup(e: impl std::fmt::Display) -> AppError {
    app_err!(
        ErrorCode::TlsError,
        "errors.pg.tlsSetup",
        "Could not prepare TLS settings"
    )
    .with_details(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Outer(std::io::Error);
    impl std::fmt::Display for Outer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "error connecting to server")
        }
    }
    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn chain_joins_sources() {
        let e = Outer(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "Connection refused",
        ));
        assert_eq!(chain(&e), "error connecting to server: Connection refused");
    }

    #[test]
    fn tcp_and_timeout_texts() {
        let e = tcp_error(&std::io::Error::other("boom"), "db1", 5432);
        assert_eq!(e.code, ErrorCode::ConnectionFailed);
        assert_eq!(e.message, "Could not reach db1:5432");
        assert!(e.retryable);
        assert_eq!(
            connect_timeout("db1", 5432, 10).message,
            "No answer from db1:5432 within 10 s"
        );
    }
}
