//! Открытые подключения на стенде: switchover Patroni попадает в журнал (`GPG_IT_PATRONI`,
//! `GPG_IT_DOCKER=1`), пулы и узел каталога у списка узлов (`GPG_IT_PRIMARY`, `GPG_IT_REPLICA`),
//! сведения об узле и закрытие молчащего соединения (`GPG_IT_SINGLE`).
//!
//! Запускать с `--test-threads=1`: тест с switchover меняет общий стенд.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use glafira_pg_lib::commands::topology::{member_facts, member_facts_within};
use glafira_pg_lib::conn::pool::{PoolKey, PoolPurpose, PoolRegistry};
use glafira_pg_lib::conn::profile::{ConnectionKind, HostPort, SslMode};
use glafira_pg_lib::error::ErrorCode;
use glafira_pg_lib::secrets::{PATRONI_PASSWORD, PG_PASSWORD, SecretStore, secret_key};
use glafira_pg_lib::state::AppState;
use glafira_pg_lib::topology::journal::EventKind;
use glafira_pg_lib::topology::model::{Role, member_name};
use glafira_pg_lib::topology::runtime::{ConnState, LiveSource, Runtimes};

/// Срок от возврата `patronictl switchover` до `LeaderChanged` в журнале.
const SWITCHOVER_LIMIT: Duration = Duration::from_secs(20);

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

    let deadline = Instant::now() + SWITCHOVER_LIMIT;
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
    // Критерий спеки — не дольше 15 с после завершения переключения; первый опрос приходится на ~10 с
    // после открытия, а если он застанет кластер без лидера, нужен следующий (ещё 10 с). Наблюдаемое — ≈10 с.
    assert!(latency < SWITCHOVER_LIMIT, "{latency:?}");
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

    // Каждый узел опрошен через свой пул опроса, исправное соединение вернулось в пул.
    for m in [primary, replica] {
        let key = PoolKey {
            connection_id: "rt-list".into(),
            member: m.name.clone(),
            database: "postgres".into(),
            purpose: PoolPurpose::Probe,
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
    let member = member_name(&profile.hosts[0].host, profile.hosts[0].port);
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
    let probe = PoolKey {
        connection_id: "rt-stall".into(),
        member: member.clone(),
        database: "postgres".into(),
        purpose: PoolPurpose::Probe,
    };
    assert_eq!(
        st.pools.get(&probe).unwrap().idle_count(),
        1,
        "опрос оставил соединение в своём пуле"
    );
    member_facts(&st, "rt-stall", &member).await.unwrap();
    // Сведения об узле идут через пул каталога: соединение опроса ему не выдаётся.
    let key = PoolKey {
        purpose: PoolPurpose::Catalog,
        ..probe
    };
    assert_eq!(st.pools.get(&key).unwrap().idle_count(), 1);

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

/// Интервал фонового опроса в тестах неверного пароля: за пару секунд проходит много «10-секундных» циклов.
const FAST_POLL: Duration = Duration::from_millis(150);

fn fast_runtimes(secrets: Arc<dyn SecretStore>) -> (Arc<Runtimes>, Arc<PoolRegistry>) {
    let pools = Arc::new(PoolRegistry::default());
    let rt = Runtimes::with_interval(
        Arc::new(LiveSource::new(secrets, pools.clone())),
        pools.clone(),
        FAST_POLL,
    );
    (rt, pools)
}

fn wrong_password(id: &str) -> Arc<common::CountingSecrets> {
    let s = common::CountingSecrets::default();
    s.inner
        .set(&secret_key(id, PG_PASSWORD), "not-the-password")
        .unwrap();
    Arc::new(s)
}

/// Неверный пароль у сервера: первое подключение — `Failed` с ошибкой узла, фонового опроса нет, и за
/// много интервалов опроса — ни одной новой попытки входа и ни одного чтения keychain.
#[tokio::test]
async fn wrong_password_fails_the_first_connect_and_is_never_retried() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let proxy = common::PausableProxy::start(&addr).await;
    let mut profile = common::server_profile("rt-wrong", &format!("127.0.0.1:{}", proxy.port()));
    profile.pg.ssl_mode = SslMode::Disable;
    let secrets = wrong_password("rt-wrong");
    let (rt, pools) = fast_runtimes(secrets.clone());

    let e = rt.connect(profile).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::AuthFailed, "{e:?}");
    assert_eq!(e.key, "errors.pg.authFailed", "{e:?}");
    let view = rt.view("rt-wrong");
    assert_eq!(view.state, ConnState::Failed);
    assert!(view.topology.is_none() && view.catalog_member.is_none());
    tokio::time::sleep(FAST_POLL * 10).await;
    assert_eq!(proxy.accepted(), 1, "одна попытка входа");
    assert_eq!(secrets.pg_password_reads(), 1, "keychain прочитан один раз");
    assert_eq!(
        rt.target("rt-wrong", None).unwrap_err().key,
        "errors.topology.notConnected"
    );
    rt.shutdown();
    drop(pools);
}

