//! Список узлов стенда (`GPG_IT_PRIMARY`, `GPG_IT_REPLICA`); остановка и promote — с `GPG_IT_DOCKER=1`.
//!
//! Запускать только с `--test-threads=1`: тест с остановкой primary и promote меняет общий стенд,
//! остальные тесты в это время видели бы его полуразобранным.

mod common;

use std::time::{Duration, Instant};

use glafira_pg_lib::conn::pg::ConnectSpec;
use glafira_pg_lib::conn::profile::SslMode;
use glafira_pg_lib::error::ErrorCode;
use glafira_pg_lib::topology::connector::AdhocConnector;
use glafira_pg_lib::topology::model::{Role, Topology, TopologyWarning, now_ms};
use glafira_pg_lib::topology::probe::{collect_with_timeout, observe_hosts};

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
    // `state` берётся только из строки pg_stat_replication primary: значит, реплика сопоставлена.
    assert_eq!(t.members[1].state.as_deref(), Some("streaming"));
    assert_eq!(t.members[1].upstream.as_deref(), Some("pg-primary:5432"));
    // Версия — «16.x» без суффикса дистрибутива («16.15 (Debian 16.15-1.pgdg13+2)»).
    assert!(t.members.iter().all(|m| {
        m.server_version
            .as_deref()
            .is_some_and(|v| v.starts_with("16") && !v.contains(char::is_whitespace))
    }));
}

/// Простаивающая догнавшая реплика: задержка replay 0, а не «время с последней транзакции».
/// Несколько попыток — на случай, если опрос попал ровно на приём очередной порции WAL.
#[tokio::test]
async fn caught_up_replica_has_zero_replay_delay() {
    let mut seen = Vec::new();
    for _ in 0..5 {
        let Some(t) = observe().await else { return };
        seen.push(t.members[1].replay_delay_ms);
        if seen.last() == Some(&Some(0)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    panic!("replayDelayMs ни разу не был 0: {seen:?}");
}

/// Молчащий узел: соединение открыто, ответов нет (ни данных, ни RST). `collect` не должен
/// висеть — иначе опрос списка узлов не завершился бы никогда.
#[tokio::test]
async fn collect_gives_up_on_a_stalled_node() {
    let Some(addr) = common::env("GPG_IT_PRIMARY") else {
        return;
    };
    let proxy = common::PausableProxy::start(&addr).await;
    let mut profile = common::server_profile("it", &addr);
    profile.pg.ssl_mode = SslMode::Disable;
    let (host, port) = common::host_port(&addr);
    let secrets = common::secrets_with_password("it");
    let mut spec = ConnectSpec::build(&profile, &host, port, "postgres", secrets.as_ref()).unwrap();
    spec.tcp_host = "127.0.0.1".into();
    spec.tcp_port = proxy.port();
    let client = spec.connect().await.unwrap();

    proxy.freeze_existing();
    let started = Instant::now();
    let e = tokio::time::timeout(
        Duration::from_secs(5),
        collect_with_timeout(&client, Duration::from_secs(1)),
    )
    .await
    .expect("collect не должен висеть на молчащем узле")
    .unwrap_err();
    let elapsed = started.elapsed();
    assert_eq!(e.code, ErrorCode::ConnectionFailed);
    assert_eq!(e.key, "errors.pg.noAnswer");
    assert!(e.retryable);
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_millis(2500),
        "ответ о таймауте пришёл через {elapsed:?}"
    );
}

#[tokio::test]
async fn stopped_primary_then_promoted_replica() {
    if common::env("GPG_IT_DOCKER").is_none()
        || common::env("GPG_IT_PRIMARY").is_none()
        || common::env("GPG_IT_REPLICA").is_none()
    {
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
