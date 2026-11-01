//! Кеш итога поиска и пробы `pg_stat_kcache` по узлу (M1/M2 финального ревью P3b). Проба вызывает
//! `pg_stat_kcache()` и проходит весь хеш расширения, а без загруженной библиотеки пишет ERROR в лог сервера; топ
//! «с момента сброса» обновляется каждые 10 с, карточка — каждые 5 с, поэтому итог запоминается на узел:
//! пригодно или не установлено — на `KCACHE_TTL_MS`, отказ пробы от состояния сервера — на `KCACHE_REFUSED_TTL_MS`.
//! Разовый сбой пробы (отмена, тайм-аут, обрыв соединения) не запоминается: следующий поиск пробует снова.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::diag::queries::KcacheCaps;
use crate::diag::query_detail::NodeKey;
use crate::sync::MutexExt;
use crate::topology::runtime::CloseListener;

/// Сколько живёт итог «пригодно» или «не установлено / не пригодно по каталогу»: установленное или удалённое
/// расширение замечается не позже чем через минуту.
pub const KCACHE_TTL_MS: i64 = 60_000;

/// Сколько не пробуется узел, где проба отказала из-за состояния сервера (библиотека не загружена, нет прав, нет
/// функции): каждая такая проба — строка ERROR в логе сервера. Как `WAIT_SAMPLING_RETRY_MS`.
pub const KCACHE_REFUSED_TTL_MS: i64 = 600_000;

/// Итог пробы `pg_stat_kcache()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// Функция отвечает: kcache читается.
    Usable,
    /// Отказ от состояния сервера, который сам не пройдёт: запоминается на `KCACHE_REFUSED_TTL_MS`.
    Refused,
    /// Разовый сбой: не запоминается, kcache в этом чтении не используется.
    Transient,
}

/// Ошибка пробы по SQLSTATE и тексту: `55…` (в том числе «must be loaded via shared_preload_libraries» — `55000`),
/// `42…` (нет прав `42501`, нет функции `42883`, нет объекта), `3F000` (нет схемы), `58P01` (нет файла библиотеки) и
/// `XX000` с упоминанием `shared_preload_libraries` или `pg_stat_kcache` — состояние сервера. Остальное (отмена
/// `57014`, нехватка ресурсов, обрыв соединения без SQLSTATE) — разовое.
pub fn classify_probe_error(code: Option<&str>, message: &str) -> ProbeOutcome {
    let refused = match code {
        Some(c) if c.starts_with("55") || c.starts_with("42") => true,
        Some("3F000" | "58P01") => true,
        Some("XX000") => {
            message.contains("shared_preload_libraries") || message.contains("pg_stat_kcache")
        }
        _ => false,
    };
    if refused {
        ProbeOutcome::Refused
    } else {
        ProbeOutcome::Transient
    }
}

/// Запомненный итог: время и возможности (`None` — не используется) плюс срок жизни записи.
#[derive(Debug, Clone)]
struct Entry {
    at_ms: i64,
    ttl_ms: i64,
    caps: Option<KcacheCaps>,
}

/// Итоги поиска `pg_stat_kcache` по узлам открытых подключений. Устаревшие записи выбрасываются при записи, записи
/// закрытого подключения и ушедшего узла — по `CloseListener`.
#[derive(Debug, Default)]
pub struct KcacheProbes {
    entries: Mutex<HashMap<NodeKey, Entry>>,
}

impl KcacheProbes {
    /// Действующий итог узла: `Some(None)` — kcache не используется, `None` — итога нет (не искали, устарел или был
    /// разовый сбой).
    pub fn cached(&self, node: &NodeKey, now_ms: i64) -> Option<Option<KcacheCaps>> {
        self.entries
            .lock_safe()
            .get(node)
            .filter(|e| now_ms - e.at_ms < e.ttl_ms)
            .map(|e| e.caps.clone())
    }

    /// Итог поиска: пригодно или не установлено — на `KCACHE_TTL_MS`.
    pub fn remember(&self, node: &NodeKey, now_ms: i64, caps: Option<KcacheCaps>) {
        self.put(node, now_ms, KCACHE_TTL_MS, caps);
    }

    /// Отказ пробы от состояния сервера — на `KCACHE_REFUSED_TTL_MS`.
    pub fn remember_refused(&self, node: &NodeKey, now_ms: i64) {
        self.put(node, now_ms, KCACHE_REFUSED_TTL_MS, None);
    }

    /// Итог узла больше не действует: чтение с kcache упало (расширение удалили или сломали) — следующий поиск
    /// проверит заново.
    pub fn forget(&self, node: &NodeKey) {
        self.entries.lock_safe().remove(node);
    }

    fn put(&self, node: &NodeKey, now_ms: i64, ttl_ms: i64, caps: Option<KcacheCaps>) {
        let mut m = self.entries.lock_safe();
        m.retain(|_, e| now_ms - e.at_ms < e.ttl_ms);
        m.insert(
            node.clone(),
            Entry {
                at_ms: now_ms,
                ttl_ms,
                caps,
            },
        );
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock_safe().len()
    }
}

