//! Билеты фоновых запусков (спец. P4c §3.3, §5). Билет — 32 случайных байта hex на запуск. Он живёт только в памяти,
//! от `starting` до конца запуска. Шлюз по билету из рукопожатия узнаёт запуск (`Gate::run_for_ticket`), а по id
//! запуска:
//! - область сессии — подключение и запрос;
//! - счёт вызовов — предел [`MAX_RUN_CALLS`] на запуск во всех его сессиях;
//! - сессию запуска — живая сессия одна, новая (агент перезапустил MCP-сервер) сменяет прежнюю.
//!
//! Ход работы получает очередь запусков через [`RunObserver`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use glafira_mcp_core::error::ToolError;
use glafira_mcp_core::handshake::tokens_equal;

use crate::sync::MutexExt;

/// Предел вызовов инструментов за запуск (решение владельца 9).
pub const MAX_RUN_CALLS: u32 = 40;

/// Область фонового запуска: что видит его сессия.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunScope {
    pub run_id: String,
    /// Подключение запроса; `None` — «Проверить» (запуск без подключения).
    pub connection_id: Option<String>,
    /// Запрос на оптимизацию; `None` — «Проверить».
    pub request_id: Option<String>,
}

/// Ход работы запуска для очереди (спец. P4c §5.4). Вызывается из обвязки вызова шлюза: быстро, без ожиданий.
pub trait RunObserver: Send + Sync {
    /// Сессия запуска подтверждена (`true`) или закрыта (`false`); смена сессии — новое `true` без `false`.
    fn run_session(&self, run_id: &str, connected: bool);
    /// Вызов инструмента сессией запуска. `calls` — вызовов с начала запуска вместе с этим, отклонённые тоже.
    /// `calls > MAX_RUN_CALLS` — предел достигнут, вызов отклонён (`run_limit`).
    fn run_call(&self, run_id: &str, tool: &str, calls: u32);
}

/// Итог [`Tickets::attach_session`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attach {
    /// Сессии нельзя: запуска нет (билет отозван, запуск закончился) или эту сессию сменила новая.
    Gone,
    /// Сессия уже подтверждена.
    Known,
    /// Новая сессия запуска; `replaced` — прежняя, её надо закрыть.
    New { replaced: Option<String> },
}

struct Entry {
    ticket: String,
    scope: RunScope,
    calls: u32,
    session: Option<String>,
    /// Сменённые сессии запуска: вызов, который такая сессия успела послать до закрытия, не возвращает её назад и
    /// не закрывает новую.
    replaced: Vec<String>,
}

#[derive(Default)]
pub struct Tickets {
    runs: Mutex<HashMap<String, Entry>>,
    observer: Mutex<Option<Arc<dyn RunObserver>>>,
}

/// Инструмент, подключение или запрос вне области фонового запуска (спец. P4c §5.2).
pub fn not_in_run() -> ToolError {
    ToolError::new("not_allowed", "Not available to a background run")
}

/// Предел вызовов запуска (спец. P4c §5.2).
pub fn run_limit() -> ToolError {
    ToolError::new(
        "run_limit",
        format!(
            "The background run reached {MAX_RUN_CALLS} tool calls: stop and write the summary"
        ),
    )
}

/// Запуск закончился (билет отозван), а его сессия ещё вызывает.
pub fn run_over() -> ToolError {
    ToolError::new("not_allowed", "The background run is over")
}

/// 32 случайных байта hex, как токен `mcp.json`. Без системного генератора — две UUID v4 (тоже 64 hex).
fn new_ticket() -> String {
    crate::mcp::files::new_token().unwrap_or_else(|e| {
        tracing::warn!("background run ticket: {e}");
        format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        )
    })
}

impl Tickets {
    pub fn new() -> Self {
        Self::default()
    }

    /// Получатель хода работы (очередь запусков); заменяет прежнего.
    pub fn set_observer(&self, o: Arc<dyn RunObserver>) {
        *self.observer.lock_safe() = Some(o);
    }

    fn observer(&self) -> Option<Arc<dyn RunObserver>> {
        self.observer.lock_safe().clone()
    }

