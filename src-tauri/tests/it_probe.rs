//! Список узлов стенда (`GPG_IT_PRIMARY`, `GPG_IT_REPLICA`); остановка и promote — с `GPG_IT_DOCKER=1`.

mod common;

use glafira_pg_lib::topology::connector::AdhocConnector;
use glafira_pg_lib::topology::model::{Role, Topology, TopologyWarning, now_ms};
use glafira_pg_lib::topology::probe::observe_hosts;

async fn observe() -> Option<Topology> {
    let p = common::host_list_profile("it")?;
    let nodes = AdhocConnector {
        profile: p.clone(),
        secrets: common::secrets_with_password("it"),
    };
    Some(observe_hosts(&p.id, p.kind, &p.hosts, &nodes, now_ms()).await)
}

#[tokio::test]
async fn primary_and_replica_are_detected() {
    let Some(t) = observe().await else { return };
    assert!(t.warnings.is_empty(), "{:?}", t.warnings);
    assert_eq!(t.members[0].role, Role::Primary);
    assert!(t.members[1].role.is_standby(), "{:?}", t.members[1].role);
    assert!(t.members[1].lag_bytes.is_some());
    assert_eq!(t.members[1].upstream.as_deref(), Some("pg-primary:5432"));
    assert!(t.members.iter().all(|m| {
        m.server_version
            .as_deref()
            .is_some_and(|v| v.starts_with("16"))
    }));
}

#[tokio::test]
async fn stopped_primary_then_promoted_replica() {
    if common::env("GPG_IT_DOCKER").is_none() || common::env("GPG_IT_PRIMARY").is_none() {
        return;
    }
    // Пара пересоздаётся при любом выходе, в том числе при падении проверок: promote необратим,
    // а следующий тест должен снова видеть primary и потоковую реплику.
    let restore = common::restore_on_drop(common::reset_replication_pair);
    common::docker(&["stop", "gp-pg-primary"]);
    let down = observe().await.unwrap();
    common::docker(&[
        "exec",
        "gp-pg-replica",
        "psql",
        "-U",
        "postgres",
        "-tAc",
        "select pg_promote()",
    ]);
    let promoted = observe().await.unwrap();
    drop(restore);

    assert!(
        down.warnings.contains(&TopologyWarning::NoPrimary),
        "{:?}",
        down.warnings
    );
    assert!(!down.members[0].reachable);
    assert_eq!(promoted.members[1].role, Role::Primary);
    assert!(!promoted.warnings.contains(&TopologyWarning::NoPrimary));
}