impl CloseListener for KcacheProbes {
    fn connection_closed(&self, connection_id: &str) {
        self.entries
            .lock_safe()
            .retain(|k, _| k.id != connection_id);
    }

    fn member_removed(&self, connection_id: &str, member: &str) {
        self.entries
            .lock_safe()
            .retain(|k, _| !(k.id == connection_id && k.member == member));
    }
}

/// Узел, для которого ищется kcache, и общий кеш итогов (владеемые: уходят в замыкание `on_member`).
#[derive(Debug, Clone)]
pub struct KcacheNode {
    pub probes: Arc<KcacheProbes>,
    pub node: NodeKey,
}

impl KcacheNode {
    pub fn new(probes: &Arc<KcacheProbes>, id: &str, generation: u64, member: &str) -> Self {
        Self {
            probes: probes.clone(),
            node: NodeKey {
                id: id.to_string(),
                generation,
                member: member.to_string(),
            },
        }
    }

    /// Итог узла есть и действует (поиск не кончился разовым сбоем пробы).
    pub fn settled(&self, now_ms: i64) -> bool {
        self.probes.cached(&self.node, now_ms).is_some()
    }

    pub fn forget(&self) {
        self.probes.forget(&self.node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, generation: u64, member: &str) -> NodeKey {
        NodeKey {
            id: id.into(),
            generation,
            member: member.into(),
        }
    }

    fn caps() -> KcacheCaps {
        KcacheCaps {
            schema: "\"public\"".into(),
            exec_prefix: true,
            has_top: true,
        }
    }

    #[test]
    fn probe_errors_from_server_state_are_refusals_and_the_rest_is_transient() {
        for (code, msg) in [
            (
                Some("55000"),
                "pg_stat_kcache must be loaded via shared_preload_libraries",
            ),
            (
                Some("42501"),
                "permission denied for function pg_stat_kcache",
            ),
            (
                Some("42883"),
                "function public.pg_stat_kcache() does not exist",
            ),
            (Some("3F000"), "schema \"x\" does not exist"),
            (Some("58P01"), "could not access file \"pg_stat_kcache\""),
            (
                Some("XX000"),
                "pg_stat_kcache must be loaded via shared_preload_libraries",
            ),
        ] {
            assert_eq!(
                classify_probe_error(code, msg),
                ProbeOutcome::Refused,
                "{code:?}"
            );
        }
        for (code, msg) in [
            (
                Some("57014"),
                "canceling statement due to statement timeout",
            ),
            (Some("53200"), "out of memory"),
            (Some("XX000"), "unexpected"),
            (None, "connection closed"),
        ] {
            assert_eq!(
                classify_probe_error(code, msg),
                ProbeOutcome::Transient,
                "{code:?}"
            );
        }
    }

    #[test]
    fn outcomes_live_for_their_ttl_and_negatives_are_cached_too() {
        let p = KcacheProbes::default();
        let a = node("c1", 1, "pg-1");
        assert_eq!(p.cached(&a, 0), None);
        p.remember(&a, 0, Some(caps()));
        assert_eq!(p.cached(&a, KCACHE_TTL_MS - 1), Some(Some(caps())));
        assert_eq!(p.cached(&a, KCACHE_TTL_MS), None, "устарело");
        p.remember(&a, 0, None);
        assert_eq!(
            p.cached(&a, 1),
            Some(None),
            "«не установлено» тоже запоминается"
        );
        p.remember_refused(&a, 0);
        assert_eq!(
            p.cached(&a, KCACHE_TTL_MS),
            Some(None),
            "отказ живёт дольше"
        );
        assert_eq!(p.cached(&a, KCACHE_REFUSED_TTL_MS), None);
        assert_eq!(
            p.cached(&node("c1", 2, "pg-1"), 1),
            None,
            "переподключение — новый поиск"
        );
        p.forget(&a);
        assert_eq!(p.cached(&a, 1), None);
    }

    #[test]
    fn closing_and_removed_members_drop_entries() {
        let p = KcacheProbes::default();
        p.remember(&node("c1", 1, "pg-1"), 0, None);
        p.remember(&node("c1", 1, "pg-2"), 0, None);
        p.remember(&node("c2", 1, "pg-1"), 0, None);
        p.member_removed("c1", "pg-2");
        assert_eq!(p.len(), 2);
        p.connection_closed("c1");
        assert_eq!(p.len(), 1);
        p.remember(&node("c3", 1, "x"), KCACHE_REFUSED_TTL_MS, None);
        assert_eq!(p.len(), 1, "устаревшие выброшены при записи");
    }

    #[test]
    fn a_node_is_settled_only_with_a_live_outcome() {
        let probes = Arc::new(KcacheProbes::default());
        let n = KcacheNode::new(&probes, "c1", 1, "pg-1");
        assert!(!n.settled(0), "разовый сбой не оставляет итога");
        probes.remember(&n.node, 0, None);
        assert!(n.settled(1));
        n.forget();
        assert!(!n.settled(1));
    }
}
