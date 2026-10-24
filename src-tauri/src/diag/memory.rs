//! Прошлые счётчики для скоростей и роста (спец. P2 §3.6): ключ — подключение, номер открытия, узел, инструмент.

use std::collections::HashMap;
use std::sync::Mutex;

use pg_advisor::rates::CounterSnapshot;

use crate::diag::Tool;
use crate::sync::MutexExt;
use crate::topology::runtime::CloseListener;

type Key = (String, u64, String, Tool);

#[derive(Default)]
pub struct CounterMemory {
    inner: Mutex<HashMap<Key, CounterSnapshot>>,
}

impl CounterMemory {
    /// Запоминает `next` и возвращает прошлый снимок того же ключа.
    pub fn swap(
        &self,
        connection: &str,
        generation: u64,
        member: &str,
        tool: Tool,
        next: CounterSnapshot,
    ) -> Option<CounterSnapshot> {
        self.inner.lock_safe().insert(
            (connection.to_string(), generation, member.to_string(), tool),
            next,
        )
    }
}

/// Закрытие подключения и исчезновение участника стирают их снимки: память не растёт с каждым открытием.
impl CloseListener for CounterMemory {
    fn connection_closed(&self, connection_id: &str) {
        self.inner.lock_safe().retain(|k, _| k.0 != connection_id);
    }
    fn member_removed(&self, connection_id: &str, member: &str) {
        self.inner
            .lock_safe()
            .retain(|k, _| !(k.0 == connection_id && k.2 == member));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::runtime::CloseListener;

    #[test]
    fn swap_returns_the_previous_snapshot_of_the_same_opening() {
        let m = CounterMemory::default();
        let s = |at| CounterSnapshot {
            at_ms: at,
            started_ms: 1,
            reset_ms: None,
            values: Default::default(),
        };
        assert_eq!(m.swap("c", 1, "pg-1", Tool::Pulse, s(1)), None);
        assert_eq!(
            m.swap("c", 1, "pg-1", Tool::Pulse, s(2)).map(|p| p.at_ms),
            Some(1)
        );
        assert_eq!(
            m.swap("c", 2, "pg-1", Tool::Pulse, s(3)),
            None,
            "новое открытие подключения — с чистого листа"
        );
        assert_eq!(
            m.swap("c", 2, "pg-2", Tool::Pulse, s(3)),
            None,
            "другой узел"
        );
        assert_eq!(
            m.swap("c", 2, "pg-1", Tool::Replication, s(3)),
            None,
            "другой инструмент"
        );
        m.member_removed("c", "pg-1");
        assert_eq!(m.swap("c", 2, "pg-1", Tool::Pulse, s(4)), None);
        m.connection_closed("c");
        assert_eq!(m.swap("c", 2, "pg-2", Tool::Pulse, s(5)), None);
    }
}
