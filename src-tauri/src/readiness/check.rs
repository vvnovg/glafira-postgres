//! Проверка подключения по шагам (спец. P1 §10): Patroni → обнаружение → узлы → готовность.
//! Работает с несохранённым профилем: секреты — форма поверх keychain.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use pg_advisor::readiness::{ReadinessItem, evaluate};
use serde::Serialize;
use tokio::task::AbortHandle;
use tokio_postgres::Client;

use crate::conn::pg::ConnectSpec;
use crate::conn::profile::{
    ConnectionKind, ConnectionProfile, normalize_profile, validate_profile,
};
use crate::conn::run_blocking;
use crate::error::{AppError, AppResult};
use crate::i18n::LocalizedText;
use crate::readiness::{collect_facts, preload_of};
use crate::secrets::{PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, SecretStore, secret_key};
use crate::topology::connector::{AdhocConnector, NodeConnector};
use crate::topology::model::{Topology, now_ms};
use crate::topology::observe::default_member;
use crate::topology::patroni::{ReqwestHttp, discover};
use crate::topology::probe::{COLLECT_TIMEOUT, no_answer, observe_hosts};

/// Предел на запросы чек-листа к одному узлу: молчащий узел (оборванный NAT, пропавший туннель)
/// не должен подвесить проверку подключения.
const READINESS_TIMEOUT: Duration = COLLECT_TIMEOUT;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TestStep {
    Patroni,
    Discovery,
    Node,
    Readiness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StepStatus {
    Running,
    Ok,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestEvent {
    pub step: TestStep,
    pub member: Option<String>,
    pub status: StepStatus,
    pub message: Option<LocalizedText>,
    pub error: Option<AppError>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestReport {
    pub ok: bool,
    pub topology: Option<Topology>,
    pub readiness: Vec<ReadinessItem>,
}

type Emit<'a> = &'a (dyn Fn(TestEvent) + Send + Sync);

fn event(
    emit: Emit,
    step: TestStep,
    member: Option<&str>,
    status: StepStatus,
    message: Option<LocalizedText>,
    error: Option<AppError>,
) {
    emit(TestEvent {
        step,
        member: member.map(String::from),
        status,
        message,
        error,
    });
}

/// Узел, на котором проверяется готовность: имя участника, адрес и порт.
type Target = (String, String, u16);

/// Запрос чек-листа не дольше `limit`.
async fn bounded<T>(limit: Duration, query: impl Future<Output = AppResult<T>>) -> AppResult<T> {
    tokio::time::timeout(limit, query)
        .await
        .unwrap_or_else(|_| Err(no_answer(limit)))
}

/// Новое соединение с узлом; задача соединения запоминается, чтобы её можно было прервать.
async fn adhoc_client(
    profile: &ConnectionProfile,
    secrets: &Arc<dyn SecretStore>,
    host: &str,
    port: u16,
    tasks: &mut Vec<AbortHandle>,
) -> AppResult<Client> {
    let (p, s, h) = (profile.clone(), secrets.clone(), host.to_string());
    let db = profile.pg.database.clone();
    let spec = run_blocking(move || ConnectSpec::build(&p, &h, port, &db, s.as_ref())).await?;
    let (client, task) = spec.connect_with_handle().await?;
    tasks.push(task);
    Ok(client)
}

/// `pg_stat_statements` в `shared_preload_libraries` — на каждом доступном узле, остальные факты — на
/// целевом; каждый запрос — не дольше `limit`. Клиенты закрываются при выходе; если проверка не
/// удалась (узел молчит, обрыв, ошибка запроса), задачи соединений прерываются: drop клиента с
/// неотвеченным запросом сокет не закрывает.
async fn readiness_items(
    profile: &ConnectionProfile,
    secrets: &Arc<dyn SecretStore>,
    reachable: &[Target],
    target: &str,
    limit: Duration,
) -> AppResult<Vec<ReadinessItem>> {
    let mut tasks = Vec::new();
    let items = readiness_queries(profile, secrets, reachable, target, limit, &mut tasks).await;
    if items.is_err() {
        tasks.iter().for_each(AbortHandle::abort);
    }
    items
}

async fn readiness_queries(
    profile: &ConnectionProfile,
    secrets: &Arc<dyn SecretStore>,
    reachable: &[Target],
    target: &str,
    limit: Duration,
    tasks: &mut Vec<AbortHandle>,
) -> AppResult<Vec<ReadinessItem>> {
    let mut preload = Vec::new();
    let mut target_client = None;
    for (name, host, port) in reachable {
        let c = adhoc_client(profile, secrets, host, *port, tasks).await?;
        preload.push((name.clone(), bounded(limit, preload_of(&c)).await?));
        if name == target {
            target_client = Some(c);
        }
    }
    let Some(c) = target_client else {
        return Err(AppError::internal_with("readiness target is not reachable"));
    };
    let facts = bounded(limit, collect_facts(&c, preload, &profile.pg.database)).await?;
    Ok(evaluate(&facts))
}

pub async fn run(
    mut profile: ConnectionProfile,
    secrets: Arc<dyn SecretStore>,
    emit: Emit<'_>,
) -> AppResult<TestReport> {
    normalize_profile(&mut profile);
    validate_profile(&profile)?;
    let failed = |topology: Option<Topology>| TestReport {
        ok: false,
        topology,
        readiness: vec![],
    };
    let nodes = AdhocConnector {
        profile: profile.clone(),
        secrets: secrets.clone(),
    };

    // 1–2. Patroni или опрос узлов, затем итог обнаружения.
    let topology = if profile.kind == ConnectionKind::Patroni {
        event(
            emit,
            TestStep::Patroni,
            None,
            StepStatus::Running,
            None,
            None,
        );
        let (p, s) = (profile.clone(), secrets.clone());
        let http = run_blocking(move || {
            let settings = p.patroni.as_ref().expect("checked by validate_profile");
            let password = s.get(&secret_key(&p.id, PATRONI_PASSWORD))?;
            let key_password = s.get(&secret_key(&p.id, PATRONI_KEY_PASSWORD))?;
            ReqwestHttp::new(settings, password, key_password.as_deref())
        })
        .await;
        let http = match http {
            Ok(h) => h,
            Err(e) => {
                event(
                    emit,
                    TestStep::Patroni,
                    None,
                    StepStatus::Failed,
                    None,
                    Some(e),
                );
                return Ok(failed(None));
            }
        };
        let settings = profile
            .patroni
            .as_ref()
            .expect("checked by validate_profile");
        match discover(&http, settings, &profile.id, None, now_ms()).await {
            Ok(t) => {
                let leader = t
                    .leader()
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| "—".into());
                event(
                    emit,
                    TestStep::Patroni,
                    None,
                    StepStatus::Ok,
                    Some(ltext!(
                        "test.patroniOk",
                        "{{members}} members, leader {{leader}}",
                        members = t.members.len(),
                        leader = leader
                    )),
                    None,
                );
                t
            }
            Err(e) => {
                event(
                    emit,
                    TestStep::Patroni,
                    None,
                    StepStatus::Failed,
                    None,
                    Some(e),
                );
                return Ok(failed(None));
            }
        }
    } else {
        event(
            emit,
            TestStep::Patroni,
            None,
            StepStatus::Skipped,
            None,
            None,
        );
        observe_hosts(&profile.id, profile.kind, &profile.hosts, &nodes, now_ms()).await
    };
    event(
        emit,
        TestStep::Discovery,
        None,
        StepStatus::Ok,
        Some(ltext!(
            "test.discoveryOk",
            "{{members}} nodes found",
            members = topology.members.len()
        )),
        None,
    );

    // 3. Узлы: у Patroni — отдельное соединение с PostgreSQL каждого участника, у списка — итог опроса.
    let mut pg_ok: Vec<Target> = Vec::new();
    for m in &topology.members {
        event(
            emit,
            TestStep::Node,
            Some(&m.name),
            StepStatus::Running,
            None,
            None,
        );
        let facts = if profile.kind == ConnectionKind::Patroni {
            nodes.facts(&m.name, &m.host, m.port).await.map(|f| {
                (
                    if f.in_recovery { "replica" } else { "primary" },
                    f.server_version,
                )
            })
        } else if m.reachable {
            Ok((
                if m.role.is_standby() {
                    "replica"
                } else {
                    "primary"
                },
                m.server_version.clone().unwrap_or_default(),
            ))
        } else {
            Err(m
                .error
                .clone()
                .unwrap_or_else(|| AppError::internal_with("node unreachable")))
        };
        match facts {
            Ok((role, version)) => {
                pg_ok.push((m.name.clone(), m.host.clone(), m.port));
                event(
                    emit,
                    TestStep::Node,
                    Some(&m.name),
                    StepStatus::Ok,
                    Some(ltext!(
                        "test.nodeOk",
                        "{{role}}, PostgreSQL {{version}}",
                        role = role,
                        version = version
                    )),
                    None,
                );
            }
            Err(e) => event(
                emit,
                TestStep::Node,
                Some(&m.name),
                StepStatus::Failed,
                None,
                Some(e),
            ),
        }
    }
    let all_nodes_ok = pg_ok.len() == topology.members.len();

    // 4. Готовность — на узле по умолчанию (или любом доступном).
    let target = default_member(&profile, &topology)
        .and_then(|name| pg_ok.iter().find(|(n, _, _)| *n == name).cloned())
        .or_else(|| pg_ok.first().cloned());
    let Some((target, _, _)) = target else {
        event(
            emit,
            TestStep::Readiness,
            None,
            StepStatus::Skipped,
            None,
            None,
        );
        return Ok(failed(Some(topology)));
    };
    event(
        emit,
        TestStep::Readiness,
        None,
        StepStatus::Running,
        None,
        None,
    );
    match readiness_items(&profile, &secrets, &pg_ok, &target, READINESS_TIMEOUT).await {
        Ok(items) => {
            event(emit, TestStep::Readiness, None, StepStatus::Ok, None, None);
            Ok(TestReport {
                ok: all_nodes_ok,
                topology: Some(topology),
                readiness: items,
            })
        }
        Err(e) => {
            event(
                emit,
                TestStep::Readiness,
                None,
                StepStatus::Failed,
                None,
                Some(e),
            );
            Ok(failed(Some(topology)))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::conn::profile::{SslMode, sample_profile};
    use crate::conn::testutil;
    use crate::secrets::MemoryStore;

    const WAIT: Duration = Duration::from_secs(2);

    /// Узел принял соединение и молчит: проверка готовности не виснет, а соединение закрывается.
    #[tokio::test]
    async fn stalled_node_does_not_hang_the_readiness_step() {
        let server = testutil::stalling_pg_server();
        let mut profile = sample_profile("c");
        profile.pg.ssl_mode = SslMode::Disable;
        profile.pg.connect_timeout_s = 1;
        let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
        let reachable = vec![("m".to_string(), "127.0.0.1".to_string(), server.port)];
        let started = Instant::now();
        let e = readiness_items(
            &profile,
            &secrets,
            &reachable,
            "m",
            Duration::from_millis(300),
        )
        .await
        .unwrap_err();
        assert_eq!(e.key, "errors.pg.noAnswer");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(server.wait_closed(1, WAIT).await, "сокет закрыт");
    }

    #[tokio::test]
    async fn unreachable_target_is_an_error_not_a_panic() {
        let profile = sample_profile("c");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
        let e = readiness_items(&profile, &secrets, &[], "m", Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(e.code, crate::error::ErrorCode::Internal);
    }
}
