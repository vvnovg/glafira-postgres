//! Действия Patroni на стенде (спец. P2 §8.5–8.6, §12.2). Меняют кластер — только под `GPG_IT_DOCKER=1`;
//! восстановление — при любом выходе (`common::restore_on_drop`), затем `common::wait_stand()`.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use glafira_pg_lib::actions::Confirmation;
use glafira_pg_lib::actions::journal::Outcome;
use glafira_pg_lib::actions::patroni::{self as pa, PatroniAction};
use glafira_pg_lib::actions::watch::{
    self, WatchEvent, WatchPlan, WatchSink, WatchStatus, WatchStep,
};
use glafira_pg_lib::error::ErrorCode;
use glafira_pg_lib::topology::model::{Role, now_ms};
use glafira_pg_lib::topology::patroni::{ReqwestHttp, discover};

fn docker_enabled() -> bool {
    common::env("GPG_IT_DOCKER").as_deref() == Some("1")
}

/// Ждёт, пока топология подключения `it` (её опрос идёт сам) не станет такой, как требует `cond`.
async fn topology_until(
    st: &glafira_pg_lib::state::AppState,
    what: &str,
    limit: Duration,
    cond: impl Fn(&glafira_pg_lib::topology::model::Topology) -> bool,
) {
    common::wait_until(what, limit, async || {
        st.runtimes.view("it").topology.as_ref().is_some_and(&cond)
    })
    .await;
}

#[tokio::test]
async fn scheduled_switchover_is_shown_and_cancelled() {
    if !docker_enabled() {
        return;
    }
    let Some((_dir, st)) = common::open_patroni().await else {
        return;
    };
    let restore = common::restore_on_drop(|| {
        let _ = common::try_patronictl(&["flush", "demo", "switchover", "--force"]);
        common::wait_stand();
    });
    let in_an_hour = time::OffsetDateTime::now_utc() + time::Duration::hours(1);
    let at = in_an_hour
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let action = PatroniAction::Switchover {
        candidate: None,
        scheduled_at: Some(at),
    };
    let p = pa::preview(&st, "it", &action).await.unwrap();
    assert!(p.can_send && p.leader.is_some());
    let done = pa::perform(&st, "it", &action, &Confirmation::default())
        .await
        .unwrap();
    assert!(done.journal_written);
    topology_until(
        &st,
        "запланированный switchover виден",
        Duration::from_secs(30),
        |t| t.scheduled_switchover.is_some(),
    )
    .await;
    pa::perform(
        &st,
        "it",
        &PatroniAction::CancelSwitchover,
        &Confirmation::default(),
    )
    .await
    .unwrap();
    topology_until(
        &st,
        "расписание снято",
        Duration::from_secs(30),
        |t| t.scheduled_switchover.is_none(),
    )
    .await;
    let actions: Vec<String> = st
        .actions
        .list(Some("it"), 10)
        .into_iter()
        .map(|r| r.action)
        .collect();
    assert_eq!(actions, ["cancelSwitchover", "switchover"]);
    // После восстановления расписания на стенде нет: свежее чтение /cluster, а не кэш опроса топологии.
    drop(restore);
    let settings = common::patroni_settings().unwrap();
    let http = ReqwestHttp::new(
        &settings,
        Some(common::PATRONI_PASSWORD_VALUE.into()),
        None,
        None,
        None,
    )
    .unwrap();
    let after = discover(&http, &settings, "it", None, now_ms())
        .await
        .unwrap();
    assert!(after.scheduled_switchover.is_none(), "{after:?}");
}

