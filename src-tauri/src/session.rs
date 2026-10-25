//! Длительные сессии (сэмплер, наблюдение — P2b): id → подключение и остановка (спец. P2 §3.5).
//!
//! Порядок блокировок. Мьютекс реестра держится только на время вставки или изъятия записи: остановка сессии
//! (замыкание `stop`) всегда вызывается после того, как блокировка отпущена, — замыкание может взять чужие
//! std-мьютексы (сэмплер: `SamplerRegistry::unsubscribe`), но не этот. Слушатель закрытия вызывается под
//! блокировкой `entries` `Runtimes`, поэтому замыкания остановки не берут `entries` и не ждут (`await`):
//! наблюдение снимает ускорение `Runtimes::unboost`, а тот берёт только мьютекс ускорений `boosts`.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::sync::MutexExt;
use crate::topology::runtime::CloseListener;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Sampler,
    Watch,
}

struct Entry {
    connection_id: String,
    _kind: SessionKind,
    /// Быстрая остановка без `await` (прерывание задачи, отписка): вызывается и из слушателя закрытия.
    stop: Box<dyn FnOnce() + Send>,
}

#[derive(Default)]
pub struct SessionRegistry {
    inner: Mutex<HashMap<String, Entry>>,
}

impl SessionRegistry {
    pub fn insert(
        &self,
        id: String,
        connection_id: &str,
        kind: SessionKind,
        stop: Box<dyn FnOnce() + Send>,
    ) {
        self.inner.lock_safe().insert(
            id,
            Entry {
                connection_id: connection_id.to_string(),
                _kind: kind,
                stop,
            },
        );
    }

    /// Останавливает сессию; `false` — её уже нет. Остановка — вне блокировки реестра.
    pub fn stop(&self, id: &str) -> bool {
        let e = self.inner.lock_safe().remove(id);
        e.map(|e| (e.stop)()).is_some()
    }

    /// Сессия закончилась сама (закрытый канал): убрать без вызова остановки.
    pub fn forget(&self, id: &str) {
        self.inner.lock_safe().remove(id);
    }

    /// Выход из приложения: остановить все сессии.
    pub fn stop_all(&self) {
        let all: Vec<Entry> = self.inner.lock_safe().drain().map(|(_, e)| e).collect();
        for e in all {
            (e.stop)();
        }
    }
}

/// Закрытие подключения останавливает его сессии. Участник, исчезнувший из топологии, сессий не трогает: сэмплер
/// узла останавливает свой реестр (`SamplerRegistry::member_removed`), а сессия ждёт `stop_session` вкладки.
impl CloseListener for SessionRegistry {
    fn connection_closed(&self, connection_id: &str) {
        let mine: Vec<Entry> = {
            let mut m = self.inner.lock_safe();
            let ids: Vec<String> = m
                .iter()
                .filter(|(_, e)| e.connection_id == connection_id)
                .map(|(k, _)| k.clone())
                .collect();
            ids.into_iter().filter_map(|k| m.remove(&k)).collect()
        };
        for e in mine {
            (e.stop)();
        }
    }

    fn member_removed(&self, _: &str, _: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn stop_runs_once_and_connection_close_stops_its_sessions() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let r = SessionRegistry::default();
        let n = Arc::new(AtomicUsize::new(0));
        let add = |id: &str, conn: &str| {
            let n = n.clone();
            r.insert(
                id.into(),
                conn,
                SessionKind::Sampler,
                Box::new(move || {
                    n.fetch_add(1, Ordering::SeqCst);
                }),
            );
        };
        add("a", "c1");
        add("b", "c1");
        add("x", "c2");
        assert!(r.stop("a"));
        assert!(!r.stop("a"), "второй раз — сессии нет");
        r.connection_closed("c1");
        assert_eq!(n.load(Ordering::SeqCst), 2);
        r.forget("x");
        r.stop_all();
        assert_eq!(
            n.load(Ordering::SeqCst),
            2,
            "забытая сессия не останавливается"
        );
    }
}
