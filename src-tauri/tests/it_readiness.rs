//! Проверка подключения против стенда: шаги и чек-лист.

mod common;

use std::sync::{Arc, Mutex};

use glafira_pg_lib::conn::profile::ConnectionKind;
use glafira_pg_lib::readiness::check::{StepStatus, TestEvent, TestStep, run};
use glafira_pg_lib::secrets::{PATRONI_PASSWORD, SecretStore, secret_key};
use pg_advisor::readiness::Status;

fn status_of(report: &glafira_pg_lib::readiness::check::TestReport, id: &str) -> Status {
    report.readiness.iter().find(|i| i.id == id).unwrap().status
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
        &|e| events.lock().unwrap().push(e),
    )
    .await
    .unwrap();
    assert!(report.ok);
    assert_eq!(status_of(&report, "monitorRole"), Status::Ok);
    assert_eq!(status_of(&report, "signalRole"), Status::Ok);
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
    let report = run(p, secrets, &|e| events.lock().unwrap().push(e))
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
        &|e| events.lock().unwrap().push(e),
    )
    .await
    .unwrap();
    assert!(!report.ok);
    let ev = events.lock().unwrap();
    let node = ev
        .iter()
        .find(|e| e.step == TestStep::Node && e.status == StepStatus::Failed)
        .unwrap();
    assert_eq!(
        node.error.as_ref().unwrap().code,
        glafira_pg_lib::error::ErrorCode::AuthFailed
    );
}
