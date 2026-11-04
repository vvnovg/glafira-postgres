//! Ядро MCP-шлюза Glafira (спец. P4 §3.1): транспорт шима (Unix socket / named pipe), рукопожатие, журнал, лимиты;
//! сессии, подтверждение клиентов и обвязка вызова — задача 5. Ядро ничего не знает о PostgreSQL: инструменты и
//! `ServerHandler` объявляет крейт `glafira-pg-mcp`, данные даёт приложение.

pub mod audit;
pub mod endpoint;
pub mod error;
pub mod handshake;
pub mod limits;

pub use audit::{AuditEvent, AuditLog, CallRecord};
pub use endpoint::{BoxStream, Endpoint};
pub use error::{BoxFuture, ToolError};
