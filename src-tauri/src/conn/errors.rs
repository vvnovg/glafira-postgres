//! Ошибки PostgreSQL → `AppError` (таблица §13 спеки P1).

use openssl::x509::X509VerifyResult;
use tokio_postgres::error::SqlState;

use crate::error::{AppError, ErrorCode};
use crate::i18n::LocalizedText;

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

/// Строка таблицы SQLSTATE → код и текст ошибки (§13 спеки). Чистая функция: проверяется без сервера.
pub(crate) fn classify_sqlstate(code: &SqlState) -> (ErrorCode, LocalizedText) {
    if *code == SqlState::INVALID_PASSWORD || *code == SqlState::INVALID_AUTHORIZATION_SPECIFICATION
    {
        (
            ErrorCode::AuthFailed,
            ltext!(
                "errors.pg.authFailed",
                "Authentication failed: check the user, the password and pg_hba.conf"
            ),
        )
    } else if *code == SqlState::INVALID_CATALOG_NAME {
        (
            ErrorCode::InvalidInput,
            ltext!("errors.pg.databaseNotFound", "The database does not exist"),
        )
    } else if *code == SqlState::INSUFFICIENT_PRIVILEGE {
        (
            ErrorCode::NotAuthorized,
            ltext!(
                "errors.pg.notAuthorized",
                "Not enough privileges for the query"
            ),
        )
    } else if *code == SqlState::QUERY_CANCELED {
        (
            ErrorCode::Timeout,
            ltext!(
                "errors.pg.statementTimeout",
                "The query was interrupted by statement_timeout"
            ),
        )
    } else if *code == SqlState::TOO_MANY_CONNECTIONS {
        (
            ErrorCode::ConnectionFailed,
            ltext!(
                "errors.pg.tooManyConnections",
                "The server has no free connections (max_connections)"
            ),
        )
    } else if *code == SqlState::CANNOT_CONNECT_NOW {
        (
            ErrorCode::NodeUnavailable,
            ltext!(
                "errors.pg.cannotConnectNow",
                "The server is starting up or in recovery"
            ),
        )
    } else if *code == SqlState::ADMIN_SHUTDOWN || *code == SqlState::CRASH_SHUTDOWN {
        // 57P01 приходит и при остановке сервера, и от `pg_terminate_backend`: сеанс завершил администратор.
        (
            ErrorCode::NodeUnavailable,
            ltext!(
                "errors.pg.serverShutdown",
                "The server is shutting down or restarting, or an administrator ended the session"
            ),
        )
    } else if *code == SqlState::LOCK_NOT_AVAILABLE {
        (
            ErrorCode::Timeout,
            ltext!(
                "errors.pg.lockTimeout",
                "The query waited for a lock longer than lock_timeout"
            ),
        )
    } else {
        (
            ErrorCode::Internal,
            ltext!(
                "errors.pg.serverError",
                "PostgreSQL error {{code}}",
                code = code.code()
            ),
        )
    }
}

/// Конфликт recovery: запрос на реплике отменён, чтобы применить WAL (40001 — снимок, 40P01 — взаимная блокировка
/// с буфером). Код один с ошибками сериализации и обычных взаимных блокировок, поэтому нужен второй признак.
/// Признаков два, потому что сервер переводит сообщения по `lc_messages` (русский каталог переводит и эту фразу),
/// а приложение не может заставить сервер говорить по-английски: английский текст «conflict with recovery» — это
/// первый признак; второй, не зависящий от локали, — файл исходников сервера (поле `F` ошибки): на PostgreSQL 13–18
/// конфликты recovery поднимаются в `tcop/postgres.c`, а сериализация и взаимные блокировки — в других файлах
/// (`predicate.c`, `deadlock.c`, код кучи и исполнителя). Достаточно любого из двух признаков при подходящем коде.
pub(crate) fn is_recovery_conflict(code: &SqlState, message: &str, file: Option<&str>) -> bool {
    (*code == SqlState::T_R_SERIALIZATION_FAILURE || *code == SqlState::T_R_DEADLOCK_DETECTED)
        && (message.contains("conflict with recovery") || file == Some("postgres.c"))
}

