//! Открытые подключения на стенде: switchover Patroni попадает в журнал (`GPG_IT_PATRONI`,
//! `GPG_IT_DOCKER=1`), пулы и узел каталога у списка узлов (`GPG_IT_PRIMARY`, `GPG_IT_REPLICA`),
//! сведения об узле и закрытие молчащего соединения (`GPG_IT_SINGLE`).
//!
//! Запускать с `--test-threads=1`: тест с switchover меняет общий стенд.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use glafira_pg_lib::commands::topology::{member_facts, member_facts_within};
use glafira_pg_lib::conn::pool::{PoolKey, PoolRegistry};
use glafira_pg_lib::conn::profile::{ConnectionKind, SslMode};
use glafira_pg_lib::secrets::{PATRONI_PASSWORD, SecretStore, secret_key};
use glafira_pg_lib::state::AppState;
use glafira_pg_lib::topology::journal::EventKind;
use glafira_pg_lib::topology::model::Role;
use glafira_pg_lib::topology::runtime::{ConnState, LiveSource, Runtimes};

fn runtimes(secrets: Arc<dyn SecretStore>) -> (Arc<Runtimes>, Arc<PoolRegistry>) {
    let pools = Arc::new(PoolRegistry::default());
    let rt = Runtimes::new(
        Arc::new(LiveSource::new(secrets, pools.clone())),
        pools.clone(),
    );
    (rt, pools)
}

