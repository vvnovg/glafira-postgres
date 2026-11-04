//! Ошибка инструмента для LLM (спец. P4 §10.2): стабильный код и английский текст; в ответе — `CallToolResult` с
//! `isError: true` и текстом `<code>: <message>`. Перенос `BackendError` из `model.rs` Glafira for Kafka
//! (`unexpected` → открытый `internal`).

use std::fmt;
use std::future::Future;
use std::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// `code` — стабильный код (`not_allowed`, `invalid_argument`, `NODE_UNAVAILABLE`…), `message` — по-английски.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub code: String,
    pub message: String,
}

impl ToolError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Нет такого подключения или доступ LLM к нему выключен — не различаем (спец. §4.3).
    pub fn not_allowed() -> Self {
        Self::new(
            "not_allowed",
            "Unknown connection or LLM access is off for it; call list_connections",
        )
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new("invalid_argument", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not_found", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ToolError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_code_and_message() {
        let e = ToolError::invalid_argument("limit must be between 1 and 500, got 0");
        assert_eq!(
            e.to_string(),
            "invalid_argument: limit must be between 1 and 500, got 0"
        );
        let na = ToolError::not_allowed();
        assert_eq!(na.code, "not_allowed");
        assert_eq!(
            na.message,
            "Unknown connection or LLM access is off for it; call list_connections"
        );
        assert_eq!(ToolError::not_found("x").code, "not_found");
        assert_eq!(ToolError::internal("x").code, "internal");
        assert_eq!(ToolError::new("NODE_UNAVAILABLE", "down").message, "down");
    }

    #[test]
    fn box_future_is_send() {
        fn send<T: Send>(_: &T) {}
        let f: BoxFuture<'static, u8> = Box::pin(async { 1 });
        send(&f);
    }
}
