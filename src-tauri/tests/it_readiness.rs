//! Проверка подключения против стенда: шаги и чек-лист.

mod common;

use std::sync::{Arc, Mutex};

use glafira_pg_lib::conn::pg::ConnectSpec;
use glafira_pg_lib::conn::profile::ConnectionKind;
use glafira_pg_lib::readiness::check::{StepStatus, TestEvent, TestStep, run};
use glafira_pg_lib::secrets::{
    MemoryStore, PATRONI_PASSWORD, PG_PASSWORD, SecretStore, secret_key,
};
use pg_advisor::readiness::Status;

fn no_tunnels() -> Arc<glafira_pg_lib::net::ssh::tunnel::TunnelRegistry> {
    common::no_tunnels(Default::default())
}

fn status_of(report: &glafira_pg_lib::readiness::check::TestReport, id: &str) -> Status {
    report.readiness.iter().find(|i| i.id == id).unwrap().status
}

const PLAIN_USER: &str = "glafira_plain";
const PLAIN_PASSWORD: &str = "plain-secret";

/// Роль без `pg_monitor` и `pg_signal_backend`: создаётся (и приводится в исходное состояние) от имени
/// суперпользователя стенда. Сессия диагностики по умолчанию только для чтения, поэтому запись включается явно.
async fn ensure_plain_role(addr: &str) {
    let mut p = common::server_profile("su", addr);
    p.pg.username = "postgres".into();
    let s = MemoryStore::default();
    s.set(&secret_key("su", PG_PASSWORD), "postgres-secret")
        .unwrap();
    let (host, port) = common::host_port(addr);
    let c = ConnectSpec::build(&p, &host, port, "postgres", &s)
        .unwrap()
        .connect()
        .await
        .unwrap();
    // Отдельным запросом: многооператорная строка — одна неявная транзакция, режим которой уже выбран.
    c.batch_execute("SET default_transaction_read_only = off")
        .await
        .unwrap();
    c.batch_execute(&format!(
        "DO $$ BEGIN \
           IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '{PLAIN_USER}') THEN \
             CREATE ROLE {PLAIN_USER} LOGIN PASSWORD '{PLAIN_PASSWORD}'; \
           END IF; \
         END $$; \
         REVOKE pg_monitor, pg_signal_backend FROM {PLAIN_USER}; \
         ALTER ROLE {PLAIN_USER} NOREPLICATION;"
    ))
    .await
    .unwrap();
}

#[tokio::test]
async fn single_server_without_pg_stat_statements() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let events = Mutex::new(Vec::<TestEvent>::new());
    let report = run(
        common::server_profile("t", &addr),
        common::secrets_with_password("t"),
        no_tunnels(),
        &|e| events.lock().unwrap().push(e),
    )
    .await
    .unwrap();
    assert!(report.ok);
    assert_eq!(status_of(&report, "monitorRole"), Status::Ok);
    assert_eq!(status_of(&report, "signalRole"), Status::Ok);
    // glafira_diag без атрибута REPLICATION: удалять слоты он не сможет — предупреждение с готовым исправлением.
    assert_eq!(status_of(&report, "replicationAttr"), Status::Warn);
    assert_eq!(
        report
            .readiness
            .iter()
            .find(|i| i.id == "replicationAttr")
            .unwrap()
            .fix
            .as_deref(),
        Some("ALTER ROLE \"glafira_diag\" REPLICATION;")
    );
    assert_eq!(status_of(&report, "pgssPreloaded"), Status::Missing);
    assert_eq!(status_of(&report, "pgssInstalled"), Status::Missing);
    let ev = events.lock().unwrap();
    assert!(
        ev.iter()
            .any(|e| e.step == TestStep::Patroni && e.status == StepStatus::Skipped)
    );
    assert!(
        ev.iter()
            .any(|e| e.step == TestStep::Node && e.status == StepStatus::Ok)
    );
}

/// Спек P1 §10: роль без `pg_monitor` — это тот случай, ради которого чек-лист и нужен. Проверка не
/// падает целиком (ни `shared_preload_libraries`, ни другие факты не должны требовать привилегий), а
/// показывает «Роль мониторинга — missing» с готовым `GRANT`.
#[tokio::test]
async fn role_without_pg_monitor_gets_a_checklist() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    ensure_plain_role(&addr).await;
    let mut profile = common::server_profile("plain", &addr);
    profile.pg.username = PLAIN_USER.into();
    let secrets = MemoryStore::default();
    secrets
        .set(&secret_key("plain", PG_PASSWORD), PLAIN_PASSWORD)
        .unwrap();
    let events = Mutex::new(Vec::<TestEvent>::new());
    let report = run(profile, Arc::new(secrets), no_tunnels(), &|e| {
        events.lock().unwrap().push(e)
    })
    .await
    .unwrap();
    let ev = events.lock().unwrap();
    assert!(report.ok, "{ev:?}");
    assert!(
        ev.iter()
            .any(|e| e.step == TestStep::Readiness && e.status == StepStatus::Ok),
        "{ev:?}"
    );
    assert!(!report.readiness.is_empty(), "{ev:?}");
    let item = |id: &str| report.readiness.iter().find(|i| i.id == id).unwrap();
    assert_eq!(status_of(&report, "monitorRole"), Status::Missing);
    assert_eq!(
        item("monitorRole").fix.as_deref(),
        Some("GRANT pg_monitor TO \"glafira_plain\";")
    );
    assert_eq!(status_of(&report, "signalRole"), Status::Missing);
    assert_eq!(
        item("signalRole").fix.as_deref(),
        Some("GRANT pg_signal_backend TO \"glafira_plain\";")
    );
    assert_eq!(status_of(&report, "replicationAttr"), Status::Warn);
    assert_eq!(
        item("replicationAttr").fix.as_deref(),
        Some("ALTER ROLE \"glafira_plain\" REPLICATION;")
    );
    // Значение shared_preload_libraries роли не видно: «неизвестно», а не ошибка.
    assert_eq!(status_of(&report, "pgssPreloaded"), Status::Warn);
    assert_eq!(item("pgssPreloaded").params["reason"], "unknown");
    assert_eq!(status_of(&report, "pgssInstalled"), Status::Missing);
}

