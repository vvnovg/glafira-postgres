//! Ядро MCP-шлюза Glafira (спец. P4 §3.1): транспорт шима (Unix socket / named pipe), рукопожатие, сессии,
//! подтверждение клиентов (`Gate`), лимиты, журнал и обвязка вызова (`SessionCtx`). Ядро ничего не знает о
//! PostgreSQL: инструменты и `ServerHandler` объявляет крейт `glafira-pg-mcp` и передаёт их через `SessionFactory`,
//! данные даёт приложение.

pub mod audit;
pub mod call;
pub mod compact;
pub mod endpoint;
pub mod error;
pub mod gate;
pub mod handshake;
pub mod limits;
pub mod server;
mod session;

pub use audit::{AuditEvent, AuditLog, CallRecord};
pub use call::{CallMeta, CallOk, DbSlots, LongGuard, SessionCtx};
pub use endpoint::{BoxStream, Endpoint};
pub use error::{BoxFuture, ToolError};
pub use gate::{ClientIdent, Gate, SessionAccess, SessionView};
pub use server::{
    GatewayOptions, McpServerHandle, SessionFactory, SessionSnapshot, serve_stream, start,
};