/// Список узлов, где один узел отвергает вход: подключение `degraded`, а фоновый опрос входит на этот узел
/// один раз — дальше отдаёт запомненный отказ, не открывая соединений.
#[tokio::test]
async fn poller_does_not_retry_a_rejected_login() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let rejecting = common::RejectingPg::start().await;
    let mut profile = common::server_profile("rt-mixed", &addr);
    profile.kind = ConnectionKind::HostList;
    profile.target_server_type = glafira_pg_lib::conn::profile::TargetServerType::Any;
    profile.hosts.push(HostPort {
        host: "127.0.0.1".into(),
        port: rejecting.port(),
    });
    profile.pg.ssl_mode = SslMode::Disable;
    let rejected = member_name("127.0.0.1", rejecting.port());
    let (rt, _pools) = fast_runtimes(common::secrets_with_password("rt-mixed"));

    let view = rt.connect(profile).await.unwrap();
    assert_eq!(view.state, ConnState::Degraded);
    let first = view.topology.unwrap();
    let m = first.member(&rejected).unwrap();
    assert!(!m.reachable);
    assert_eq!(m.error.as_ref().unwrap().code, ErrorCode::AuthFailed);

    tokio::time::sleep(FAST_POLL * 10).await;
    assert_eq!(rejecting.accepted(), 1, "одна попытка входа за 10 опросов");
    let v = rt.view("rt-mixed");
    assert_eq!(v.state, ConnState::Degraded);
    let t = v.topology.unwrap();
    assert!(
        t.members.iter().any(|m| m.reachable),
        "живой узел опрашивается"
    );
    assert_eq!(
        t.member(&rejected).unwrap().error.as_ref().unwrap().key,
        "errors.pg.loginNotRetried",
        "отказ запомнен"
    );
    // Переоткрытие подключения (как после сохранения пароля) пробует войти снова.
    rt.disconnect("rt-mixed");
    rt.shutdown();
}

/// Patroni отвечает (верный пароль API), а пароль PostgreSQL неверен: обзор, который раз в 10 с спрашивает
/// сведения об узле, входит в PostgreSQL один раз, дальше получает запомненный отказ.
#[tokio::test]
async fn patroni_member_with_a_wrong_pg_password_is_not_logged_into_again() {
    let Some(mut settings) = common::patroni_settings() else {
        return;
    };
    let proxy = common::PausableProxy::start("localhost:5441").await;
    let o = settings
        .host_overrides
        .iter_mut()
        .find(|o| o.member == "patroni1")
        .unwrap();
    (o.host, o.port) = (Some("127.0.0.1".into()), Some(proxy.port()));
    let mut profile = common::server_profile("rt-pw", "localhost:5441");
    profile.kind = ConnectionKind::Patroni;
    profile.hosts.clear();
    profile.patroni = Some(settings);
    profile.pg.ssl_mode = SslMode::Disable;
    let secrets = wrong_password("rt-pw");
    secrets
        .inner
        .set(
            &secret_key("rt-pw", PATRONI_PASSWORD),
            common::PATRONI_PASSWORD_VALUE,
        )
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let st = AppState::new(dir.path(), secrets.clone()).unwrap();
    st.connections.upsert(profile).unwrap();

    let view = st
        .runtimes
        .connect(st.profile("rt-pw").unwrap())
        .await
        .unwrap();
    assert!(
        matches!(view.state, ConnState::Connected | ConnState::Degraded),
        "{view:?}"
    );
    let first = member_facts(&st, "rt-pw", "patroni1").await.unwrap_err();
    assert_eq!(
        (first.code, first.key.as_str()),
        (ErrorCode::AuthFailed, "errors.pg.authFailed")
    );
    for _ in 0..5 {
        let e = member_facts(&st, "rt-pw", "patroni1").await.unwrap_err();
        assert_eq!(e.key, "errors.pg.loginNotRetried", "{e:?}");
    }
    assert_eq!(proxy.accepted(), 1, "одна попытка входа");
    assert_eq!(secrets.pg_password_reads(), 1, "keychain прочитан один раз");
    st.runtimes.disconnect("rt-pw");
}