/// Чек-лист под произвольной ролью стенда (id профиля `rp`), пароль — в памяти.
async fn checklist_as(
    addr: &str,
    user: &str,
    password: &str,
) -> glafira_pg_lib::readiness::check::TestReport {
    let mut profile = common::server_profile("rp", addr);
    profile.pg.username = user.into();
    let secrets = MemoryStore::default();
    secrets
        .set(&secret_key("rp", PG_PASSWORD), password)
        .unwrap();
    let report = run(profile, Arc::new(secrets), no_tunnels(), &|_| {})
        .await
        .unwrap();
    assert!(report.ok);
    report
}

/// Суперпользователю отдельный атрибут не нужен; обычная роль с атрибутом `REPLICATION` — тоже готова.
#[tokio::test]
async fn replication_attribute_item_follows_the_role() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let report = checklist_as(&addr, common::SUPERUSER, common::SUPERUSER_PASSWORD).await;
    assert_eq!(status_of(&report, "replicationAttr"), Status::Ok);
    ensure_plain_role(&addr).await;
    let a = addr.clone();
    let _restore = common::restore_on_drop(move || {
        common::run_sql_blocking(
            &a,
            "postgres",
            &format!("ALTER ROLE {PLAIN_USER} NOREPLICATION"),
        );
    });
    common::run_sql_blocking(
        &addr,
        "postgres",
        &format!("ALTER ROLE {PLAIN_USER} REPLICATION"),
    );
    let report = checklist_as(&addr, PLAIN_USER, PLAIN_PASSWORD).await;
    assert_eq!(status_of(&report, "replicationAttr"), Status::Ok);
}

#[tokio::test]
async fn patroni_cluster_is_ready() {
    let Some(settings) = common::patroni_settings() else {
        return;
    };
    let mut p = common::server_profile("tp", "localhost:5441");
    p.kind = ConnectionKind::Patroni;
    p.hosts.clear();
    p.patroni = Some(settings);
    let secrets = common::secrets_with_password("tp");
    secrets
        .set(
            &secret_key("tp", PATRONI_PASSWORD),
            common::PATRONI_PASSWORD_VALUE,
        )
        .unwrap();
    let events = Mutex::new(Vec::<TestEvent>::new());
    let secrets: Arc<dyn SecretStore> = secrets;
    let report = run(p, secrets, no_tunnels(), &|e| {
        events.lock().unwrap().push(e)
    })
    .await
    .unwrap();
    assert!(report.ok, "{:?}", events.lock().unwrap());
    assert_eq!(status_of(&report, "pgssPreloaded"), Status::Ok);
    assert_eq!(status_of(&report, "pgssInstalled"), Status::Ok);
    assert_eq!(status_of(&report, "trackIoTiming"), Status::Ok);
    let ev = events.lock().unwrap();
    assert_eq!(
        ev.iter()
            .filter(|e| e.step == TestStep::Node && e.status == StepStatus::Ok)
            .count(),
        3
    );
}

#[tokio::test]
async fn wrong_password_fails_node_step() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let secrets = glafira_pg_lib::secrets::MemoryStore::default();
    secrets
        .set(
            &secret_key("w", glafira_pg_lib::secrets::PG_PASSWORD),
            "wrong",
        )
        .unwrap();
    let events = Mutex::new(Vec::<TestEvent>::new());
    let report = run(
        common::server_profile("w", &addr),
        Arc::new(secrets),
        no_tunnels(),
        &|e| events.lock().unwrap().push(e),
    )
    .await
    .unwrap();
    assert!(!report.ok);
    assert!(report.readiness.is_empty());
    let ev = events.lock().unwrap();
    assert!(
        ev.iter()
            .any(|e| e.step == TestStep::Readiness && e.status == StepStatus::Skipped),
        "{ev:?}"
    );
    assert!(
        !ev.iter()
            .any(|e| e.step == TestStep::Readiness && e.status == StepStatus::Running),
        "{ev:?}"
    );
    let node = ev
        .iter()
        .find(|e| e.step == TestStep::Node && e.status == StepStatus::Failed)
        .unwrap();
    assert_eq!(
        node.error.as_ref().unwrap().code,
        glafira_pg_lib::error::ErrorCode::AuthFailed
    );
}