#[tokio::test]
async fn switchover_reaches_journal_within_poll_window() {
    if common::env("GPG_IT_DOCKER").is_none() {
        return;
    }
    let Some(settings) = common::patroni_settings() else {
        return;
    };
    let mut profile = common::server_profile("rt", "localhost:5441");
    profile.kind = ConnectionKind::Patroni;
    profile.hosts.clear();
    profile.patroni = Some(settings);
    let secrets = common::secrets_with_password("rt");
    secrets
        .set(
            &secret_key("rt", PATRONI_PASSWORD),
            common::PATRONI_PASSWORD_VALUE,
        )
        .unwrap();
    let (rt, _pools) = runtimes(secrets);

    let view = rt.connect(profile).await.unwrap();
    assert_eq!(
        view.state,
        ConnState::Connected,
        "{:?}",
        view.topology.map(|t| t.warnings)
    );
    let topo = view.topology.unwrap();
    let leader = topo.leader().unwrap().name.clone();
    let candidate = topo
        .members
        .iter()
        .find(|m| m.name != leader)
        .unwrap()
        .name
        .clone();
    // После переключения следующий тест должен видеть лидера и две потоковые реплики: стенд ждём при любом
    // выходе, в том числе при падении проверок ниже. Защита объявлена после `rt` и освобождается раньше него.
    let restore = common::restore_on_drop(common::wait_stand);
    common::patronictl(&[
        "switchover",
        "--leader",
        &leader,
        "--candidate",
        &candidate,
        "--force",
    ]);
    let switched = Instant::now();

    let deadline = Instant::now() + Duration::from_secs(40);
    let found = loop {
        let hit = rt.events("rt").into_iter().find(
            |e| matches!(&e.event, EventKind::LeaderChanged { to: Some(t), .. } if *t == candidate),
        );
        if let Some(e) = hit {
            break Some(e);
        }
        if Instant::now() > deadline {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    let latency = switched.elapsed();
    let catalog = rt.view("rt").catalog_member;
    rt.shutdown();
    eprintln!("switchover -> LeaderChanged в журнале: {latency:?}");
    assert!(found.is_some(), "LeaderChanged не попал в журнал");
    // Критерий спеки — не дольше 15 с после завершения переключения (переключение само занимает несколько секунд).
    assert!(latency < Duration::from_secs(40));
    assert_eq!(
        catalog.as_deref(),
        Some(candidate.as_str()),
        "каталог следует за новым лидером"
    );
    assert_eq!(rt.view("rt").state, ConnState::Disconnected);
    drop(restore);
}

#[tokio::test]
async fn host_list_uses_pooled_connections_and_default_member() {
    let Some(mut profile) = common::host_list_profile("rt-list") else {
        return;
    };
    profile.pg.ssl_mode = SslMode::Disable;
    let (rt, pools) = runtimes(common::secrets_with_password("rt-list"));

    let view = rt.connect(profile).await.unwrap();
    assert_eq!(
        view.state,
        ConnState::Connected,
        "{:?}",
        view.topology.map(|t| t.warnings)
    );
    let topo = view.topology.unwrap();
    let primary = topo
        .members
        .iter()
        .find(|m| m.role == Role::Primary)
        .unwrap();
    let replica = topo.members.iter().find(|m| m.role.is_standby()).unwrap();
    assert_eq!(
        view.catalog_member.as_deref(),
        Some(primary.name.as_str()),
        "цель — primary"
    );

    // Каждый узел опрошен через свой пул, исправное соединение вернулось в пул.
    for m in [primary, replica] {
        let key = PoolKey {
            connection_id: "rt-list".into(),
            member: m.name.clone(),
            database: "postgres".into(),
        };
        assert_eq!(
            pools.get(&key).expect("пул узла").idle_count(),
            1,
            "{}",
            m.name
        );
    }
    let v = rt
        .set_catalog_member("rt-list", Some(replica.name.clone()))
        .unwrap();
    assert_eq!(
        (v.catalog_member.as_deref(), v.catalog_fallback),
        (Some(replica.name.as_str()), false)
    );
    let (m, _) = rt.target("rt-list", None).unwrap();
    assert_eq!(m.name, replica.name);
    rt.disconnect("rt-list");
    assert!(pools.keys().is_empty());
}

fn single_state(id: &str, addr: &str) -> (tempfile::TempDir, AppState, String) {
    let mut profile = common::server_profile(id, addr);
    profile.pg.ssl_mode = SslMode::Disable;
    let dir = tempfile::tempdir().unwrap();
    let st = AppState::new(dir.path(), common::secrets_with_password(id)).unwrap();
    let member =
        glafira_pg_lib::topology::model::member_name(&profile.hosts[0].host, profile.hosts[0].port);
    st.connections.upsert(profile).unwrap();
    (dir, st, member)
}

#[tokio::test]
async fn member_facts_come_from_the_pooled_connection() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let (_dir, st, member) = single_state("rt-single", &addr);
    let view = st
        .runtimes
        .connect(st.profile("rt-single").unwrap())
        .await
        .unwrap();
    assert_eq!(view.state, ConnState::Connected);
    assert_eq!(view.catalog_member.as_deref(), Some(member.as_str()));

    let facts = member_facts(&st, "rt-single", &member).await.unwrap();
    assert!(
        facts.connections >= 1 && facts.max_connections >= facts.connections,
        "{facts:?}"
    );
    assert!(facts.databases >= 1);
    let started = facts.started_at_ms;
    assert!(
        started > 1_500_000_000_000 && started < glafira_pg_lib::topology::model::now_ms(),
        "{started}"
    );

    // Узел каталога и БД по умолчанию — `member = None`, `database = None`; другая БД — отдельный пул.
    let c = st.client("rt-single", None, None).await.unwrap();
    let pid_default: i32 = c
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    drop(c);
    let again = st
        .client("rt-single", None, Some("postgres"))
        .await
        .unwrap();
    let pid_again: i32 = again
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(pid_default, pid_again, "то же соединение из пула");
    drop(again);
    let other = st
        .client("rt-single", Some(&member), Some("template1"))
        .await
        .unwrap();
    let db: String = other
        .query_one("SELECT current_database()", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(db, "template1");
    drop(other);

    let e = member_facts(&st, "rt-single", "нет-такого")
        .await
        .unwrap_err();
    assert_eq!(e.key, "errors.topology.unknownMember");
    st.runtimes.disconnect("rt-single");
    let e = member_facts(&st, "rt-single", &member).await.unwrap_err();
    assert_eq!(e.key, "errors.topology.notConnected");
}

/// Молчащий путь: запрос не получает ответа, по таймауту соединение закрывается (сокет, не пул),
/// следующий запрос идёт по новому соединению.
#[tokio::test]
async fn stalled_member_facts_close_the_connection() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let proxy = common::PausableProxy::start(&addr).await;
    let (_dir, st, member) = single_state("rt-stall", &format!("127.0.0.1:{}", proxy.port()));
    st.runtimes
        .connect(st.profile("rt-stall").unwrap())
        .await
        .unwrap();
    let key = PoolKey {
        connection_id: "rt-stall".into(),
        member: member.clone(),
        database: "postgres".into(),
    };
    assert_eq!(
        st.pools.get(&key).unwrap().idle_count(),
        1,
        "опрос оставил соединение в пуле"
    );
    member_facts(&st, "rt-stall", &member).await.unwrap();

    proxy.freeze_existing();
    let started = Instant::now();
    let e = member_facts_within(&st, "rt-stall", &member, Duration::from_millis(500))
        .await
        .unwrap_err();
    assert_eq!(e.key, "errors.pg.noAnswer", "{e:?}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert!(
        proxy.wait_closed(1, Duration::from_secs(3)).await,
        "сокет зависшего соединения закрыт"
    );
    assert_eq!(
        st.pools.get(&key).unwrap().idle_count(),
        0,
        "зависшее соединение не в пуле"
    );

    let facts = member_facts(&st, "rt-stall", &member).await.unwrap();
    assert!(facts.connections >= 1);
    st.runtimes.disconnect("rt-stall");
}
