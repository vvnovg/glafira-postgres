//! Блокировки и DDL в очереди (спец. P2 §5.4): находки по лесу `locktree`.

use crate::findings::{
    Finding, FindingTarget, SessionActionLink, Severity, grade, thresholds as th,
};
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

/// Ожидания всех ждущих в поддереве, включая сам узел: по элементу на каждого, у кого задан `lock`; `None` — время
/// ожидания неизвестно.
fn all_waits(n: &LockNode) -> Vec<Option<f64>> {
    n.lock
        .as_ref()
        .map(|l| l.wait_s)
        .into_iter()
        .chain(n.children.iter().flat_map(all_waits))
        .collect()
}

/// Находки инструмента «Блокировки» на узле `member`. `rootBlocker` и `ddlQueue` — по находке на корень и на
/// элемент очереди (уточнение 2 плана P2a), `lockWait` и `lockCycle` — по узлу. `rootBlocker` — только для корня,
/// который сам не ждёт: ждущий корень (его блокирующий не виден, например подготовленная транзакция) завершать
/// нечего, его очередь учитывает `lockWait`.
pub fn evaluate(member: Option<&str>, d: &LocksData) -> Vec<Finding> {
    let mut out = Vec::new();
    for root in d.roots.iter().filter(|r| r.lock.is_none()) {
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
                // Завершать можно только сессию с известным временем запуска: иначе PID мог достаться другой.
                if let FindingTarget::Session {
                    pid,
                    backend_start_us,
                } = &t
                {
                    f = f.link_action(
                        SessionActionLink::TerminateRootBlocker {
                            pid: *pid,
                            backend_start_us: *backend_start_us,
                        },
                        member,
                    );
                }
                f = f.target(t);
            }
            out.push(f);
        }
    }
    for q in &d.ddl_queue {
        let mut f = Finding::new("ddlQueue", Severity::Critical)
            .member(member)
            .param("count", q.queued);
        // Имя отношения известно только для БД соединения; иначе интерфейс собирает «oid N в БД X».
        if let Some(name) = &q.relation {
            f = f.param("relation", name.clone());
        } else {
            if let Some(oid) = q.relation_oid {
                f = f.param("relationOid", oid);
            }
            if let Some(db) = &q.relation_database {
                f = f.param("relationDatabase", db.clone());
            }
        }
        if let Some(w) = q.wait_s {
            f = f.param("maxS", w);
        }
        if let Some(t) = session_target(&q.session) {
            f = f.target(t);
        }
        out.push(f);
    }
    // `count` — все ждущие сессии леса, `maxS` — самое долгое из известных ожиданий.
    let waiting: Vec<Option<f64>> = d.roots.iter().flat_map(all_waits).collect();
    if let Some(max) = waiting.iter().copied().flatten().reduce(f64::max)
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
    use crate::findings::{FindingLink, SessionActionLink};
    use crate::locktree::build;

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
        // Корень с известным временем запуска предлагает завершение; ссылка на инструмент остаётся.
        assert!(root.links.iter().any(|l| matches!(
            l,
            FindingLink::Action {
                action: SessionActionLink::TerminateRootBlocker { .. },
                ..
            }
        )));
        assert!(root.links.contains(&FindingLink::Action {
            action: SessionActionLink::TerminateRootBlocker {
                pid: 1,
                backend_start_us: 1
            },
            member: Some("pg-1".into()),
        }));
        assert!(root.links.contains(&FindingLink::OpenTool {
            tool: "sessions".into(),
            member: Some("pg-1".into())
        }));
        // Остальные находки действий не предлагают.
        assert!(f.iter().filter(|x| x.id != "rootBlocker").all(|x| {
            x.links
                .iter()
                .all(|l| !matches!(l, FindingLink::Action { .. }))
        }));
    }

    #[test]
    fn a_root_without_backend_start_gets_no_action_link() {
        // Без времени запуска PID может принадлежать уже другой сессии: завершать нельзя.
        let mut h = holder(1);
        h.session.backend_start_us = None;
        let f = evaluate(
            Some("pg-1"),
            &build(&[h, waiter(2, &[1], "AccessShareLock", Some(400.0))]),
        );
        let root = f.iter().find(|x| x.id == "rootBlocker").unwrap();
        assert!(root.target.is_none());
        assert_eq!(
            root.links,
            [FindingLink::OpenTool {
                tool: "sessions".into(),
                member: Some("pg-1".into())
            }]
        );
    }

    use crate::locktree::{LockRow, WaitingLock};

    fn sess(pid: i32) -> LockSession {
        LockSession {
            pid,
            backend_start_us: Some(pid as i64),
            ..Default::default()
        }
    }

    fn holder(pid: i32) -> LockRow {
        LockRow {
            session: sess(pid),
            blocked_by: vec![],
            waiting: None,
        }
    }

    fn waiter(pid: i32, blocked_by: &[i32], mode: &str, wait_s: Option<f64>) -> LockRow {
        LockRow {
            session: sess(pid),
            blocked_by: blocked_by.to_vec(),
            waiting: Some(WaitingLock {
                locktype: Some("relation".into()),
                mode: Some(mode.into()),
                relation: Some("shop.orders".into()),
                wait_s,
                ..Default::default()
            }),
        }
    }

    #[test]
    fn a_waiting_root_is_no_root_blocker_but_its_queue_is_counted_in_lock_wait() {
        // 2 ждёт подготовленную транзакцию (pid 0), 3 стоит за 2.
        let rows = [
            waiter(2, &[0], "AccessShareLock", Some(40.0)),
            waiter(3, &[2], "AccessShareLock", Some(10.0)),
        ];
        let f = evaluate(None, &build(&rows));
        assert!(
            f.iter().all(|x| x.id != "rootBlocker"),
            "ждущий корень завершать нечего: {f:?}"
        );
        let wait = f.iter().find(|x| x.id == "lockWait").expect("lockWait");
        assert_eq!(
            (
                wait.severity,
                wait.params["count"].as_u64(),
                wait.params["maxS"].as_f64()
            ),
            (Severity::Warning, Some(2), Some(40.0))
        );
    }

    #[test]
    fn lock_wait_counts_every_waiter_even_without_a_known_wait() {
        let rows = [
            holder(1),
            waiter(2, &[1], "AccessShareLock", None),
            waiter(3, &[1], "AccessShareLock", Some(40.0)),
        ];
        let f = evaluate(None, &build(&rows));
        let wait = f.iter().find(|x| x.id == "lockWait").expect("lockWait");
        assert_eq!(
            (wait.params["count"].as_u64(), wait.params["maxS"].as_f64()),
            (Some(2), Some(40.0))
        );
    }

    #[test]
    fn no_lock_wait_when_no_waiter_has_a_known_wait() {
        let rows = [holder(1), waiter(2, &[1], "AccessShareLock", None)];
        let f = evaluate(None, &build(&rows));
        assert!(f.iter().all(|x| x.id != "lockWait"), "{f:?}");
    }

    #[test]
    fn ddl_queue_on_a_table_of_another_database_carries_oid_and_database() {
        let mut ddl = waiter(2, &[1], "AccessExclusiveLock", Some(12.0));
        if let Some(l) = ddl.waiting.as_mut() {
            l.relation = None;
            l.relation_oid = Some(16_450);
            l.relation_database = Some("orders".into());
        }
        let rows = [
            holder(1),
            ddl,
            waiter(3, &[2], "AccessShareLock", Some(5.0)),
        ];
        let f = evaluate(None, &build(&rows));
        let q = f.iter().find(|x| x.id == "ddlQueue").expect("ddlQueue");
        assert!(!q.params.contains_key("relation"), "{:?}", q.params);
        assert_eq!(q.params["relationOid"], 16_450);
        assert_eq!(q.params["relationDatabase"], "orders");
    }
}
