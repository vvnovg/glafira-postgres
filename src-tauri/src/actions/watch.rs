//! Наблюдение после действия Patroni (спец. P2 §8.6): ускоренный опрос топологии и шаги до «готово».

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::task::AbortHandle;

use crate::actions::patroni::PatroniAction;
use crate::session::SessionKind;
use crate::state::AppState;
use crate::topology::model::{Role, Topology};
use crate::topology::runtime::Runtimes;

/// Опрос топологии во время наблюдения (вместо 10 с).
pub const BOOST_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WatchStep {
    Accepted,
    LeaderStepped,
    NewLeaderRunning,
    TimelineIncreased,
    ReplicasStreaming,
    Restarted,
    MemberRunning,
    PendingCleared,
    PauseApplied,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WatchStatus {
    Running,
    Done,
    Failed,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchEvent {
    pub step: WatchStep,
    pub status: WatchStatus,
    pub at_ms: i64,
    pub detail: Option<String>,
}

/// Куда уходят события наблюдения; `false` — канал закрыт, наблюдение заканчивается.
pub trait WatchSink: Send + Sync {
    fn send(&self, e: WatchEvent) -> bool;
}

#[derive(Debug, Clone, PartialEq)]
pub enum WatchPlan {
    Switch {
        old_leader: String,
        candidate: Option<String>,
        timeline: Option<i64>,
    },
    Restart {
        member: String,
        start_before: Option<String>,
        only_pending: bool,
    },
    Pause {
        paused: bool,
    },
}

impl WatchPlan {
    /// Наблюдается то, что происходит сейчас: запланированные действия и отмены — нет (обзор покажет баннер).
    pub fn for_action(
        a: &PatroniAction,
        before: &Topology,
        restart_start: Option<String>,
    ) -> Option<WatchPlan> {
        match a {
            PatroniAction::Switchover {
                candidate,
                scheduled_at: None,
            } => Some(WatchPlan::Switch {
                old_leader: before.leader()?.name.clone(),
                candidate: candidate.clone(),
                timeline: before.cluster_timeline(),
            }),
            PatroniAction::Failover { candidate } => Some(WatchPlan::Switch {
                old_leader: before.leader().map(|m| m.name.clone()).unwrap_or_default(),
                candidate: Some(candidate.clone()),
                timeline: before.cluster_timeline(),
            }),
            PatroniAction::Restart {
                member,
                only_pending,
                scheduled_at: None,
            } => Some(WatchPlan::Restart {
                member: member.clone(),
                start_before: restart_start,
                only_pending: *only_pending,
            }),
            PatroniAction::Pause { paused } => Some(WatchPlan::Pause { paused: *paused }),
            _ => None,
        }
    }

    pub fn steps(&self) -> Vec<WatchStep> {
        use WatchStep::*;
        match self {
            WatchPlan::Switch { .. } => vec![
                Accepted,
                LeaderStepped,
                NewLeaderRunning,
                TimelineIncreased,
                ReplicasStreaming,
                Done,
            ],
            WatchPlan::Restart {
                only_pending: true, ..
            } => vec![Accepted, Restarted, MemberRunning, PendingCleared, Done],
            WatchPlan::Restart { .. } => vec![Accepted, Restarted, MemberRunning, Done],
            WatchPlan::Pause { .. } => vec![Accepted, PauseApplied, Done],
        }
    }

    pub fn deadline(&self) -> Duration {
        match self {
            WatchPlan::Switch { .. } => Duration::from_secs(120),
            WatchPlan::Restart { .. } => Duration::from_secs(300),
            WatchPlan::Pause { .. } => Duration::from_secs(30),
        }
    }
}

fn is_leader(role: Role) -> bool {
    matches!(role, Role::Primary | Role::StandbyLeader)
}

fn running_or_streaming(state: Option<&str>) -> bool {
    matches!(state, Some("running") | Some("streaming"))
}

/// Выполнено ли условие шага на текущей топологии.
fn reached(plan: &WatchPlan, step: WatchStep, t: &Topology, start_now: Option<&str>) -> bool {
    use WatchStep::*;
    match (plan, step) {
        (_, Accepted) | (_, Done) => true,
        (WatchPlan::Switch { old_leader, .. }, LeaderStepped) => t
            .member(old_leader)
            .is_none_or(|m| !is_leader(m.role) || !m.reachable),
        (
            WatchPlan::Switch {
                old_leader,
                candidate,
                ..
            },
            NewLeaderRunning,
        ) => t.leader().is_some_and(|l| {
            &l.name != old_leader
                && l.reachable
                && l.state.as_deref() == Some("running")
                && candidate.as_ref().is_none_or(|c| c == &l.name)
        }),
        (WatchPlan::Switch { timeline, .. }, TimelineIncreased) => {
            match (timeline, t.cluster_timeline()) {
                (Some(before), Some(now)) => now > *before,
                _ => true,
            }
        }
        (WatchPlan::Switch { .. }, ReplicasStreaming) => t
            .members
            .iter()
            .filter(|m| m.reachable && !is_leader(m.role))
            .all(|m| running_or_streaming(m.state.as_deref())),
        (WatchPlan::Restart { start_before, .. }, Restarted) => {
            start_now.is_some() && start_now != start_before.as_deref()
        }
        (WatchPlan::Restart { member, .. }, MemberRunning) => t
            .member(member)
            .is_some_and(|m| m.reachable && running_or_streaming(m.state.as_deref())),
        (WatchPlan::Restart { member, .. }, PendingCleared) => {
            t.member(member).is_some_and(|m| !m.pending_restart)
        }
        (WatchPlan::Pause { paused }, PauseApplied) => t.paused.unwrap_or(false) == *paused,
        _ => false,
    }
}

/// Последний шаг, до которого дошло действие: шаги проходятся по порядку, первый невыполненный останавливает.
pub fn progress(plan: &WatchPlan, t: &Topology, start_now: Option<&str>) -> WatchStep {
    let mut last = WatchStep::Accepted;
    for step in plan.steps() {
        if !reached(plan, step, t, start_now) {
            break;
        }
        last = step;
    }
    last
}

fn now_ms() -> i64 {
    crate::actions::journal::now_ms()
}

/// Открывает сессию наблюдения: ускоренный опрос, шаги по каждому опросу, срок; итог — id сессии для `stop_session`.
/// Остановка сессии (панель закрыта, подключение закрыто) прерывает задачу и снимает ускорение. Нужен рантайм tokio.
pub fn start(st: &AppState, id: &str, plan: WatchPlan, sink: Arc<dyn WatchSink>) -> String {
    let session_id = uuid::Uuid::new_v4().to_string();
    let runtimes = st.runtimes.clone();
    let live = st.live.clone();
    let connections = st.connections.clone();
    let sessions = st.sessions.clone();
    let (conn, sid) = (id.to_string(), session_id.clone());
    let deadline = Instant::now() + plan.deadline();
    runtimes.boost(id, BOOST_INTERVAL, deadline);
    let task = tokio::spawn(async move {
        let mut last = WatchStep::Accepted;
        sink.send(WatchEvent {
            step: last,
            status: WatchStatus::Running,
            at_ms: now_ms(),
            detail: None,
        });
        while Instant::now() < deadline {
            tokio::time::sleep(BOOST_INTERVAL).await;
            let Some(t) = runtimes.view(&conn).topology else {
                continue;
            };
            let start_now = match &plan {
                WatchPlan::Restart { member, .. } => match connections.get(&conn) {
                    Ok(p) => match live.http_for(&p).await {
                        Ok(http) => {
                            crate::actions::patroni::postmaster_start(http.as_ref(), &t, member)
                                .await
                        }
                        Err(_) => None,
                    },
                    Err(_) => None,
                },
                _ => None,
            };
            let step = progress(&plan, &t, start_now.as_deref());
            if step > last {
                last = step;
                let status = if step == WatchStep::Done {
                    WatchStatus::Done
                } else {
                    WatchStatus::Running
                };
                if !sink.send(WatchEvent {
                    step,
                    status,
                    at_ms: now_ms(),
                    detail: None,
                }) {
                    break;
                }
                if step == WatchStep::Done {
                    break;
                }
            }
        }
        if last != WatchStep::Done {
            sink.send(WatchEvent {
                step: last,
                status: WatchStatus::TimedOut,
                at_ms: now_ms(),
                detail: None,
            });
        }
        runtimes.unboost(&conn);
        sessions.forget(&sid);
    });
    // Задача не закончится раньше вставки сессии: первым она ждёт интервал опроса (2 с), а срок не меньше 30 с.
    st.sessions.insert(
        session_id.clone(),
        id,
        SessionKind::Watch,
        stop_watch(task.abort_handle(), &st.runtimes, id),
    );
    session_id
}

/// Остановка сессии наблюдения: прерывание задачи и снятие ускорения — быстро и без `await`. Вызывается и из
/// слушателя закрытия под `entries` `Runtimes` (`disconnect`, переоткрытие): поэтому `Runtimes::unboost` берёт
/// только свой мьютекс ускорений, а не `entries`, — иначе тот же поток захватил бы `entries` повторно и завис.
/// `Runtimes` — слабой ссылкой: реестр сессий сам слушатель закрытия `Runtimes`, сильная дала бы цикл ссылок.
pub(crate) fn stop_watch(
    abort: AbortHandle,
    runtimes: &Arc<Runtimes>,
    connection: &str,
) -> Box<dyn FnOnce() + Send> {
    let (runtimes, connection) = (Arc::downgrade(runtimes), connection.to_string());
    Box::new(move || {
        abort.abort();
        if let Some(rt) = runtimes.upgrade() {
            rt.unboost(&connection);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::model::{Member, Role, Topology};

    fn m(name: &str, role: Role, state: &str, tl: i64) -> Member {
        let mut m = Member::new(name, name, 5432);
        m.role = role;
        m.state = Some(state.into());
        m.timeline = Some(tl);
        m.reachable = true;
        m
    }

    fn topo(members: Vec<Member>) -> Topology {
        let mut t = Topology::empty_for_tests();
        t.members = members;
        t
    }

    #[test]
    fn switchover_steps_follow_the_cluster() {
        let plan = WatchPlan::Switch {
            old_leader: "pg-1".into(),
            candidate: Some("pg-2".into()),
            timeline: Some(2),
        };
        let before = topo(vec![
            m("pg-1", Role::Primary, "running", 2),
            m("pg-2", Role::Replica, "streaming", 2),
        ]);
        assert_eq!(progress(&plan, &before, None), WatchStep::Accepted);
        let stepped = topo(vec![
            m("pg-1", Role::Replica, "stopped", 2),
            m("pg-2", Role::Replica, "streaming", 2),
        ]);
        assert_eq!(progress(&plan, &stepped, None), WatchStep::LeaderStepped);
        let promoted = topo(vec![
            m("pg-1", Role::Replica, "stopped", 2),
            m("pg-2", Role::Primary, "running", 3),
        ]);
        assert_eq!(
            progress(&plan, &promoted, None),
            WatchStep::TimelineIncreased
        );
        let done = topo(vec![
            m("pg-1", Role::Replica, "streaming", 3),
            m("pg-2", Role::Primary, "running", 3),
        ]);
        assert_eq!(progress(&plan, &done, None), WatchStep::Done);
        // Лидером стал не кандидат — дальше «сложил полномочия» не идём.
        let other = topo(vec![
            m("pg-1", Role::Replica, "streaming", 3),
            m("pg-3", Role::Primary, "running", 3),
            m("pg-2", Role::Replica, "streaming", 3),
        ]);
        assert_eq!(progress(&plan, &other, None), WatchStep::LeaderStepped);
    }

    #[test]
    fn restart_needs_a_new_postmaster_and_cleared_pending() {
        let plan = WatchPlan::Restart {
            member: "pg-2".into(),
            start_before: Some("t0".into()),
            only_pending: true,
        };
        let mut r = m("pg-2", Role::Replica, "streaming", 2);
        r.pending_restart = true;
        let t = topo(vec![m("pg-1", Role::Primary, "running", 2), r.clone()]);
        assert_eq!(progress(&plan, &t, Some("t0")), WatchStep::Accepted);
        assert_eq!(progress(&plan, &t, Some("t1")), WatchStep::MemberRunning);
        r.pending_restart = false;
        let t = topo(vec![m("pg-1", Role::Primary, "running", 2), r]);
        assert_eq!(progress(&plan, &t, Some("t1")), WatchStep::Done);
        let plain = WatchPlan::Restart {
            member: "pg-2".into(),
            start_before: Some("t0".into()),
            only_pending: false,
        };
        assert!(!plain.steps().contains(&WatchStep::PendingCleared));
    }

    #[test]
    fn pause_is_done_when_the_topology_agrees() {
        let plan = WatchPlan::Pause { paused: true };
        let mut t = topo(vec![]);
        t.paused = Some(false);
        assert_eq!(progress(&plan, &t, None), WatchStep::Accepted);
        t.paused = Some(true);
        assert_eq!(progress(&plan, &t, None), WatchStep::Done);
    }

    #[test]
    fn scheduled_actions_and_cancels_are_not_watched() {
        let t = topo(vec![m("pg-1", Role::Primary, "running", 2)]);
        let sched = PatroniAction::Switchover {
            candidate: None,
            scheduled_at: Some("2030-01-01T00:00:00Z".into()),
        };
        assert!(WatchPlan::for_action(&sched, &t, None).is_none());
        assert!(WatchPlan::for_action(&PatroniAction::CancelSwitchover, &t, None).is_none());
        let now = PatroniAction::Switchover {
            candidate: None,
            scheduled_at: None,
        };
        assert_eq!(
            WatchPlan::for_action(&now, &t, None).unwrap().deadline(),
            Duration::from_secs(120)
        );
        let restart = PatroniAction::Restart {
            member: "pg-1".into(),
            only_pending: false,
            scheduled_at: None,
        };
        assert_eq!(
            WatchPlan::for_action(&restart, &t, Some("t0".into()))
                .unwrap()
                .deadline(),
            Duration::from_secs(300)
        );
        assert_eq!(
            WatchPlan::for_action(&PatroniAction::Pause { paused: true }, &t, None)
                .unwrap()
                .deadline(),
            Duration::from_secs(30)
        );
    }

    #[derive(Default)]
    struct Events(std::sync::Mutex<Vec<WatchEvent>>);

    impl WatchSink for Events {
        fn send(&self, e: WatchEvent) -> bool {
            self.0.lock().unwrap().push(e);
            true
        }
    }

    /// Наблюдение — сессия: первым идёт «принято», `stop_session` прерывает задачу (событий больше нет), второй
    /// раз сессии уже нет. Подключение не открыто — ускорять нечего, наблюдение просто ждёт топологию.
    #[tokio::test]
    async fn the_watch_is_a_session_that_stop_session_ends() {
        let dir = tempfile::tempdir().unwrap();
        let st =
            AppState::new(dir.path(), Arc::new(crate::secrets::MemoryStore::default())).unwrap();
        let events = Arc::new(Events::default());
        let sid = start(&st, "c", WatchPlan::Pause { paused: true }, events.clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        while events.0.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "нет первого события");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let first = events.0.lock().unwrap().clone();
        assert_eq!(
            (first.len(), first[0].step, first[0].status),
            (1, WatchStep::Accepted, WatchStatus::Running)
        );
        assert!(st.sessions.stop(&sid), "сессия наблюдения зарегистрирована");
        assert!(!st.sessions.stop(&sid), "второй раз — сессии нет");
    }

    #[test]
    fn event_json() {
        let e = WatchEvent {
            step: WatchStep::NewLeaderRunning,
            status: WatchStatus::Running,
            at_ms: 5,
            detail: None,
        };
        assert_eq!(
            serde_json::to_value(e).unwrap(),
            serde_json::json!({"step": "newLeaderRunning", "status": "running", "atMs": 5, "detail": null})
        );
    }
}
