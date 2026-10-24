//! Блокировки и DDL в очереди (спец. P2 §5.4): находки по лесу `locktree`.

use crate::findings::{Finding, FindingTarget, Severity, grade, thresholds as th};
use crate::locktree::{LockNode, LockSession, LocksData};

fn session_target(s: &LockSession) -> Option<FindingTarget> {
    s.backend_start_us.map(|us| FindingTarget::Session {
        pid: s.pid,
        backend_start_us: us,
    })
}

/// Самое долгое ожидание среди жертв поддерева: корень отвечает за всех, кто стоит за ним в цепочке.
fn max_wait(n: &LockNode) -> Option<f64> {
    n.children
        .iter()
        .filter_map(|c| c.lock.as_ref().and_then(|l| l.wait_s))
        .chain(n.children.iter().filter_map(max_wait))
        .reduce(f64::max)
}

/// Ожидания всех ждущих в поддереве, включая сам узел.
fn all_waits(n: &LockNode) -> Vec<f64> {
    n.lock
        .as_ref()
        .and_then(|l| l.wait_s)
        .into_iter()
        .chain(n.children.iter().flat_map(all_waits))
        .collect()
}

/// Находки инструмента «Блокировки» на узле `member`. `rootBlocker` и `ddlQueue` — по находке на корень и на
/// элемент очереди (уточнение 2 плана P2a), `lockWait` и `lockCycle` — по узлу.
pub fn evaluate(member: Option<&str>, d: &LocksData) -> Vec<Finding> {
    let mut out = Vec::new();
    for root in &d.roots {
        let wait = max_wait(root).unwrap_or(0.0);
        let sev = if root.victims >= th::ROOT_BLOCKER_CRIT_VICTIMS {
            Some(Severity::Critical)
        } else {
            grade(wait, th::ROOT_BLOCKER_WAIT_S.0, th::ROOT_BLOCKER_WAIT_S.1)
        };
        if let Some(sev) = sev {
            let mut f = Finding::new("rootBlocker", sev)
                .member(member)
                .param("count", root.victims)
                .param("maxS", wait)
                .link_tool("sessions", member);
            if let Some(t) = session_target(&root.session) {
                f = f.target(t);
            }
            out.push(f);
        }
    }
    for q in &d.ddl_queue {
        let mut f = Finding::new("ddlQueue", Severity::Critical)
            .member(member)
            .param("count", q.queued)
            .param("relation", q.relation.clone().unwrap_or_default());
        if let Some(w) = q.wait_s {
            f = f.param("maxS", w);
        }
        if let Some(t) = session_target(&q.session) {
            f = f.target(t);
        }
        out.push(f);
    }
    let waiting: Vec<f64> = d.roots.iter().flat_map(all_waits).collect();
    if let Some(max) = waiting.iter().copied().reduce(f64::max)
        && let Some(sev) = grade(max, th::LOCK_WAIT_S.0, th::LOCK_WAIT_S.1)
    {
        out.push(
            Finding::new("lockWait", sev)
                .member(member)
                .param("maxS", max)
                .param("count", waiting.len()),
        );
    }
    for c in &d.cycles {
        out.push(
            Finding::new("lockCycle", Severity::Info)
                .member(member)
                .param("count", c.len()),
        );
    }
    crate::findings::sort(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_blocker_and_ddl_queue_findings() {
        use crate::locktree::*;
        let rows = [
            LockRow {
                session: LockSession {
                    pid: 1,
                    backend_start_us: Some(1),
                    ..Default::default()
                },
                blocked_by: vec![],
                waiting: None,
            },
            LockRow {
                session: LockSession {
                    pid: 2,
                    backend_start_us: Some(2),
                    ..Default::default()
                },
                blocked_by: vec![1],
                waiting: Some(WaitingLock {
                    locktype: Some("relation".into()),
                    mode: Some("AccessExclusiveLock".into()),
                    relation: Some("shop.orders".into()),
                    wait_s: Some(12.0),
                    ..Default::default()
                }),
            },
            LockRow {
                session: LockSession {
                    pid: 3,
                    backend_start_us: Some(3),
                    ..Default::default()
                },
                blocked_by: vec![2],
                waiting: Some(WaitingLock {
                    locktype: Some("relation".into()),
                    mode: Some("AccessShareLock".into()),
                    wait_s: Some(400.0),
                    ..Default::default()
                }),
            },
        ];
        let f = evaluate(Some("pg-1"), &build(&rows));
        let got: Vec<(&str, Severity)> = f.iter().map(|x| (x.id.as_str(), x.severity)).collect();
        // Жертва в глубине цепочки (pid 3) ждёт 400 с — корень отвечает и за неё.
        assert_eq!(
            got,
            [
                ("ddlQueue", Severity::Critical),
                ("lockWait", Severity::Critical),
                ("rootBlocker", Severity::Critical)
            ]
        );
        let root = f.iter().find(|x| x.id == "rootBlocker").unwrap();
        assert_eq!(
            root.target,
            Some(FindingTarget::Session {
                pid: 1,
                backend_start_us: 1
            })
        );
        assert_eq!(
            (root.params["count"].as_u64(), root.params["maxS"].as_f64()),
            (Some(2), Some(400.0))
        );
        let ddl = f.iter().find(|x| x.id == "ddlQueue").unwrap();
        assert_eq!(ddl.params["relation"], "shop.orders");
    }
}
