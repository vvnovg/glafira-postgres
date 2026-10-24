//! Дерево блокировок (спец. P2 §5.4): лес «блокирующий → жертвы», циклы и DDL в очереди. Чистая логика без ввода-вывода:
//! сбор строк (`pg_stat_activity` + `pg_locks` + `pg_blocking_pids`) — в `src-tauri/src/diag/locks.rs`.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// Режимы, которые запрашивает DDL: ALTER/DROP/TRUNCATE/LOCK (AccessExclusive), CREATE TRIGGER и FK (ShareRowExclusive),
/// CREATE INDEX без CONCURRENTLY (Share).
pub const DDL_MODES: [&str; 3] = ["AccessExclusiveLock", "ShareRowExclusiveLock", "ShareLock"];

/// Краткая строка сессии: как `SessionRow` без счётчика блокировок. Длительности — от `now()` сервера, секунды.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockSession {
    pub pid: i32,
    /// Вместе с `pid` — ключ сессии, защищающий от переиспользования PID.
    pub backend_start_us: Option<i64>,
    pub user: Option<String>,
    pub application: Option<String>,
    pub database: Option<String>,
    pub state: Option<String>,
    pub query: Option<String>,
    pub xact_age_s: Option<f64>,
    pub query_age_s: Option<f64>,
}

/// Блокировка, которую сессия запросила и не получила (`pg_locks WHERE NOT granted`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingLock {
    pub locktype: Option<String>,
    pub mode: Option<String>,
    /// `relation::regclass` — только для отношения текущей БД соединения.
    pub relation: Option<String>,
    pub relation_oid: Option<i64>,
    /// Имя БД отношения: для отношения другой БД `relation` пуст, и интерфейс показывает «oid N в БД X».
    pub relation_database: Option<String>,
    pub wait_s: Option<f64>,
    /// Ожидание оценено по `state_change` (PostgreSQL 13: нет `pg_locks.waitstart`), а не измерено.
    pub approx: bool,
}

/// Участник цепочки блокировок: ждущий (с `waiting`) или блокирующий. `blocked_by` — `pg_blocking_pids`, только у ждущих.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockRow {
    pub session: LockSession,
    pub blocked_by: Vec<i32>,
    pub waiting: Option<WaitingLock>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockNode {
    pub session: LockSession,
    /// Что узел ждёт; у корня пусто.
    pub lock: Option<WaitingLock>,
    /// Остальные блокирующие: родитель — только первый из `pg_blocking_pids`.
    pub also_blocked_by: Vec<i32>,
    /// Сколько сессий в поддереве (без самого узла).
    pub victims: usize,
    pub children: Vec<LockNode>,
}

/// DDL в очереди: ждёт блокировку отношения в режиме из `DDL_MODES` и сам задерживает других.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DdlQueueItem {
    pub session: LockSession,
    /// Кого ждёт сам DDL (родитель в лесу); у корня-DDL не бывает, но поле допускает пусто.
    pub waits_for: Option<i32>,
    pub wait_s: Option<f64>,
    /// Сколько сессий стоит за DDL.
    pub queued: usize,
    pub relation: Option<String>,
    pub mode: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocksData {
    pub roots: Vec<LockNode>,
    /// Циклы ожидания: pid участников по возрастанию.
    pub cycles: Vec<Vec<i32>>,
    pub ddl_queue: Vec<DdlQueueItem>,
}

/// Лес «блокирующий → жертвы». Родитель жертвы — первый из её `pg_blocking_pids`, кто есть в строках; остальные —
/// в `also_blocked_by`. Корень — сессия, которая сама не ждёт и кого-то держит. Ждущие, от которых по цепочке
/// родителей не дойти до корня, — циклы (deadlock detector разрешит их за `deadlock_timeout`).
pub fn build(rows: &[LockRow]) -> LocksData {
    let index: HashMap<i32, &LockRow> = rows.iter().map(|r| (r.session.pid, r)).collect();
    let parent = |r: &LockRow| {
        r.blocked_by
            .iter()
            .copied()
            .find(|p| *p != r.session.pid && index.contains_key(p))
    };
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for r in rows {
        if let Some(p) = parent(r) {
            children.entry(p).or_default().push(r.session.pid);
        }
    }
    let mut visited = HashSet::new();
    let mut roots: Vec<LockNode> = rows
        .iter()
        .filter(|r| {
            parent(r).is_none() && r.waiting.is_none() && children.contains_key(&r.session.pid)
        })
        .map(|r| grow(r.session.pid, &index, &children, &mut visited, None))
        .collect();
    roots.sort_by(|a, b| {
        b.victims
            .cmp(&a.victims)
            .then(a.session.pid.cmp(&b.session.pid))
    });
    // Циклы: ждущие, до которых лес не дошёл; идём по родителям до повтора.
    let mut cycles: BTreeSet<Vec<i32>> = BTreeSet::new();
    for r in rows
        .iter()
        .filter(|r| r.waiting.is_some() && !visited.contains(&r.session.pid))
    {
        let mut path = vec![r.session.pid];
        let mut cur = r;
        while let Some(p) = parent(cur) {
            if let Some(i) = path.iter().position(|x| *x == p) {
                let mut cycle = path[i..].to_vec();
                cycle.sort_unstable();
                cycles.insert(cycle);
                break;
            }
            path.push(p);
            cur = index[&p];
        }
    }
    let mut ddl_queue = Vec::new();
    for r in &roots {
        find_ddl(r, None, &mut ddl_queue);
    }
    LocksData {
        roots,
        cycles: cycles.into_iter().collect(),
        ddl_queue,
    }
}