#[tokio::test]
async fn read_only_and_missing_login_are_refused() {
    let Some((_dir, st)) = common::open_patroni().await else {
        return;
    };
    // Если защита когда-нибудь пропустит действие, пауза не должна остаться на стенде (нужен Docker).
    let _restore = docker_enabled().then(|| {
        common::restore_on_drop(|| {
            let _ = common::try_patronictl(&["resume", "--wait"]);
            common::wait_stand();
        })
    });
    let mut p = st.profile("it").unwrap();
    p.read_only = true;
    st.connections.upsert(p.clone()).unwrap();
    let e = pa::perform(
        &st,
        "it",
        &PatroniAction::Pause { paused: true },
        &Confirmation::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::ReadOnly);
    p.read_only = false;
    p.patroni.as_mut().unwrap().username = None;
    st.connections.upsert(p).unwrap();
    let e = pa::perform(
        &st,
        "it",
        &PatroniAction::Pause { paused: true },
        &Confirmation::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(e.key, "errors.patroni.authRequired");
    let e = pa::perform(
        &st,
        "it",
        &PatroniAction::Failover {
            candidate: "patroni2".into(),
        },
        &Confirmation::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        e.code,
        ErrorCode::ConfirmationRequired,
        "имя кандидата обязательно"
    );
    assert!(
        st.actions.list(Some("it"), 10).is_empty(),
        "отказы защиты в журнал не пишутся"
    );
}

/// Предпросмотр ничего не меняет: кандидаты с готовностью, клиентские сессии лидера, участник для restart.
#[tokio::test]
async fn preview_shows_candidates_leader_sessions_and_the_member() {
    let Some((_dir, st)) = common::open_patroni().await else {
        return;
    };
    let topology = st.runtimes.view("it").topology.expect("топология открыта");
    let switchover = PatroniAction::Switchover {
        candidate: None,
        scheduled_at: None,
    };
    let p = pa::preview(&st, "it", &switchover).await.unwrap();
    let leader = p.leader.clone().expect("у стенда есть лидер");
    assert!(p.can_send && !p.paused);
    assert_eq!(p.leader_state.as_deref(), Some("running"));
    assert_eq!(p.leader_reachable, Some(true));
    let mut names: Vec<&str> = p.candidates.iter().map(|c| c.member.as_str()).collect();
    names.sort();
    assert_eq!(names.len(), 2, "две реплики: {:?}", p.candidates);
    assert!(!names.contains(&leader.as_str()), "лидер не кандидат");
    assert!(
        p.candidates.iter().all(|c| c.ready),
        "на здоровом стенде обе реплики готовы: {:?}",
        p.candidates
    );
    assert!(p.member.is_none() && p.scheduled_switchover.is_none());

    // Число сессий лидера: клиент сверх начального счёта добавляет ровно одну, сессия самого Glafira
    // (`application_name` продукта) не считается.
    let base = p.leader_client_sessions.expect("сессии лидера прочитаны");
    let port = topology.member(&leader).expect("лидер в топологии").port;
    let addr = format!("localhost:{port}");
    let _own = common::connect_as(
        &addr,
        "postgres",
        common::SUPERUSER,
        common::SUPERUSER_PASSWORD,
    )
    .await;
    let _client = common::superuser(&addr, "postgres").await;
    common::wait_until(
        "сессия клиента видна, сессия Glafira — нет",
        Duration::from_secs(10),
        async || {
            pa::preview(&st, "it", &switchover)
                .await
                .unwrap()
                .leader_client_sessions
                == Some(base + 1)
        },
    )
    .await;

    let replica = p.candidates[0].member.clone();
    let restart = PatroniAction::Restart {
        member: replica.clone(),
        only_pending: false,
        scheduled_at: None,
    };
    let p = pa::preview(&st, "it", &restart).await.unwrap();
    let m = p.member.expect("участник restart");
    assert_eq!(m.name, replica);
    assert!(m.role != Role::Primary && m.state.is_some());
    assert!(p.candidates.is_empty() && p.leader_client_sessions.is_none());
}

/// Действия Patroni — только у подключения к кластеру Patroni.
#[tokio::test]
async fn patroni_actions_need_a_patroni_connection() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let (_dir, st) = common::open_server(&addr).await;
    let action = PatroniAction::Pause { paused: true };
    let e = pa::preview(&st, "it", &action).await.unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str()),
        (ErrorCode::InvalidInput, "errors.actions.patroniOnly")
    );
    let e = pa::perform(&st, "it", &action, &Confirmation::default())
        .await
        .unwrap_err();
    assert_eq!(e.key, "errors.actions.patroniOnly");
    assert!(st.actions.list(Some("it"), 10).is_empty());
}

