//! Право сессии на вызовы (спец. P4 §4.3–§4.4): решает приложение, ядро спрашивает на каждый вызов. Перенос
//! `ClientIdent`, `SessionView`, `SessionAccess` и методов `Backend` о сессиях из `model.rs` Glafira for Kafka в
//! отдельный трейт: данных инструментов ядро не читает (их даёт трейт `PgBackend` крейта `glafira-pg-mcp`).
//! P4c (спец. P4c §3.3, §5.2): сессия фонового запуска приходит с билетом в рукопожатии. Какие инструменты и prompts
//! ей видны и что ей можно вызывать, решает приложение: ядро не знает PostgreSQL.

use crate::error::ToolError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdent {
    pub name: String,
    pub version: String,
}

/// Сессия глазами приложения: клиент становится известен из `initialize` или из `_meta` первого запроса
/// (протокол 2026-07-28 без `initialize`) и дальше не меняется. `run` — id фонового запуска, если шим пришёл с
/// верным билетом (P4c §3.3); задаётся при подключении и не меняется.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    pub id: String,
    pub client: Option<ClientIdent>,
    pub run: Option<String>,
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

/// Отказ `check_call` по умолчанию: инструмент не виден сессии.
pub const HIDDEN_TOOL_MESSAGE: &str = "This tool is not available in this session";

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

    /// P4c: id фонового запуска по билету из рукопожатия (`run=<билет>`, форма уже проверена). `None` — билет
    /// неизвестен или запуск закончился: шим получает `ERR bad_ticket`. Сравнение — за постоянное время.
    fn run_for_ticket(&self, _ticket: &str) -> Option<String> {
        None
    }

    /// P4c: можно ли сессии этот вызов (`connection_id` — из `CallMeta`). Спрашивается на каждый вызов после
    /// `access`; ошибка уходит модели как есть и пишется в журнал. По умолчанию — только видимость инструмента.
    fn check_call(
        &self,
        session: &SessionView,
        tool: &str,
        _connection_id: Option<&str>,
    ) -> Result<(), ToolError> {
        if self.tool_visible(session, tool) {
            Ok(())
        } else {
            Err(ToolError::new("not_allowed", HIDDEN_TOOL_MESSAGE))
        }
    }

    /// P4c: инструмент есть в `tools/list` этой сессии.
    fn tool_visible(&self, _session: &SessionView, _tool: &str) -> bool {
        true
    }

    /// P4c: сессии виден список prompts (`prompts/list`, `prompts/get`).
    fn prompts_visible(&self, _session: &SessionView) -> bool {
        true
    }
}
