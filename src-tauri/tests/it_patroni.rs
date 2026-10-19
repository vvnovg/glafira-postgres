//! Patroni REST API стенда (`GPG_IT_PATRONI`); переключения и пауза — ещё и `GPG_IT_DOCKER=1`.

mod common;

use std::time::Duration;

use glafira_pg_lib::topology::journal::{EventKind, diff};
use glafira_pg_lib::topology::model::{Role, Topology, now_ms};
use glafira_pg_lib::topology::patroni::{ReqwestHttp, discover};

async fn observe(prev: Option<&Topology>) -> Option<Topology> {
    let settings = common::patroni_settings()?;
    let http =
        ReqwestHttp::new(&settings, Some(common::PATRONI_PASSWORD_VALUE.into()), None).unwrap();
    Some(
        discover(&http, &settings, "it", prev, now_ms())
            .await
            .unwrap(),
    )
}

#[tokio::test]
async fn discovers_three_members_with_one_leader() {
    let Some(t) = observe(None).await else { return };
    assert_eq!(t.scope.as_deref(), Some("demo"));
    assert_eq!(t.members.len(), 3);
    assert_eq!(
        t.members.iter().filter(|m| m.role == Role::Primary).count(),
        1
    );
    assert!(t.members.iter().all(|m| m.reachable), "{:?}", t.warnings);
    assert!(t.members.iter().all(|m| {
        m.server_version
            .as_deref()
            .is_some_and(|v| v.starts_with("17."))
    }));
    assert!(t.members.iter().all(|m| {
        m.patroni_version
            .as_deref()
            .is_some_and(|v| v.starts_with("4."))
    }));
    assert!(t.warnings.is_empty(), "{:?}", t.warnings);
}

#[tokio::test]
async fn switchover_and_pause_are_observed() {
    if common::env("GPG_IT_DOCKER").is_none() {
        return;
    }
    let Some(before) = observe(None).await else {
        return;
    };
    // Стенд приводится в порядок при любом выходе, в том числе при падении проверок: после
    // переключения и паузы следующий тест должен видеть лидера и две потоковые реплики.
    let restore = common::restore_on_drop(|| {
        // Если пауза не включилась, `resume` вернёт ошибку — это не помеха.
        let _ = common::try_patronictl(&["resume", "--wait"]);
        common::wait_stand();
    });
    let leader = before.leader().unwrap().name.clone();
    let candidate = before
        .members
        .iter()
        .find(|m| m.name != leader)
        .unwrap()
        .name
        .clone();
    common::patronictl(&[
        "switchover",
        "--leader",
        &leader,
        "--candidate",
        &candidate,
        "--force",
    ]);
    let mut after = before.clone();
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        after = observe(Some(&before)).await.unwrap();
        if after.leader().is_some_and(|m| m.name == candidate) {
            break;
        }
    }
    let events = diff(&before, &after);
    assert!(
        events.iter().any(|e| e.event
            == EventKind::LeaderChanged {
                from: Some(leader.clone()),
                to: Some(candidate.clone())
            }),
        "{events:?}"
    );

    common::patronictl(&["pause", "--wait"]);
    let paused = observe(Some(&after)).await.unwrap();
    drop(restore);
    assert_eq!(paused.paused, Some(true));
    let resumed = observe(Some(&paused)).await.unwrap();
    assert_eq!(resumed.paused, Some(false), "пауза снята");
}

#[tokio::test]
async fn stopped_member_becomes_unreachable() {
    if common::env("GPG_IT_DOCKER").is_none() {
        return;
    }
    let Some(before) = observe(None).await else {
        return;
    };
    // Останавливаем реплику (не лидера), чтобы не ждать failover.
    let victim = before
        .members
        .iter()
        .find(|m| m.role != Role::Primary)
        .unwrap()
        .name
        .clone();
    let container = format!("gp-{victim}");
    // Контейнер запускается снова, даже если `observe` или проверка ниже упали.
    let start = common::restore_on_drop(|| {
        common::docker(&["start", &container]);
        common::wait_stand();
    });
    common::docker(&["stop", &container]);
    let t = observe(Some(&before)).await.unwrap();
    drop(start);
    let m = t
        .member(&victim)
        .expect("участник ещё в /cluster до истечения ttl");
    assert!(!m.reachable);
}
