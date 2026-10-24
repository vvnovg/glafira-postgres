use std::collections::BTreeMap;

use serde::Serialize;

use crate::i18n::LocalizedText;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    ConnectionFailed,
    TlsError,
    AuthFailed,
    NotAuthorized,
    Timeout,
    InvalidInput,
    NotFound,
    /// Patroni REST API ответил ошибкой или неожиданным ответом.
    PatroniError,
    /// Узел не отвечает или пока не принимает соединения (запускается, в восстановлении).
    NodeUnavailable,
    /// Туннель ssh: нет клиента, ключ хоста, вход, проброс, процесс (вид — в `params.kind`).
    SshError,
    /// Возможность недоступна в этой среде (например, OpenSSH старше 8.4 для askpass).
    FeatureUnavailable,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct AppError {
    pub code: ErrorCode,
    /// Ключ словаря интерфейса (`src/locales/*.json`).
    pub key: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// Английский текст (для логов и как запасной вариант перевода).
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    pub retryable: bool,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn localized(code: ErrorCode, text: LocalizedText) -> Self {
        Self {
            code,
            key: text.key,
            params: text.params,
            message: text.text,
            details: None,
            retryable: Self::retryable_code(code),
        }
    }

    fn retryable_code(code: ErrorCode) -> bool {
        matches!(
            code,
            ErrorCode::ConnectionFailed | ErrorCode::Timeout | ErrorCode::NodeUnavailable
        )
    }

    /// Текст ошибки для полей-сообщений (ключ, параметры, английский текст).
    pub fn text(&self) -> LocalizedText {
        LocalizedText {
            key: self.key.clone(),
            params: self.params.clone(),
            text: self.message.clone(),
        }
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// Неожиданная внутренняя ошибка: общий текст «Internal error», подробности — в `details`
    /// (технический текст, не переводится).
    pub fn internal_with(details: impl Into<String>) -> Self {
        app_err!(ErrorCode::Internal, "errors.internal", "Internal error").with_details(details)
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        app_err!(
            ErrorCode::Internal,
            "errors.io",
            "I/O error: {{error}}",
            error = e
        )
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        app_err!(
            ErrorCode::Internal,
            "errors.json",
            "JSON error: {{error}}",
            error = e
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_for_frontend() {
        let e = app_err!(
            ErrorCode::NodeUnavailable,
            "errors.x",
            "Node {{n}}",
            n = "pg-1"
        );
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["code"], "NODE_UNAVAILABLE");
        assert_eq!(v["key"], "errors.x");
        assert_eq!(v["params"]["n"], "pg-1");
        assert_eq!(v["message"], "Node pg-1");
        assert_eq!(v["retryable"], true);
        assert!(v.get("details").is_none());
    }

    #[test]
    fn ssh_codes_serialize_for_frontend() {
        let v = |code| {
            serde_json::to_value(AppError::localized(code, LocalizedText::raw("x"))).unwrap()
        };
        assert_eq!(v(ErrorCode::SshError)["code"], "SSH_ERROR");
        assert_eq!(
            v(ErrorCode::FeatureUnavailable)["code"],
            "FEATURE_UNAVAILABLE"
        );
    }

    #[test]
    fn retryable_codes() {
        for (code, retry) in [
            (ErrorCode::ConnectionFailed, true),
            (ErrorCode::Timeout, true),
            (ErrorCode::NodeUnavailable, true),
            (ErrorCode::AuthFailed, false),
            (ErrorCode::PatroniError, false),
            // Перезапускаемость `SshError` задаётся при создании ошибки (виды `exited` и `forwardFailed`).
            (ErrorCode::SshError, false),
            (ErrorCode::FeatureUnavailable, false),
        ] {
            assert_eq!(
                AppError::localized(code, LocalizedText::raw("x")).retryable,
                retry,
                "{code:?}"
            );
        }
    }
}
