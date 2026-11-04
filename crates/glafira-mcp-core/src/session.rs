//! Реестр сессий сервера (перенос `SessionEntry`, `Shared` и счётчика сессий из `server/mod.rs` Glafira for Kafka):
//! клиент, время начала, число вызовов, «идёт вызов», «идёт длительная операция», лимитер частоты, отмена сессии.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::audit::{AuditLog, now_ms};
use crate::gate::{ClientIdent, Gate};
use crate::limits::{CALLS_PER_MINUTE, GLOBAL_CONCURRENCY, RateLimiter};
use crate::server::{GatewayOptions, SessionSnapshot};

pub(crate) struct SessionEntry {
    pub(crate) client: Option<ClientIdent>,
    pub(crate) started_ms: u64,
    pub(crate) calls: u64,
    pub(crate) in_flight: u32,
    /// Идёт длительная операция (`observe_*`).
    pub(crate) long_running: bool,
    pub(crate) limiter: RateLimiter,
    pub(crate) cancel: CancellationToken,
}

impl SessionEntry {
    pub(crate) fn new(cancel: CancellationToken) -> Self {
        Self {
            client: None,
            started_ms: now_ms(),
            calls: 0,
            in_flight: 0,
            long_running: false,
            limiter: RateLimiter::new(CALLS_PER_MINUTE),
            cancel,
        }
    }
}

/// Общее состояние сервера: право, журнал, параметры, слоты всех сессий и реестр.
pub(crate) struct Shared {
    pub(crate) gate: Arc<dyn Gate>,
    pub(crate) audit: Arc<AuditLog>,
    pub(crate) options: GatewayOptions,
    pub(crate) global: Arc<Semaphore>,
    sessions: Mutex<HashMap<String, SessionEntry>>,
}

impl Shared {
    pub(crate) fn new(
        gate: Arc<dyn Gate>,
        audit: Arc<AuditLog>,
        options: GatewayOptions,
    ) -> Arc<Self> {
        Arc::new(Self {
            gate,
            audit,
            options,
            global: Arc::new(Semaphore::new(GLOBAL_CONCURRENCY)),
            sessions: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn sessions(&self) -> MutexGuard<'_, HashMap<String, SessionEntry>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Открытые сессии: отменённые, которые rmcp ещё закрывает, не показываются.
    pub(crate) fn snapshot(&self) -> Vec<SessionSnapshot> {
        let mut v: Vec<SessionSnapshot> = self
            .sessions()
            .iter()
            .filter(|(_, e)| !e.cancel.is_cancelled())
            .map(|(id, e)| SessionSnapshot {
                id: id.clone(),
                client_name: e.client.as_ref().map(|c| c.name.clone()),
                client_version: e.client.as_ref().map(|c| c.version.clone()),
                started_ms: e.started_ms,
                calls: e.calls,
                active: e.in_flight > 0,
            })
            .collect();
        v.sort_by(|a, b| {
            a.started_ms
                .cmp(&b.started_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        v
    }

    /// Отменяет открытые сессии, подходящие под условие; возвращает их число (уже отменённые не считаются).
    pub(crate) fn cancel_where(&self, pick: impl Fn(&str, &SessionEntry) -> bool) -> usize {
        let mut n = 0;
        for (id, e) in self.sessions().iter() {
            if !e.cancel.is_cancelled() && pick(id, e) {
                e.cancel.cancel();
                n += 1;
            }
        }
        n
    }
}

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_session_id() -> String {
    format!("s{}", NEXT_SESSION.fetch_add(1, Ordering::Relaxed))
}