/// Узел и его поддерево; `visited` отмечает всех, кто попал в лес.
fn grow(
    pid: i32,
    index: &HashMap<i32, &LockRow>,
    children: &HashMap<i32, Vec<i32>>,
    visited: &mut HashSet<i32>,
    parent_pid: Option<i32>,
) -> LockNode {
    visited.insert(pid);
    let r = index[&pid];
    let kids: Vec<i32> = children
        .get(&pid)
        .into_iter()
        .flatten()
        .copied()
        .filter(|c| !visited.contains(c))
        .collect();
    let kids: Vec<LockNode> = kids
        .into_iter()
        .map(|c| grow(c, index, children, visited, Some(pid)))
        .collect();
    LockNode {
        session: r.session.clone(),
        lock: r.waiting.clone(),
        also_blocked_by: r
            .blocked_by
            .iter()
            .copied()
            .filter(|p| Some(*p) != parent_pid && *p != pid)
            .collect(),
        victims: kids.iter().map(|k| 1 + k.victims).sum(),
        children: kids,
    }
}

/// DDL в очереди: ждущий блокировку отношения в режиме DDL, за которым стоит хотя бы одна сессия.
fn find_ddl(n: &LockNode, parent: Option<i32>, out: &mut Vec<DdlQueueItem>) {
    if let Some(l) = &n.lock
        && l.locktype.as_deref() == Some("relation")
        && l.mode.as_deref().is_some_and(|m| DDL_MODES.contains(&m))
        && !n.children.is_empty()
    {
        out.push(DdlQueueItem {
            session: n.session.clone(),
            waits_for: parent,
            wait_s: l.wait_s,
            queued: n.victims,
            relation: l.relation.clone(),
            mode: l.mode.clone().unwrap_or_default(),
        });
    }
    for c in &n.children {
        find_ddl(c, Some(n.session.pid), out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(pid: i32) -> LockSession {
        LockSession {
            pid,
            backend_start_us: Some(pid as i64),
            ..Default::default()
        }
    }

    fn waiting(pid: i32, blocked_by: &[i32], mode: &str, wait_s: f64) -> LockRow {
        LockRow {
            session: s(pid),
            blocked_by: blocked_by.to_vec(),
            waiting: Some(WaitingLock {
                locktype: Some("relation".into()),
                mode: Some(mode.into()),
                relation: Some("shop.orders".into()),
                wait_s: Some(wait_s),
                ..Default::default()
            }),
        }
    }

    fn holder(pid: i32) -> LockRow {
        LockRow {
            session: s(pid),
            blocked_by: vec![],
            waiting: None,
        }
    }

    #[test]
    fn ddl_in_the_queue_behind_a_long_transaction() {
        // 1 держит строку; 2 — ALTER, ждёт 1; 3 и 4 — SELECT, ждут 2.
        let rows = [
            holder(1),
            waiting(2, &[1], "AccessExclusiveLock", 40.0),
            waiting(3, &[2], "AccessShareLock", 30.0),
            waiting(4, &[2], "AccessShareLock", 5.0),
        ];
        let d = build(&rows);
        assert_eq!(d.roots.len(), 1);
        let root = &d.roots[0];
        assert_eq!((root.session.pid, root.victims), (1, 3));
        assert_eq!(
            root.children
                .iter()
                .map(|c| c.session.pid)
                .collect::<Vec<_>>(),
            [2]
        );
        assert_eq!(
            root.children[0]
                .children
                .iter()
                .map(|c| c.session.pid)
                .collect::<Vec<_>>(),
            [3, 4]
        );
        assert_eq!(d.ddl_queue.len(), 1);
        let q = &d.ddl_queue[0];
        assert_eq!(
            (q.session.pid, q.waits_for, q.queued, q.mode.as_str()),
            (2, Some(1), 2, "AccessExclusiveLock")
        );
        assert!(d.cycles.is_empty());
    }

    #[test]
    fn several_blockers_go_to_the_first_and_the_rest_are_listed() {
        let rows = [
            holder(1),
            holder(2),
            waiting(3, &[2, 1], "RowExclusiveLock", 1.0),
        ];
        let d = build(&rows);
        assert_eq!(
            d.roots.iter().map(|r| r.session.pid).collect::<Vec<_>>(),
            [2],
            "корень без жертв в лес не попадает"
        );
        assert_eq!(d.roots[0].children[0].also_blocked_by, [1]);
        assert!(d.ddl_queue.is_empty(), "RowExclusiveLock — не DDL");
    }

    #[test]
    fn cycles_are_reported_apart_from_the_forest() {
        let rows = [
            waiting(5, &[6], "ShareLock", 1.0),
            waiting(6, &[5], "ShareLock", 1.0),
        ];
        let d = build(&rows);
        assert!(d.roots.is_empty());
        assert_eq!(d.cycles, [vec![5, 6]]);
    }
}