/// Ошибка без ответа сервера (нет `DbError`): классификация по тексту цепочки причин.
/// `closed` — `tokio_postgres::Error::is_closed()`. Порядок проверок важен: сообщение о несовпадении
/// имени хоста содержит и `certificate verify failed`, поэтому идёт раньше.
fn classify_text(text: &str, closed: bool) -> AppError {
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
    } else if closed {
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

pub fn from_pg(e: &tokio_postgres::Error) -> AppError {
    if let Some(db) = e.as_db_error() {
        let code = db.code();
        let details = match db.detail() {
            Some(d) => format!("{}: {} ({d})", code.code(), db.message()),
            None => format!("{}: {}", code.code(), db.message()),
        };
        if is_recovery_conflict(code, db.message(), db.file()) {
            return app_err!(
                ErrorCode::RecoveryConflict,
                "errors.pg.recoveryConflict",
                "The query on the replica was canceled by a recovery conflict"
            )
            .with_details(details);
        }
        let (error_code, text) = classify_sqlstate(code);
        return AppError::localized(error_code, text).with_details(details);
    }
    classify_text(&chain(e), e.is_closed())
}

/// Ошибка рукопожатия TLS с причиной отказа проверки сертификата (`verify` — из обратного вызова, см.
/// `ConnectSpec::connect_with_handle`); без неё — обычная классификация по тексту. Ответ сервера
/// (`ErrorResponse`) важнее: раз он пришёл, рукопожатие прошло.
pub fn from_handshake(e: &tokio_postgres::Error, verify: Option<X509VerifyResult>) -> AppError {
    let Some(v) = verify.filter(|_| e.as_db_error().is_none()) else {
        return from_pg(e);
    };
    // Причину крейт openssl обычно уже дописывает в текст ошибки; если нет — добавляем сами.
    let text = chain(e);
    let reason = v.error_string();
    let details = if text.contains(reason) {
        text
    } else {
        format!("{text}: {reason}")
    };
    let detailed = |err: AppError| err.with_details(details);
    match v.as_raw() {
        openssl_sys::X509_V_ERR_CERT_HAS_EXPIRED => detailed(app_err!(
            ErrorCode::TlsError,
            "errors.pg.certExpired",
            "The server certificate has expired"
        )),
        openssl_sys::X509_V_ERR_CERT_NOT_YET_VALID => detailed(app_err!(
            ErrorCode::TlsError,
            "errors.pg.certNotYetValid",
            "The server certificate is not valid yet"
        )),
        openssl_sys::X509_V_ERR_HOSTNAME_MISMATCH | openssl_sys::X509_V_ERR_IP_ADDRESS_MISMATCH => {
            detailed(app_err!(
                ErrorCode::TlsError,
                "errors.pg.hostnameMismatch",
                "The server host name does not match its certificate"
            ))
        }
        _ => from_pg(e),
    }
}

/// Поток оборвался: в цепочке причин ошибка ввода-вывода «конец потока / сброс / разрыв» или такой текст
/// (TLS-библиотеки и HTTP-клиент оборачивают конец потока в свои ошибки).
pub fn stream_closed(e: &(dyn std::error::Error + 'static)) -> bool {
    use std::io::ErrorKind;
    let mut cur = Some(e);
    while let Some(err) = cur {
        if let Some(io) = err.downcast_ref::<std::io::Error>()
            && matches!(
                io.kind(),
                ErrorKind::UnexpectedEof
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::BrokenPipe
            )
        {
            return true;
        }
        cur = err.source();
    }
    let text = chain(e).to_lowercase();
    [
        "unexpected eof",
        "early eof",
        "connection closed",
        "connection reset",
        "broken pipe",
    ]
    .iter()
    .any(|m| text.contains(m))
}

/// Соединение закрыто до ответа сервера (`ssh -D` закрывает его, когда бастион не смог открыть канал к узлу).
pub fn closed_early(e: &tokio_postgres::Error) -> bool {
    e.is_closed() || stream_closed(e)
}

/// Можно ли доверять соединению после ошибки. Ответ сервера (`ErrorResponse`) соединение не портит;
/// обрыв, ошибка ввода-вывода или протокола — портят: такое соединение закрывают
/// (`PooledClient::discard`), а не возвращают в пул.
pub fn connection_suspect(e: &tokio_postgres::Error) -> bool {
    e.as_db_error().is_none()
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

/// Таймаут TCP/TLS-подключения — `CONNECTION_FAILED` (§13 спеки); `TIMEOUT` — только SQLSTATE 57014.
pub fn connect_timeout(host: &str, port: u16, secs: u64) -> AppError {
    app_err!(
        ErrorCode::ConnectionFailed,
        "errors.pg.connectTimeout",
        "No answer from {{host}}:{{port}} within {{secs}} s",
        host = host,
        port = port,
        secs = secs
    )
}

/// Отказ во входе, который пул запомнил: новых попыток входа не будет, пока подключение не откроют заново
/// (`conn::pool::PgPool::auth_failure`). Подробности — от исходного отказа (ответ сервера).
pub fn login_not_retried(original: &AppError) -> AppError {
    app_err!(
        ErrorCode::AuthFailed,
        "errors.pg.loginNotRetried",
        "Authentication failed; the login is not retried until the connection is reopened: check the user, the password and pg_hba.conf, then choose “Reconnect” in the connection menu"
    )
    .with_details(
        original
            .details
            .clone()
            .unwrap_or_else(|| original.message.clone()),
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

    /// Обрыв потока узнаётся по виду ошибки ввода-вывода в цепочке причин или по тексту; отказ в соединении — нет.
    #[test]
    fn stream_closed_looks_through_the_chain() {
        let io = |kind, text: &str| Outer(std::io::Error::new(kind, text.to_string()));
        assert!(stream_closed(&io(
            std::io::ErrorKind::UnexpectedEof,
            "early"
        )));
        assert!(stream_closed(&io(std::io::ErrorKind::ConnectionReset, "x")));
        assert!(stream_closed(&io(std::io::ErrorKind::BrokenPipe, "x")));
        assert!(stream_closed(&io(
            std::io::ErrorKind::Other,
            "connection closed before message completed"
        )));
        assert!(!stream_closed(&io(
            std::io::ErrorKind::ConnectionRefused,
            "Connection refused"
        )));
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

    #[test]
    fn connect_timeout_is_connection_failed_not_timeout() {
        // §13: TCP-таймаут — CONNECTION_FAILED; TIMEOUT зарезервирован за SQLSTATE 57014.
        let e = connect_timeout("db1", 5432, 10);
        assert_eq!(e.code, ErrorCode::ConnectionFailed);
        assert_eq!(e.key, "errors.pg.connectTimeout");
        assert!(e.retryable);
    }

    #[test]
    fn sqlstate_table() {
        let rows = [
            ("28P01", ErrorCode::AuthFailed, "errors.pg.authFailed"),
            ("28000", ErrorCode::AuthFailed, "errors.pg.authFailed"),
            (
                "3D000",
                ErrorCode::InvalidInput,
                "errors.pg.databaseNotFound",
            ),
            ("42501", ErrorCode::NotAuthorized, "errors.pg.notAuthorized"),
            ("57014", ErrorCode::Timeout, "errors.pg.statementTimeout"),
            (
                "53300",
                ErrorCode::ConnectionFailed,
                "errors.pg.tooManyConnections",
            ),
            (
                "57P03",
                ErrorCode::NodeUnavailable,
                "errors.pg.cannotConnectNow",
            ),
        ];
        for (sqlstate, code, key) in rows {
            let (c, t) = classify_sqlstate(&SqlState::from_code(sqlstate));
            assert_eq!((c, t.key.as_str()), (code, key), "{sqlstate}");
        }
        let (c, t) = classify_sqlstate(&SqlState::from_code("23505"));
        assert_eq!(c, ErrorCode::Internal);
        assert_eq!(t.key, "errors.pg.serverError");
        assert_eq!(t.params["code"], "23505");
        assert_eq!(t.text, "PostgreSQL error 23505");
    }

    #[test]
    fn shutdown_and_lock_timeout_sqlstates_are_classified() {
        use tokio_postgres::error::SqlState;
        for code in [SqlState::ADMIN_SHUTDOWN, SqlState::CRASH_SHUTDOWN] {
            let (c, t) = classify_sqlstate(&code);
            assert_eq!(
                (c, t.key.as_str()),
                (ErrorCode::NodeUnavailable, "errors.pg.serverShutdown")
            );
        }
        let (c, t) = classify_sqlstate(&SqlState::LOCK_NOT_AVAILABLE);
        assert_eq!(
            (c, t.key.as_str()),
            (ErrorCode::Timeout, "errors.pg.lockTimeout")
        );
    }

    #[test]
    fn recovery_conflict_is_recognized_by_code_and_text() {
        let msg = "canceling statement due to conflict with recovery";
        assert!(is_recovery_conflict(
            &SqlState::T_R_SERIALIZATION_FAILURE,
            msg,
            None
        ));
        assert!(is_recovery_conflict(
            &SqlState::T_R_DEADLOCK_DETECTED,
            msg,
            None
        ));
        assert!(!is_recovery_conflict(
            &SqlState::T_R_SERIALIZATION_FAILURE,
            "could not serialize access",
            None
        ));
        assert!(!is_recovery_conflict(&SqlState::QUERY_CANCELED, msg, None));
    }

    #[test]
    fn recovery_conflict_is_recognized_by_server_file_when_message_is_localized() {
        let ru = "выполнение оператора отменено из-за конфликта с восстановлением";
        assert!(is_recovery_conflict(
            &SqlState::T_R_SERIALIZATION_FAILURE,
            ru,
            Some("postgres.c")
        ));
        assert!(is_recovery_conflict(
            &SqlState::T_R_DEADLOCK_DETECTED,
            ru,
            Some("postgres.c")
        ));
        assert!(!is_recovery_conflict(
            &SqlState::T_R_SERIALIZATION_FAILURE,
            ru,
            Some("predicate.c")
        ));
        assert!(!is_recovery_conflict(
            &SqlState::QUERY_CANCELED,
            ru,
            Some("postgres.c")
        ));
    }

    #[test]
    fn text_branches() {
        let rows = [
            (
                "invalid configuration: password missing",
                false,
                ErrorCode::AuthFailed,
                "errors.pg.passwordMissing",
            ),
            (
                "error performing TLS handshake: server does not support TLS",
                false,
                ErrorCode::TlsError,
                "errors.pg.tlsNotSupported",
            ),
            (
                "error performing TLS handshake: certificate verify failed (hostname mismatch)",
                false,
                ErrorCode::TlsError,
                "errors.pg.hostnameMismatch",
            ),
            (
                "SSL routines:tls_post_process_server_certificate:certificate verify failed: (IP address mismatch)",
                false,
                ErrorCode::TlsError,
                "errors.pg.hostnameMismatch",
            ),
            (
                "SSL routines:tls_post_process_server_certificate:certificate verify failed: (self-signed certificate)",
                false,
                ErrorCode::TlsError,
                "errors.pg.caUntrusted",
            ),
            (
                "error performing TLS handshake: connection reset",
                false,
                ErrorCode::TlsError,
                "errors.pg.tlsError",
            ),
            (
                "SSL routines: unexpected eof while reading",
                true,
                ErrorCode::TlsError,
                "errors.pg.tlsError",
            ),
            (
                "handshake aborted",
                false,
                ErrorCode::TlsError,
                "errors.pg.tlsError",
            ),
            (
                "connection closed",
                true,
                ErrorCode::ConnectionFailed,
                "errors.pg.connectionClosed",
            ),
            (
                "error connecting to server: Connection refused",
                false,
                ErrorCode::ConnectionFailed,
                "errors.pg.connectionFailed",
            ),
        ];
        for (text, closed, code, key) in rows {
            let e = classify_text(text, closed);
            assert_eq!((e.code, e.key.as_str()), (code, key), "{text}");
            assert_eq!(e.details.as_deref(), Some(text), "{text}");
        }
    }

    #[test]
    fn text_classification_is_case_insensitive_and_ordered() {
        // Сообщение о несовпадении имени содержит и `certificate verify failed`: имя хоста проверяется раньше.
        let e = classify_text("CERTIFICATE VERIFY FAILED: HOSTNAME MISMATCH", false);
        assert_eq!(e.key, "errors.pg.hostnameMismatch");
        // Пароль важнее общей TLS-ветки.
        let e = classify_text("password missing (tls)", false);
        assert_eq!(e.key, "errors.pg.passwordMissing");
    }
}