/// Отказ Patroni попадает в журнал как `error` с кодом ответа и адресом участника без учётных данных, а команда
/// возвращает `actionRejected`. Restart «только если нужен» на участнике без отложенного перезапуска Patroni
/// отклоняет, ничего не перезапуская.
#[tokio::test]
async fn rejected_action_is_journaled_as_an_error() {
    if !docker_enabled() {
        return;
    }
    let Some((_dir, st)) = common::open_patroni().await else {
        return;
    };
    let _restore = common::restore_on_drop(common::wait_stand);
    let topology = st.runtimes.view("it").topology.expect("топология открыта");
    let replica = topology
        .members
        .iter()
        .find(|m| m.role != Role::Primary && m.reachable && !m.pending_restart)
        .unwrap_or_else(|| {
            panic!(
                "нужна доступная реплика без отложенного перезапуска: {:?}",
                topology.members
            )
        });
    let action = PatroniAction::Restart {
        member: replica.name.clone(),
        only_pending: true,
        scheduled_at: None,
    };
    let e = pa::perform(&st, "it", &action, &Confirmation::default())
        .await
        .unwrap_err();
    assert_eq!(
        (e.code, e.key.as_str()),
        (ErrorCode::PatroniError, "errors.patroni.actionRejected"),
        "{e:?}"
    );
    let records = st.actions.list(Some("it"), 10);
    assert_eq!(records.len(), 1, "{records:?}");
    let r = &records[0];
    assert_eq!(
        (r.action.as_str(), r.outcome, r.code.as_deref()),
        ("restart", Outcome::Error, Some("PATRONI_ERROR"))
    );
    assert_eq!(r.member.as_deref(), Some(replica.name.as_str()));
    let rec = r.patroni.as_ref().expect("запись Patroni");
    assert_eq!(rec.method, "POST");
    assert_eq!(
        rec.path,
        format!("{}/restart", replica.api_url.as_deref().unwrap())
    );
    assert_eq!(
        rec.status,
        e.params.get("status").and_then(|s| s.parse().ok())
    );
    assert!(rec.status.is_some_and(|s| s >= 400), "{rec:?}");
    assert_eq!(
        rec.response, e.details,
        "в журнале тот же ответ Patroni, что и в ошибке"
    );
    assert!(!rec.path.contains('@'), "адрес без учётных данных");
}

#[derive(Default)]
struct Events(Mutex<Vec<WatchEvent>>);

impl WatchSink for Events {
    fn send(&self, e: WatchEvent) -> bool {
        self.0.lock().unwrap().push(e);
        true
    }
}

impl Events {
    async fn until_finished(&self, limit: Duration) -> WatchEvent {
        common::wait_until(
            "наблюдение закончилось",
            limit,
            async || {
                self.0
                    .lock()
                    .unwrap()
                    .last()
                    .is_some_and(|e| e.status != WatchStatus::Running)
            },
        )
        .await;
        self.0.lock().unwrap().last().cloned().unwrap()
    }
}

fn replica_of(t: &glafira_pg_lib::topology::model::Topology) -> String {
    let leader = t.leader().unwrap().name.clone();
    t.members
        .iter()
        .find(|m| m.name != leader && m.reachable)
        .unwrap()
        .name
        .clone()
}