    /// Билет запуска `scope.run_id` (на `starting`); прежний билет того же запуска перестаёт действовать, счёт
    /// вызовов и сессия запуска начинаются заново.
    pub fn issue(&self, scope: RunScope) -> String {
        let ticket = new_ticket();
        self.runs.lock_safe().insert(
            scope.run_id.clone(),
            Entry {
                ticket: ticket.clone(),
                scope,
                calls: 0,
                session: None,
                replaced: Vec::new(),
            },
        );
        ticket
    }

    /// Id запуска по билету. Билет сравнивается со всеми живыми билетами за постоянное время
    /// (`handshake::tokens_equal`, как токен `mcp.json`), без раннего выхода из перебора.
    pub fn resolve(&self, ticket: &str) -> Option<String> {
        let runs = self.runs.lock_safe();
        let mut found = None;
        for (id, e) in runs.iter() {
            if tokens_equal(&e.ticket, ticket) {
                found = Some(id.clone());
            }
        }
        found
    }

    pub fn scope(&self, run_id: &str) -> Option<RunScope> {
        self.runs.lock_safe().get(run_id).map(|e| e.scope.clone())
    }

    /// Область сессии: `None` — обычная сессия; запуск закончился — `run_over`.
    pub fn scope_of(&self, run: Option<&str>) -> Result<Option<RunScope>, ToolError> {
        run.map(|id| self.scope(id).ok_or_else(run_over))
            .transpose()
    }

    /// Конец запуска: билет не действует. Новые сессии получают `bad_ticket`, сессия запуска — отказ на следующем
    /// вызове.
    pub fn revoke(&self, run_id: &str) {
        self.runs.lock_safe().remove(run_id);
    }

    /// Вызов сессией запуска — до проверок области: модель, которая упорно зовёт недоступное, тоже упирается в
    /// предел.
    pub fn note_call(&self, run_id: &str, tool: &str) -> Result<u32, ToolError> {
        let calls = {
            let mut runs = self.runs.lock_safe();
            let e = runs.get_mut(run_id).ok_or_else(run_over)?;
            e.calls = e.calls.saturating_add(1);
            e.calls
        };
        if let Some(o) = self.observer() {
            o.run_call(run_id, tool, calls);
        }
        if calls > MAX_RUN_CALLS {
            Err(run_limit())
        } else {
            Ok(calls)
        }
    }

    /// Сессия `session_id` с билетом запуска `run_id` просит право (`Gate::access`).
    pub fn attach_session(&self, run_id: &str, session_id: &str) -> Attach {
        let out = {
            let mut runs = self.runs.lock_safe();
            match runs.get_mut(run_id) {
                None => Attach::Gone,
                Some(e) if e.session.as_deref() == Some(session_id) => Attach::Known,
                Some(e) if e.replaced.iter().any(|s| s == session_id) => Attach::Gone,
                Some(e) => {
                    let replaced = e.session.replace(session_id.to_string());
                    if let Some(old) = &replaced {
                        e.replaced.push(old.clone());
                    }
                    Attach::New { replaced }
                }
            }
        };
        if matches!(out, Attach::New { .. })
            && let Some(o) = self.observer()
        {
            o.run_session(run_id, true);
        }
        out
    }

