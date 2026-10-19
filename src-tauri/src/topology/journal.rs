//! Журнал событий топологии (спец. P1 §6.4): разница двух опросов, до 500 событий в памяти.

use std::collections::VecDeque;

use serde::Serialize;

use crate::topology::model::{Role, Topology};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EventKind {
    LeaderChanged {
        from: Option<String>,
        to: Option<String>,
    },
    MemberAdded {
        name: String,
    },
    MemberRemoved {
        name: String,
    },
    MemberRoleChanged {
        name: String,
        from: Role,
        to: Role,
    },
    MemberStateChanged {
        name: String,
        from: Option<String>,
        to: Option<String>,
    },
    MemberUnreachable {
        name: String,
    },
    MemberReachable {
        name: String,
    },
    TimelineChanged {
        from: i64,
        to: i64,
    },
    PauseChanged {
        paused: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyEvent {
    pub at_ms: i64,
    #[serde(flatten)]
    pub event: EventKind,
}

/// События между двумя опросами; время событий — момент второго опроса.
/// `TimelineChanged` — только рост таймлайна кластера: его падение значит лишь, что узел с
/// наибольшим таймлайном стал недоступен (таймлайн PostgreSQL не убывает).
pub fn diff(prev: &Topology, next: &Topology) -> Vec<TopologyEvent> {
    let mut out = Vec::new();
    let mut push = |event: EventKind| {
        out.push(TopologyEvent {
            at_ms: next.observed_at_ms,
            event,
        })
    };

    let leader = |t: &Topology| t.leader().map(|m| m.name.clone());
    let (from, to) = (leader(prev), leader(next));
    if from != to {
        push(EventKind::LeaderChanged { from, to });
    }
    if let (Some(a), Some(b)) = (prev.cluster_timeline(), next.cluster_timeline())
        && b > a
    {
        push(EventKind::TimelineChanged { from: a, to: b });
    }
    if let (Some(a), Some(b)) = (prev.paused, next.paused)
        && a != b
    {
        push(EventKind::PauseChanged { paused: b });
    }
    for m in &next.members {
        let name = m.name.clone();
        let Some(old) = prev.member(&m.name) else {
            push(EventKind::MemberAdded { name });
            continue;
        };
        match (old.reachable, m.reachable) {
            (true, false) => {
                push(EventKind::MemberUnreachable { name });
                continue;
            }
            (false, true) => {
                push(EventKind::MemberReachable { name });
                continue;
            }
            (false, false) => continue,
            (true, true) => {}
        }
        if old.role != m.role {
            push(EventKind::MemberRoleChanged {
                name: name.clone(),
                from: old.role,
                to: m.role,
            });
        }
        if old.state != m.state {
            push(EventKind::MemberStateChanged {
                name,
                from: old.state.clone(),
                to: m.state.clone(),
            });
        }
    }
    for old in &prev.members {
        if next.member(&old.name).is_none() {
            push(EventKind::MemberRemoved {
                name: old.name.clone(),
            });
        }
    }
    out
}

/// Журнал событий: до `CAP` записей, в памяти.
#[derive(Debug, Default)]
pub struct Journal {
    events: VecDeque<TopologyEvent>,
    /// Наибольший таймлайн, уже записанный в журнал.
    high_water: Option<i64>,
}

impl Journal {
    pub const CAP: usize = 500;

    /// Записывает события и возвращает принятые: только их нужно передать в UI.
    /// `TimelineChanged`, у которого `to` не выше уже записанного таймлайна, отбрасывается:
    /// это повтор, когда узел с высоким таймлайном возвращается из недоступности.
    /// Остальные события проходят без изменений.
    pub fn extend(&mut self, events: &[TopologyEvent]) -> Vec<TopologyEvent> {
        let mut accepted = Vec::with_capacity(events.len());
        for e in events {
            if let EventKind::TimelineChanged { to, .. } = e.event {
                if self.high_water.is_some_and(|hw| to <= hw) {
                    continue;
                }
                self.high_water = Some(to);
            }
            if self.events.len() == Self::CAP {
                self.events.pop_front();
            }
            self.events.push_back(e.clone());
            accepted.push(e.clone());
        }
        accepted
    }

    /// Поднимает отметку таймлайна до `tl`, не создавая события: таймлайн, который опрос уже видел
    /// (первый опрос подключения, любой удачный опрос), не должен потом стать «новым» из-за узла,
    /// вернувшегося из недоступности. Меньшее значение и `None` отметку не меняют.
    pub fn observe_timeline(&mut self, tl: Option<i64>) {
        self.high_water = self.high_water.max(tl);
    }

    /// Новые сверху.
    pub fn newest_first(&self) -> Vec<TopologyEvent> {
        self.events.iter().rev().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::model::{member, topo};

    fn kinds(events: &[TopologyEvent]) -> Vec<&EventKind> {
        events.iter().map(|e| &e.event).collect()
    }

    #[test]
    fn switchover_gives_leader_and_role_changes() {
        let prev = topo(vec![member("a", Role::Primary), member("b", Role::Replica)]);
        let mut next = topo(vec![member("a", Role::Replica), member("b", Role::Primary)]);
        next.observed_at_ms = 2_000;
        next.members[1].timeline = Some(2);
        next.members[0].timeline = Some(2);
        let ev = diff(&prev, &next);
        assert!(ev.iter().all(|e| e.at_ms == 2_000));
        let k = kinds(&ev);
        assert!(k.contains(&&EventKind::LeaderChanged {
            from: Some("a".into()),
            to: Some("b".into())
        }));
        assert!(k.contains(&&EventKind::TimelineChanged { from: 1, to: 2 }));
        assert!(k.contains(&&EventKind::MemberRoleChanged {
            name: "a".into(),
            from: Role::Primary,
            to: Role::Replica
        }));
        assert!(k.contains(&&EventKind::MemberRoleChanged {
            name: "b".into(),
            from: Role::Replica,
            to: Role::Primary
        }));
    }

    #[test]
    fn unreachable_member_does_not_produce_role_noise() {
        let prev = topo(vec![member("a", Role::Primary), member("b", Role::Replica)]);
        let mut next = prev.clone();
        next.members[1].reachable = false;
        next.members[1].role = Role::Unknown;
        next.members[1].state = None;
        assert_eq!(
            kinds(&diff(&prev, &next)),
            vec![&EventKind::MemberUnreachable { name: "b".into() }]
        );
        assert_eq!(
            kinds(&diff(&next, &prev)),
            vec![&EventKind::MemberReachable { name: "b".into() },]
        );
    }

    #[test]
    fn added_removed_state_and_pause() {
        let prev = topo(vec![member("a", Role::Primary), member("b", Role::Replica)]);
        let mut next = topo(vec![member("a", Role::Primary), member("c", Role::Replica)]);
        next.members[0].state = Some("restarting".into());
        next.paused = Some(true);
        let k = kinds(&diff(&prev, &next))
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        assert!(k.contains(&EventKind::MemberAdded { name: "c".into() }));
        assert!(k.contains(&EventKind::MemberRemoved { name: "b".into() }));
        assert!(k.contains(&EventKind::MemberStateChanged {
            name: "a".into(),
            from: Some("running".into()),
            to: Some("restarting".into())
        }));
        assert!(k.contains(&EventKind::PauseChanged { paused: true }));
        assert!(diff(&prev, &prev).is_empty());
    }

    fn has_timeline_change(events: &[TopologyEvent]) -> bool {
        events
            .iter()
            .any(|e| matches!(e.event, EventKind::TimelineChanged { .. }))
    }

    #[test]
    fn timeline_change_survives_leaderless_window() {
        // Ключ лидера истёк: на промежуточном опросе лидера нет, старый primary недоступен.
        let prev = topo(vec![member("a", Role::Primary), member("b", Role::Replica)]);
        let mut mid = prev.clone();
        mid.observed_at_ms = 2_000;
        mid.members[0].reachable = false;
        mid.members[0].role = Role::Unknown;
        mid.members[0].state = None;
        let mut next = mid.clone();
        next.observed_at_ms = 3_000;
        next.members[1].role = Role::Primary;
        next.members[1].timeline = Some(2);

        assert!(!has_timeline_change(&diff(&prev, &mid)));
        let ev = diff(&mid, &next);
        assert!(kinds(&ev).contains(&&EventKind::TimelineChanged { from: 1, to: 2 }));
    }

    #[test]
    fn stale_timeline_of_unreachable_member_is_ignored() {
        let prev = topo(vec![member("a", Role::Primary), member("b", Role::Replica)]);
        let mut next = prev.clone();
        next.observed_at_ms = 2_000;
        next.members[0].reachable = false;
        next.members[0].role = Role::Unknown;
        next.members[0].state = None;
        next.members[0].timeline = Some(5);

        assert_eq!(next.cluster_timeline(), Some(1));
        assert!(!has_timeline_change(&diff(&prev, &next)));
    }

    #[test]
    fn timeline_drop_is_not_reported() {
        // Единственный узел с TL 2 стал недоступен, реплика всё ещё на TL 1: это не откат таймлайна.
        let mut prev = topo(vec![member("a", Role::Replica), member("b", Role::Primary)]);
        prev.members[1].timeline = Some(2);
        let mut next = prev.clone();
        next.observed_at_ms = 2_000;
        next.members[1].reachable = false;
        next.members[1].role = Role::Unknown;
        next.members[1].state = None;

        assert_eq!(prev.cluster_timeline(), Some(2));
        assert_eq!(next.cluster_timeline(), Some(1));
        assert!(!has_timeline_change(&diff(&prev, &next)));
    }

    #[test]
    fn journal_keeps_500_newest_first() {
        let mut j = Journal::default();
        let events: Vec<TopologyEvent> = (0..510)
            .map(|i| TopologyEvent {
                at_ms: i,
                event: EventKind::PauseChanged { paused: i % 2 == 0 },
            })
            .collect();
        j.extend(&events);
        let list = j.newest_first();
        assert_eq!(list.len(), Journal::CAP);
        assert_eq!(list[0].at_ms, 509);
        assert_eq!(list.last().unwrap().at_ms, 10);
    }

    #[test]
    fn journal_drops_repeated_timeline_change() {
        let tl = |at_ms, from, to| TopologyEvent {
            at_ms,
            event: EventKind::TimelineChanged { from, to },
        };
        let mut j = Journal::default();
        assert_eq!(j.extend(&[tl(1, 1, 2)]), vec![tl(1, 1, 2)]);
        // Узел с TL 2 вернулся после недоступности: diff снова даёт 1→2, журнал его не принимает.
        assert!(j.extend(&[tl(2, 1, 2)]).is_empty());
        assert_eq!(j.extend(&[tl(3, 2, 3)]), vec![tl(3, 2, 3)]);
        let at: Vec<i64> = j.newest_first().iter().map(|e| e.at_ms).collect();
        assert_eq!(at, vec![3, 1]);
    }

    #[test]
    fn observe_timeline_raises_the_mark_without_events() {
        let tl = |at_ms, from, to| TopologyEvent {
            at_ms,
            event: EventKind::TimelineChanged { from, to },
        };
        let mut j = Journal::default();
        j.observe_timeline(Some(2));
        assert!(j.newest_first().is_empty(), "событие не создаётся");
        // Узел с TL 2 уже был виден при подключении: когда он вернётся, 1→2 — не новость.
        assert!(j.extend(&[tl(1, 1, 2)]).is_empty());
        // None и меньшее значение отметку не опускают.
        j.observe_timeline(None);
        j.observe_timeline(Some(1));
        assert!(j.extend(&[tl(2, 1, 2)]).is_empty());
        assert_eq!(j.extend(&[tl(3, 2, 3)]), vec![tl(3, 2, 3)]);
        // Рост таймлайна, замеченный без события (например, при опросе без `diff`), отметку поднимает.
        j.observe_timeline(Some(5));
        assert!(j.extend(&[tl(4, 3, 4)]).is_empty());
        assert_eq!(j.extend(&[tl(5, 5, 6)]), vec![tl(5, 5, 6)]);
    }

    #[test]
    fn event_json_is_flat() {
        let e = TopologyEvent {
            at_ms: 5,
            event: EventKind::LeaderChanged {
                from: None,
                to: Some("b".into()),
            },
        };
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({"atMs": 5, "kind": "leaderChanged", "from": null, "to": "b"})
        );
    }
}
