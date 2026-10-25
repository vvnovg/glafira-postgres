//! Наблюдение после действия Patroni (спец. P2 §8.6): ускоренный опрос топологии и шаги до «готово».

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::task::AbortHandle;

use crate::actions::patroni::PatroniAction;
use crate::error::AppError;
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
    /// Причина `failed` (например, подключение закрыто).
    pub detail: Option<AppError>,
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
        /// `postmaster_start_time` участника до действия; `None` — не прочитался.
        start_before: Option<String>,
        only_pending: bool,
        /// Когда ушёл запрос (мс эпохи): без `start_before` перезапуск подтверждает время запуска позже него.
        sent_at_ms: i64,
        /// Наблюдение видело участника остановленным или недоступным (`observe`) либо Patroni подтвердил restart
        /// ответом 2xx (`confirm_restart`): без `start_before` перезапуск подтверждается только так.
        seen_down: bool,
    },
    Pause {
        paused: bool,
    },
}

impl WatchPlan {
    /// Наблюдается то, что происходит сейчас: запланированные действия и отмены — нет (обзор покажет баннер).
    /// `restart_start` — `postmaster_start_time` участника restart до действия, `sent_at_ms` — когда ушёл запрос.
    pub fn for_action(
        a: &PatroniAction,
        before: &Topology,
        restart_start: Option<String>,
        sent_at_ms: i64,
    ) -> Option<WatchPlan> {
        match a {
            PatroniAction::Switchover {
                candidate,
                scheduled_at: None,
                ..
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
                sent_at_ms,
                seen_down: false,
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

    /// Patroni ответил 2xx на restart «сейчас»: он отвечает уже после запуска PostgreSQL, и простой, который
    /// наблюдение увидело бы, остался до ответа. Ответ сам подтверждает простой (`seen_down`). При неизвестном исходе
    /// (таймаут, обрыв) это не вызывается: там простой ещё может быть виден, и правило остаётся строгим.
    pub fn confirm_restart(&mut self) {
        if let WatchPlan::Restart { seen_down, .. } = self {
            *seen_down = true;
        }
    }

    /// Запоминает увиденное на очередном опросе: участник restart остановлен, недоступен или пропал из топологии.
    pub fn observe(&mut self, t: &Topology) {
        if let WatchPlan::Restart {
            member, seen_down, ..
        } = self
            && t.member(member)
                .is_none_or(|m| !m.reachable || !running_or_streaming(m.state.as_deref()))
        {
            *seen_down = true;
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
        // Время запуска до действия известно — подтверждает другое время запуска.
        (
            WatchPlan::Restart {
                start_before: Some(before),
                ..
            },
            Restarted,
        ) => start_now.is_some_and(|now| now != before),
        // Неизвестно: просто прочитанное время ничего не значит, и одним сравнением времён не обойтись — часы
        // сервера могут расходиться с часами приложения. Нужен простой участника, который наблюдение видело, а
        // разборчивое время запуска ещё и должно быть позже отправки запроса. Быстрый перезапуск между опросами
        // так не подтвердится — наблюдение кончится по сроку, а не ложным «готово».
        (
            WatchPlan::Restart {
                start_before: None,
                sent_at_ms,
                seen_down,
                ..
            },
            Restarted,
        ) => {
            *seen_down
                && start_now
                    .and_then(start_time_ms)
                    .is_none_or(|started| started > *sent_at_ms)
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

/// `postmaster_start_time` Patroni (`2026-10-25 09:52:53.829+00:00`) в миллисекундах эпохи; другой вид — `None`.
fn start_time_ms(s: &str) -> Option<i64> {
    let rfc3339 = s.trim().replacen(' ', "T", 1);
    let t = time::OffsetDateTime::parse(&rfc3339, &time::format_description::well_known::Rfc3339)
        .ok()?;
    i64::try_from(t.unix_timestamp_nanos() / 1_000_000).ok()
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
///
/// Первое событие («принято») уходит отсюда, до задачи: её события идут не раньше, чем через интервал опроса.
/// Сессия вносится в реестр до ускорения: подключение, закрытое после этого, остановит её своим слушателем, а
/// закрытое раньше (или не открытое) `boost` покажет. Закрытое и открытое заново между вставкой и ускорением
/// `boost` не покажет, но слушатель закрытия уже снял сессию — это видно по реестру, и ускорение, легшее на новое
/// открытие, снимается. В обоих случаях наблюдение кончается сразу событием `failed`, а id остаётся id
/// закончившегося наблюдения.
pub fn start(st: &AppState, id: &str, plan: WatchPlan, sink: Arc<dyn WatchSink>) -> String {
    let session_id = uuid::Uuid::new_v4().to_string();
    let runtimes = st.runtimes.clone();
    let live = st.live.clone();
    let connections = st.connections.clone();
    let sessions = st.sessions.clone();
    let (conn, sid) = (id.to_string(), session_id.clone());
    let deadline = Instant::now() + plan.deadline();
    sink.send(WatchEvent {
        step: WatchStep::Accepted,
        status: WatchStatus::Running,
        at_ms: now_ms(),
        detail: None,
    });
    let task_sink = sink.clone();
    let task = tokio::spawn(async move {
        let sink = task_sink;
        let mut plan = plan;
        let mut last = WatchStep::Accepted;
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
            plan.observe(&t);
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
        runtimes.unboost(&conn, &sid);
        sessions.forget(&sid);
    });
    st.sessions.insert(
        session_id.clone(),
        id,
        SessionKind::Watch,
        stop_watch(task.abort_handle(), &st.runtimes, id, &session_id),
    );
    let boosted = st.runtimes.boost(id, &session_id, BOOST_INTERVAL, deadline);
    if !boosted || !st.sessions.contains(&session_id) {
        // Задача прервана до первого своего события (оно — не раньше интервала опроса); сессии могло уже не быть,
        // если её снял слушатель закрытия, — тогда ускорение снимается здесь.
        st.sessions.stop(&session_id);
        st.runtimes.unboost(id, &session_id);
        sink.send(WatchEvent {
            step: WatchStep::Accepted,
            status: WatchStatus::Failed,
            at_ms: now_ms(),
            detail: Some(crate::topology::runtime::not_connected()),
        });
    }
    session_id
}

/// Остановка сессии наблюдения `session_id`: прерывание задачи и снятие её ускорения (ускорения других
/// наблюдений подключения остаются) — быстро и без `await`. Вызывается и из
/// слушателя закрытия под `entries` `Runtimes` (`disconnect`, переоткрытие): поэтому `Runtimes::unboost` берёт
/// только свой мьютекс ускорений, а не `entries`, — иначе тот же поток захватил бы `entries` повторно и завис.
/// `Runtimes` — слабой ссылкой: реестр сессий сам слушатель закрытия `Runtimes`, сильная дала бы цикл ссылок.
pub(crate) fn stop_watch(
    abort: AbortHandle,
    runtimes: &Arc<Runtimes>,
    connection: &str,
    session_id: &str,
) -> Box<dyn FnOnce() + Send> {
    let (runtimes, connection, session_id) = (
        Arc::downgrade(runtimes),
        connection.to_string(),
        session_id.to_string(),
    );
    Box::new(move || {
        abort.abort();
        if let Some(rt) = runtimes.upgrade() {
            rt.unboost(&connection, &session_id);
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
            sent_at_ms: 0,
            seen_down: false,
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
            sent_at_ms: 0,
            seen_down: false,
        };
        assert!(!plain.steps().contains(&WatchStep::PendingCleared));
    }

    /// Время запуска Patroni (`postmaster_start_time`) читается в миллисекундах эпохи; другой вид — `None`.
    #[test]
    fn postmaster_start_time_is_read_in_patroni_format() {
        assert_eq!(start_time_ms("1970-01-01 00:00:01.5+00:00"), Some(1500));
        assert_eq!(start_time_ms("1970-01-01 03:00:02+03:00"), Some(2000));
        let a = start_time_ms("2026-10-25 13:54:45.548590+00:00").unwrap();
        let b = start_time_ms("2026-10-25 13:54:44.548590+00:00").unwrap();
        assert_eq!(a - b, 1000);
        assert_eq!(start_time_ms("t1"), None);
        assert_eq!(start_time_ms("2026-10-25 13:54:45+00"), None);
    }

    /// Время запуска до действия неизвестно: перезапуск подтверждает простой участника, увиденный наблюдением, и
    /// (если время запуска разборчиво) время запуска позже отправки запроса. Одно лишь время — не подтверждение:
    /// часы сервера могут расходиться с часами приложения.
    #[test]
    fn restart_without_a_known_start_time_needs_a_seen_downtime() {
        let sent = start_time_ms("2026-10-25 09:52:53.829+00:00").unwrap();
        let fresh = || WatchPlan::Restart {
            member: "pg-2".into(),
            start_before: None,
            only_pending: false,
            sent_at_ms: sent,
            seen_down: false,
        };
        let running = topo(vec![
            m("pg-1", Role::Primary, "running", 2),
            m("pg-2", Role::Replica, "streaming", 2),
        ]);
        let old = Some("2026-10-25 09:00:00.000+00:00");
        let new = Some("2026-10-25 09:53:10.001+00:00");
        let mut plan = fresh();
        plan.observe(&running);
        for start in [None, old, Some("t1"), new] {
            assert_eq!(
                progress(&plan, &running, start),
                WatchStep::Accepted,
                "{start:?}: простоя не видели"
            );
        }
        // Простой увиден (остановлен): время запуска позже отправки или неразборчивое — перезапуск, старое — нет.
        let mut stopped = running.clone();
        stopped.members[1].state = Some("stopped".into());
        plan.observe(&stopped);
        assert_eq!(progress(&plan, &stopped, Some("t1")), WatchStep::Restarted);
        assert_eq!(progress(&plan, &running, new), WatchStep::Done);
        assert_eq!(progress(&plan, &running, None), WatchStep::Done);
        assert_eq!(
            progress(&plan, &running, old),
            WatchStep::Accepted,
            "запуск раньше отправки"
        );
        // Недоступный участник — тоже простой.
        let mut unseen = fresh();
        let mut unreachable = running.clone();
        unreachable.members[1].reachable = false;
        unseen.observe(&unreachable);
        assert_eq!(progress(&unseen, &running, new), WatchStep::Done);
    }

    /// Время запуска до действия известно — подтверждает только другое время; простой тут не нужен.
    #[test]
    fn restart_with_a_known_start_time_compares_the_two() {
        let plan = WatchPlan::Restart {
            member: "pg-2".into(),
            start_before: Some("t0".into()),
            only_pending: false,
            sent_at_ms: 0,
            seen_down: false,
        };
        let running = topo(vec![m("pg-2", Role::Replica, "streaming", 2)]);
        assert_eq!(progress(&plan, &running, Some("t0")), WatchStep::Accepted);
        assert_eq!(progress(&plan, &running, None), WatchStep::Accepted);
        assert_eq!(progress(&plan, &running, Some("t1")), WatchStep::Done);
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
            leader: "pg-1".into(),
            candidate: None,
            scheduled_at: Some("2030-01-01T00:00:00Z".into()),
        };
        assert!(WatchPlan::for_action(&sched, &t, None, 0).is_none());
        assert!(WatchPlan::for_action(&PatroniAction::CancelSwitchover, &t, None, 0).is_none());
        let now = PatroniAction::Switchover {
            leader: "pg-1".into(),
            candidate: None,
            scheduled_at: None,
        };
        assert_eq!(
            WatchPlan::for_action(&now, &t, None, 0).unwrap().deadline(),
            Duration::from_secs(120)
        );
        let restart = PatroniAction::Restart {
            member: "pg-1".into(),
            only_pending: false,
            scheduled_at: None,
        };
        assert_eq!(
            WatchPlan::for_action(&restart, &t, Some("t0".into()), 0)
                .unwrap()
                .deadline(),
            Duration::from_secs(300)
        );
        assert_eq!(
            WatchPlan::for_action(&PatroniAction::Pause { paused: true }, &t, None, 0)
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

    /// Наблюдение на закрытом подключении (закрыто до начала или пока оно начиналось) кончается сразу: после
    /// «принято» — `failed` с причиной `notConnected`; сессии нет.
    #[tokio::test]
    async fn a_watch_on_a_closed_connection_fails_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let st =
            AppState::new(dir.path(), Arc::new(crate::secrets::MemoryStore::default())).unwrap();
        let events = Arc::new(Events::default());
        let sid = start(&st, "c", WatchPlan::Pause { paused: true }, events.clone());
        let got = events.0.lock().unwrap().clone();
        assert_eq!(
            got.iter().map(|e| (e.step, e.status)).collect::<Vec<_>>(),
            [
                (WatchStep::Accepted, WatchStatus::Running),
                (WatchStep::Accepted, WatchStatus::Failed)
            ]
        );
        assert_eq!(
            got[1].detail.as_ref().map(|e| e.key.as_str()),
            Some("errors.topology.notConnected")
        );
        assert!(!st.sessions.stop(&sid), "сессии нет");
        tokio::time::sleep(BOOST_INTERVAL + Duration::from_millis(200)).await;
        assert_eq!(
            events.0.lock().unwrap().len(),
            2,
            "задача наблюдения не идёт"
        );
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