    /// Сессия закрыта (`Gate::session_closed`): если это текущая сессия запуска, наблюдатель узнаёт об этом.
    pub fn session_closed(&self, session_id: &str) {
        let run = {
            let mut runs = self.runs.lock_safe();
            runs.iter_mut()
                .find(|(_, e)| e.session.as_deref() == Some(session_id))
                .map(|(id, e)| {
                    e.session = None;
                    id.clone()
                })
        };
        if let Some(run) = run
            && let Some(o) = self.observer()
        {
            o.run_session(&run, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(run: &str) -> RunScope {
        RunScope {
            run_id: run.into(),
            connection_id: Some("c1".into()),
            request_id: Some("r1".into()),
        }
    }

    /// Запоминает ход работы строками.
    #[derive(Default)]
    struct Seen(Mutex<Vec<String>>);

    impl RunObserver for Seen {
        fn run_session(&self, run_id: &str, connected: bool) {
            self.0
                .lock()
                .unwrap()
                .push(format!("{run_id} session {connected}"));
        }

        fn run_call(&self, run_id: &str, tool: &str, calls: u32) {
            self.0
                .lock()
                .unwrap()
                .push(format!("{run_id} {tool} {calls}"));
        }
    }

    #[test]
    fn tickets_are_random_hex_and_resolve_to_their_run() {
        let t = Tickets::new();
        let a = t.issue(scope("a1"));
        let b = t.issue(scope("a2"));
        assert!(glafira_mcp_core::handshake::valid_ticket(&a), "{a}");
        assert_ne!(a, b);
        assert_eq!(t.resolve(&a).as_deref(), Some("a1"));
        assert_eq!(t.resolve(&b).as_deref(), Some("a2"));
        assert_eq!(t.scope("a1"), Some(scope("a1")));
        // Отличие в последнем символе, короче, пусто — не билет.
        let mut near = a.clone();
        let last = near.pop().unwrap();
        near.push(if last == '0' { '1' } else { '0' });
        for wrong in [near.as_str(), &a[..63], ""] {
            assert_eq!(t.resolve(wrong), None, "{wrong:?}");
        }
        // Новый билет того же запуска заменяет прежний.
        let a2 = t.issue(scope("a1"));
        assert_eq!(t.resolve(&a), None);
        assert_eq!(t.resolve(&a2).as_deref(), Some("a1"));
        t.revoke("a1");
        assert_eq!((t.resolve(&a2), t.scope("a1")), (None, None));
        assert_eq!(t.scope_of(Some("a1")), Err(run_over()));
        assert_eq!(t.scope_of(None), Ok(None));
        assert_eq!(t.scope_of(Some("a2")), Ok(Some(scope("a2"))));
    }

    #[test]
    fn calls_are_counted_per_run_up_to_the_limit() {
        let t = Tickets::new();
        let seen = Arc::new(Seen::default());
        t.set_observer(seen.clone());
        t.issue(scope("a1"));
        for n in 1..=MAX_RUN_CALLS {
            assert_eq!(t.note_call("a1", "explain_query"), Ok(n));
        }
        let e = t.note_call("a1", "list_proposals").unwrap_err();
        assert_eq!(e, run_limit());
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "run_limit",
                "The background run reached 40 tool calls: stop and write the summary"
            )
        );
        let lines = seen.0.lock().unwrap();
        assert_eq!(lines.len(), 41);
        assert_eq!(lines[0], "a1 explain_query 1");
        assert_eq!(
            lines[40], "a1 list_proposals 41",
            "наблюдатель видит и отклонённый вызов"
        );
        assert_eq!(t.note_call("a9", "x"), Err(run_over()));
    }

    #[test]
    fn one_live_session_per_run() {
        let t = Tickets::new();
        let seen = Arc::new(Seen::default());
        t.set_observer(seen.clone());
        t.issue(scope("a1"));
        assert_eq!(t.attach_session("a1", "s1"), Attach::New { replaced: None });
        assert_eq!(t.attach_session("a1", "s1"), Attach::Known);
        // Агент перезапустил MCP-сервер: новая сессия сменяет прежнюю.
        assert_eq!(
            t.attach_session("a1", "s2"),
            Attach::New {
                replaced: Some("s1".into())
            }
        );
        // Сменённая сессия назад не возвращается: её вызов, успевший до закрытия, не закрывает новую.
        assert_eq!(t.attach_session("a1", "s1"), Attach::Gone);
        assert_eq!(t.attach_session("a1", "s2"), Attach::Known);
        t.session_closed("s1"); // сменённая — не событие
        t.session_closed("s2");
        assert_eq!(
            *seen.0.lock().unwrap(),
            ["a1 session true", "a1 session true", "a1 session false"]
        );
        assert_eq!(t.attach_session("a2", "s3"), Attach::Gone);
        t.revoke("a1");
        assert_eq!(t.attach_session("a1", "s4"), Attach::Gone);
    }
}