/// Закрытие подключения во время наблюдения (стенд не меняется: ждём паузы, которой никто не ставит). Слушатель
/// закрытия останавливает сессию наблюдения под блокировкой `Runtimes`, а её остановка снимает ускорение: закрытие
/// не зависает, сессии больше нет. `disconnect` — в своём потоке: зависание провалит тест по сроку.
#[tokio::test]
async fn closing_the_connection_during_a_watch_stops_it_without_hanging() {
    let Some((_dir, st)) = common::open_patroni().await else {
        return;
    };
    let events = Arc::new(Events::default());
    let sid = watch::start(&st, "it", WatchPlan::Pause { paused: true }, events.clone());
    common::wait_until(
        "наблюдение началось",
        Duration::from_secs(5),
        async || !events.0.lock().unwrap().is_empty(),
    )
    .await;
    let (tx, rx) = std::sync::mpsc::channel();
    let runtimes = st.runtimes.clone();
    std::thread::spawn(move || {
        runtimes.disconnect("it");
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("закрытие подключения во время наблюдения зависло");
    assert!(
        !st.sessions.stop(&sid),
        "закрытие подключения остановило наблюдение"
    );
    let seen = events.0.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(
        (seen[0].step, seen[0].status),
        (WatchStep::Accepted, WatchStatus::Running)
    );
}

#[tokio::test]
async fn switchover_now_is_watched_until_done() {
    if !docker_enabled() {
        return;
    }
    let Some((_dir, st)) = common::open_patroni().await else {
        return;
    };
    let _restore = common::restore_on_drop(common::wait_stand);
    let t = st.runtimes.view("it").topology.unwrap();
    let candidate = replica_of(&t);
    let events = Arc::new(Events::default());
    let action = PatroniAction::Switchover {
        candidate: Some(candidate.clone()),
        scheduled_at: None,
    };
    let r = pa::execute(&st, "it", &action, &Confirmation::default(), events.clone())
        .await
        .unwrap();
    // Patroni отвечает на switchover после смены лидера: срок запроса это ожидание покрывает, исход известен.
    assert!(
        r.watch.is_some() && r.journal_written && !r.outcome_unknown,
        "{r:?}"
    );
    let last = events.until_finished(Duration::from_secs(130)).await;
    assert_eq!(
        (last.step, last.status),
        (WatchStep::Done, WatchStatus::Done),
        "{:?}",
        events.0.lock().unwrap()
    );
    assert_eq!(
        st.runtimes
            .view("it")
            .topology
            .unwrap()
            .leader()
            .unwrap()
            .name,
        candidate
    );
}

#[tokio::test]
async fn replica_restart_and_pause_resume_are_watched() {
    if !docker_enabled() {
        return;
    }
    let Some((_dir, st)) = common::open_patroni().await else {
        return;
    };
    let _restore = common::restore_on_drop(|| {
        let _ = common::try_patronictl(&["resume", "--wait"]);
        common::wait_stand();
    });
    let t = st.runtimes.view("it").topology.unwrap();
    let replica = replica_of(&t);
    let events = Arc::new(Events::default());
    let restart = PatroniAction::Restart {
        member: replica,
        only_pending: false,
        scheduled_at: None,
    };
    pa::execute(
        &st,
        "it",
        &restart,
        &Confirmation::default(),
        events.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        events.until_finished(Duration::from_secs(310)).await.step,
        WatchStep::Done
    );
    for paused in [true, false] {
        let events = Arc::new(Events::default());
        pa::execute(
            &st,
            "it",
            &PatroniAction::Pause { paused },
            &Confirmation::default(),
            events.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            events.until_finished(Duration::from_secs(40)).await.status,
            WatchStatus::Done
        );
    }
}

/// Failover (`GPG_IT_FAILOVER=1`): пауза → остановка PostgreSQL лидера → failover на реплику → снятие паузы →
/// старый лидер возвращается репликой.
#[tokio::test]
async fn failover_after_the_leader_stops() {
    if !docker_enabled() || common::env("GPG_IT_FAILOVER").as_deref() != Some("1") {
        return;
    }
    let Some((_dir, st)) = common::open_patroni().await else {
        return;
    };
    let _restore = common::restore_on_drop(|| {
        let _ = common::try_patronictl(&["resume", "--wait"]);
        common::wait_stand();
    });
    let t = st.runtimes.view("it").topology.unwrap();
    let leader = t.leader().unwrap().name.clone();
    let candidate = replica_of(&t);
    let none = Arc::new(Events::default());
    pa::execute(
        &st,
        "it",
        &PatroniAction::Pause { paused: true },
        &Confirmation::default(),
        none.clone(),
    )
    .await
    .unwrap();
    none.until_finished(Duration::from_secs(40)).await;
    // Контейнер участника — gp-<имя участника>; pg_ctl — от postgres.
    common::docker(&[
        "exec",
        "-u",
        "postgres",
        &format!("gp-{leader}"),
        "pg_ctl",
        "-D",
        "/var/lib/postgresql/patroni/data",
        "stop",
        "-m",
        "fast",
    ]);
    topology_until(
        &st,
        "лидер остановлен",
        Duration::from_secs(60),
        |t| {
            t.member(&leader)
                .is_some_and(|m| m.state.as_deref() != Some("running"))
        },
    )
    .await;
    let events = Arc::new(Events::default());
    let confirm = Confirmation {
        confirm: None,
        typed_name: Some(candidate.clone()),
    };
    let r = pa::execute(
        &st,
        "it",
        &PatroniAction::Failover {
            candidate: candidate.clone(),
        },
        &confirm,
        events.clone(),
    )
    .await
    .unwrap();
    // Ответ Patroni приходит после смены лидера (дольше обычных 5 с): исход известен, наблюдение открыто.
    assert!(r.watch.is_some() && !r.outcome_unknown, "{r:?}");
    let last = events.until_finished(Duration::from_secs(130)).await;
    assert!(
        last.step >= WatchStep::NewLeaderRunning,
        "{:?}",
        events.0.lock().unwrap()
    );
    pa::execute(
        &st,
        "it",
        &PatroniAction::Pause { paused: false },
        &Confirmation::default(),
        Arc::new(Events::default()),
    )
    .await
    .unwrap();
    common::wait_stand();
    let t = st.runtimes.view("it").topology.unwrap();
    assert_eq!(t.leader().unwrap().name, candidate);
}
