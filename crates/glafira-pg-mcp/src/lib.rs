//! MCP-шлюз Glafira for PostgreSQL (спец. P4 §3.1, §5): инструменты, схемы параметров, `instructions`, контракт ответов
//! и усечение до 100 КБ. Крейт не ходит в базу: данные он получает через трейт [`model::PgBackend`], который реализует
//! приложение; сессии, право, лимиты и журнал — в `glafira-mcp-core`.

pub mod model;
pub mod params;
pub mod server;
pub mod truncate;

pub use model::{ObserveCtx, PgAnswer, PgBackend, PgObserve, PgOutput, PgRead, ProgressFn};
pub use server::{INSTRUCTIONS, PgFactory, PgServer, TOOL_NAMES};
