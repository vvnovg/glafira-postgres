//! Право сессии на вызовы (спец. P4 §4.3–§4.4): решает приложение, ядро спрашивает на каждый вызов. Перенос
//! `ClientIdent`, `SessionView`, `SessionAccess` и методов `Backend` о сессиях из `model.rs` Glafira for Kafka в
//! отдельный трейт: данных инструментов ядро не читает (их даёт трейт `PgBackend` крейта `glafira-pg-mcp`).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdent {
    pub name: String,
    pub version: String,
}

/// Сессия глазами приложения: клиент становится известен из `initialize` или из `_meta` первого запроса
/// (протокол 2026-07-28 без `initialize`) и дальше не меняется.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    pub id: String,
    pub client: Option<ClientIdent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAccess {
    Approved,
    /// Ждёт ответа пользователя (диалог подтверждения).
    Pending,
    /// Пользователь отклонил клиента: сессия закрывается.
    Rejected,
    /// MCP выключен или запрещён политикой.
    Disabled,
}

pub trait Gate: Send + Sync + 'static {
    /// Право сессии; вызывается на каждый вызов, при `initialize` и списке инструментов — должно быть быстрым.
    /// Новый клиент: реализация ставит сессию в очередь подтверждения и отвечает `Pending`.
    fn access(&self, session: &SessionView) -> SessionAccess;
    /// Имя подключения для журнала; `None`, если подключения нет.
    fn connection_name(&self, id: &str) -> Option<String>;
    /// Сессия закрыта (снять ожидающее подтверждение и т. п.).
    fn session_closed(&self, session_id: &str);
    /// Сессии изменились: открыта, закрыта, опознан клиент, начат или закончен вызов.
    fn sessions_changed(&self);
}
