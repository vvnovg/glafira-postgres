//! Проверка подключения по шагам (спец. P1 §10): SSH → Patroni → обнаружение → узлы → готовность.
//! Работает с несохранённым профилем: секреты — форма поверх keychain.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use pg_advisor::readiness::{ReadinessItem, evaluate};
use serde::Serialize;
use tokio::task::AbortHandle;
use tokio_postgres::Client;

use crate::conn::pg::ConnectSpec;
use crate::conn::profile::{
    ConnectionKind, ConnectionProfile, SshAuth, normalize_profile, validate_profile,
};
use crate::conn::run_blocking;
use crate::error::{AppError, AppResult};
use crate::i18n::LocalizedText;
use crate::net::ssh::tunnel::TunnelRegistry;
use crate::readiness::{collect_facts, preload_of};
use crate::secrets::{
    PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, SSH_PASSPHRASE, SSH_PASSWORD, SecretStore, secret_key,
};
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
    Ssh,
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

/// Туннель проверки закрывается, а секрет, положенный для askpass, снимается на любом выходе из проверки
/// (и при её отмене). Если приложение упадёт посреди проверки, секрет снимет следующий запуск: id туннеля
/// записан в `staged-secrets.json` (`TunnelRegistry::stage_secret`).
struct TunnelGuard {
    tunnels: Arc<TunnelRegistry>,
    id: String,
}

impl Drop for TunnelGuard {
    fn drop(&mut self) {
        self.tunnels.close(&self.id);
        // Только временный id: у сохранённого подключения это удалило бы его настоящие секреты из keychain.
        if self.id.starts_with("unsaved-") {
            let (tunnels, id) = (self.tunnels.clone(), self.id.clone());
            let unstage = move || tunnels.unstage_secrets(&id);
            // Keychain — вне потоков tokio.
            match tokio::runtime::Handle::try_current() {
                Ok(h) => {
                    h.spawn_blocking(unstage);
                }
                Err(_) => unstage(),
            }
        }
    }
}

/// Секрет формы (пароль или пассфраза ssh) — туда, где его найдёт askpass туннеля `tunnel_id`. Читается под id
/// профиля: для несохранённого — из формы, для сохранённого — форма поверх keychain.
async fn stage_ssh_secret(
    profile: &ConnectionProfile,
    auth: SshAuth,
    tunnel_id: &str,
    secrets: &Arc<dyn SecretStore>,
    tunnels: &Arc<TunnelRegistry>,
) -> AppResult<()> {
    let field = match auth {
        SshAuth::Password => SSH_PASSWORD,
        SshAuth::Key => SSH_PASSPHRASE,
        SshAuth::Agent => return Ok(()),
    };
    let (s, t) = (secrets.clone(), tunnels.clone());
    let (from, to) = (secret_key(&profile.id, field), tunnel_id.to_string());
    run_blocking(move || {
        if let Some(v) = s.get(&from)?
            && !v.is_empty()
        {
            t.stage_secret(&to, field, &v)?;
        }
        Ok(())
    })
    .await
}

/// Новое соединение с узлом (через туннель проверки, если он есть); задача соединения запоминается, чтобы её
/// можно было прервать.
async fn adhoc_client(
    profile: &ConnectionProfile,
    secrets: &Arc<dyn SecretStore>,
    socks: Option<SocketAddr>,
    host: &str,
    port: u16,
    tasks: &mut Vec<AbortHandle>,
) -> AppResult<Client> {
    let (p, s, h) = (profile.clone(), secrets.clone(), host.to_string());
    let db = profile.pg.database.clone();
    let spec =
        run_blocking(move || ConnectSpec::build_via(&p, &h, port, &db, s.as_ref(), socks)).await?;
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
    socks: Option<SocketAddr>,
    reachable: &[Target],
    target: &str,
    limit: Duration,
) -> AppResult<Vec<ReadinessItem>> {
    let mut tasks = Vec::new();
    let items = readiness_queries(
        profile, secrets, socks, reachable, target, limit, &mut tasks,
    )
    .await;
    if items.is_err() {
        tasks.iter().for_each(AbortHandle::abort);
    }
    items
}

async fn readiness_queries(
    profile: &ConnectionProfile,
    secrets: &Arc<dyn SecretStore>,
    socks: Option<SocketAddr>,
    reachable: &[Target],
    target: &str,
    limit: Duration,
    tasks: &mut Vec<AbortHandle>,
) -> AppResult<Vec<ReadinessItem>> {
    let mut preload = Vec::new();
    let mut target_client = None;
    for (name, host, port) in reachable {
        let c = adhoc_client(profile, secrets, socks, host, *port, tasks).await?;
        // Значение, не видимое роли, в оценку не попадает: чек-лист скажет «неизвестно», а не упадёт.
        if let Some(loaded) = bounded(limit, preload_of(&c)).await? {
            preload.push((name.clone(), loaded));
        }
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
    tunnels: Arc<TunnelRegistry>,
    emit: Emit<'_>,
) -> AppResult<TestReport> {
    normalize_profile(&mut profile);
    validate_profile(&profile)?;
    let failed = |topology: Option<Topology>| TestReport {
        ok: false,
        topology,
        readiness: vec![],
    };

    // 0. SSH: туннель проверки (спец. §5). Всегда под своим временным id `unsaved-…`: проверка сохранённого
    // подключения не трогает ни туннель открытого подключения, ни его секреты в keychain. Секрет формы askpass
    // ищет под этим id — на время проверки он лежит в keychain (или в файле, см. `TunnelRegistry::stage_secret`).
    let mut socks = None;
    let mut _tunnel_guard = None;
    if let Some(ssh) = profile.ssh.clone() {
        event(emit, TestStep::Ssh, None, StepStatus::Running, None, None);
        let mut tunnel_profile = profile.clone();
        tunnel_profile.id = format!("unsaved-{}", uuid::Uuid::new_v4());
        _tunnel_guard = Some(TunnelGuard {
            tunnels: tunnels.clone(),
            id: tunnel_profile.id.clone(),
        });
        let timeout = Duration::from_secs(profile.pg.connect_timeout_s + 5);
        let opened = match stage_ssh_secret(
            &profile,
            ssh.auth,
            &tunnel_profile.id,
            &secrets,
            &tunnels,
        )
        .await
        {
            Ok(()) => tunnels.open(&tunnel_profile, timeout).await,
            Err(e) => Err(e),
        };
        match opened {
            Ok(addr) => {
                socks = Some(addr);
                // Версию узнать не удалось (`ssh -V` молчит или отвечает не по-OpenSSH) — только о туннеле, без
                // пустого места под версию.
                let message = match tunnels.openssh_version().await {
                    Ok((major, minor)) => ltext!(
                        "test.sshOk",
                        "OpenSSH {{version}}, tunnel is up",
                        version = format!("{major}.{minor}")
                    ),
                    Err(_) => ltext!("test.sshOkNoVersion", "Tunnel is up"),
                };
                event(
                    emit,
                    TestStep::Ssh,
                    None,
                    StepStatus::Ok,
                    Some(message),
                    None,
                );
            }
            Err(e) => {
                event(emit, TestStep::Ssh, None, StepStatus::Failed, None, Some(e));
                return Ok(failed(None));
            }
        }
    } else {
        event(
            emit,
            TestStep::Ssh,
            None,
            StepStatus::Skipped,
            Some(ltext!("test.sshNotUsed", "Not used: direct connection")),
            None,
        );
    }

    let nodes = AdhocConnector {
        profile: profile.clone(),
        secrets: secrets.clone(),
        socks,
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
            ReqwestHttp::new(
                settings,
                password,
                key_password.as_deref(),
                socks,
                p.ssh.as_ref().map(|ssh| ssh.target.clone()),
            )
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
                        "{{count}} members, leader {{leader}}",
                        count = t.members.len(),
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
            Some(ltext!(
                "test.patroniNotUsed",
                "Not used: this is not a Patroni cluster"
            )),
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
            "{{count}} nodes found",
            count = topology.members.len()
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
            Some(ltext!(
                "test.readinessNoNode",
                "Not checked: no node accepted the connection"
            )),
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
    match readiness_items(
        &profile,
        &secrets,
        socks,
        &pg_ok,
        &target,
        READINESS_TIMEOUT,
    )
    .await
    {
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
            None,
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

    /// Пропущенный шаг объясняет причину: у сервера Patroni не используется, без доступного узла готовность не проверить.
    #[tokio::test]
    async fn skipped_steps_say_why() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut profile = sample_profile("c");
        profile.hosts[0].host = "127.0.0.1".into();
        profile.hosts[0].port = port;
        profile.pg.ssl_mode = SslMode::Disable;
        profile.pg.connect_timeout_s = 1;
        let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
        let events = std::sync::Mutex::new(Vec::new());
        let emit = |e: TestEvent| events.lock().unwrap().push(e);
        let tunnels = crate::net::ssh::tunnel::unused_registry(Arc::default());
        let report = run(profile, secrets, tunnels, &emit).await.unwrap();
        assert!(!report.ok);
        // Число узлов уходит параметром `count` (по нему фронтенд выбирает форму слова), а не `members`.
        let discovery = events
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.step == TestStep::Discovery)
            .and_then(|e| e.message.clone())
            .expect("шаг обнаружения сообщает число узлов");
        assert_eq!(discovery.key, "test.discoveryOk");
        assert_eq!(discovery.params.get("count").map(String::as_str), Some("1"));
        assert!(!discovery.params.contains_key("members"));
        let skipped: Vec<(TestStep, Option<String>)> = events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.status == StepStatus::Skipped)
            .map(|e| (e.step, e.message.as_ref().map(|m| m.key.clone())))
            .collect();
        assert_eq!(
            skipped,
            [
                (TestStep::Ssh, Some("test.sshNotUsed".to_string())),
                (TestStep::Patroni, Some("test.patroniNotUsed".to_string())),
                (
                    TestStep::Readiness,
                    Some("test.readinessNoNode".to_string())
                ),
            ]
        );
    }

    /// Keychain в памяти, который записывает обращения на запись: `set <ключ>` / `delete <ключ>`.
    #[derive(Default)]
    struct Keychain {
        inner: MemoryStore,
        writes: std::sync::Mutex<Vec<String>>,
    }

    impl SecretStore for Keychain {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            self.inner.get(key)
        }
        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            self.writes.lock().unwrap().push(format!("set {key}"));
            self.inner.set(key, value)
        }
        fn delete(&self, key: &str) -> AppResult<()> {
            self.writes.lock().unwrap().push(format!("delete {key}"));
            self.inner.delete(key)
        }
    }

    /// Шаг SSH: туннель проверки — под временным id, секрет формы лежит в keychain только на время проверки, а
    /// сохранённый секрет подключения и туннель открытого подключения не тронуты. Узел за бастионом недоступен —
    /// сбой проброса на шаге узла.
    #[tokio::test]
    async fn ssh_step_uses_a_temporary_tunnel_and_cleans_up() {
        use crate::net::ssh::tunnel::tests::{UNREACHABLE, profile as ssh_profile, registry_with};
        let dir = tempfile::tempdir().unwrap();
        let keychain = Arc::new(Keychain::default());
        keychain
            .inner
            .set(&secret_key("c1", SSH_PASSWORD), "saved-pw")
            .unwrap();
        let (tunnels, _) = registry_with(
            dir.path(),
            vec![("FAKE_SSH_UPSTREAM", UNREACHABLE.into())],
            keychain.clone(),
            |_| {},
        );
        // Открытое подключение c1 со своим туннелем: проверка его не закрывает.
        let mut profile = ssh_profile(SshAuth::Password);
        profile.pg.ssl_mode = SslMode::Disable;
        profile.hosts[0].host = "pg-single".into();
        let live = tunnels
            .open(&profile, Duration::from_secs(5))
            .await
            .unwrap();
        // Форма: новый пароль ssh поверх сохранённого.
        let mut overrides = std::collections::HashMap::new();
        overrides.insert(secret_key("c1", SSH_PASSWORD), "form-pw".to_string());
        let overlay: Arc<dyn SecretStore> = Arc::new(crate::secrets::OverlayStore {
            base: keychain.clone(),
            overrides,
        });
        let events = std::sync::Mutex::new(Vec::new());
        let staged = std::sync::Mutex::new(Vec::new());
        let emit = |e: TestEvent| {
            if e.step == TestStep::Ssh && e.status == StepStatus::Ok {
                // Пока туннель проверки открыт, секрет формы лежит под временным id.
                let writes = keychain.writes.lock().unwrap().clone();
                for w in writes {
                    let key = w.trim_start_matches("set ").to_string();
                    staged
                        .lock()
                        .unwrap()
                        .push((key.clone(), keychain.inner.get(&key).unwrap()));
                }
            }
            events.lock().unwrap().push(e);
        };
        let report = run(profile.clone(), overlay, tunnels.clone(), &emit)
            .await
            .unwrap();
        assert!(!report.ok);
        let events = events.into_inner().unwrap();
        let ssh: Vec<_> = events.iter().filter(|e| e.step == TestStep::Ssh).collect();
        assert_eq!(ssh.len(), 2, "{events:?}");
        let ok = ssh[1].message.as_ref().unwrap();
        assert_eq!(
            (ok.key.as_str(), ok.params["version"].as_str()),
            ("test.sshOk", "9.9")
        );
        let node = events
            .iter()
            .find(|e| e.step == TestStep::Node && e.status == StepStatus::Failed)
            .expect("узел за бастионом недоступен");
        assert_eq!(
            node.error.as_ref().unwrap().key,
            "errors.ssh.forwardFailed",
            "{node:?}"
        );
        let staged = staged.into_inner().unwrap();
        assert_eq!(staged.len(), 1, "{staged:?}");
        assert!(
            staged[0].0.starts_with("unsaved-") && staged[0].0.ends_with("/sshPassword"),
            "{staged:?}"
        );
        assert_eq!(staged[0].1.as_deref(), Some("form-pw"));
        // После проверки: туннель проверки закрыт, временный секрет снят, сохранённый — прежний.
        assert_eq!(tunnels.open_ids(), ["c1".to_string()]);
        assert_eq!(tunnels.route("c1").unwrap(), Some(live));
        let deadline = Instant::now() + Duration::from_secs(5);
        while keychain.inner.get(&staged[0].0).unwrap().is_some() {
            assert!(Instant::now() < deadline, "временный секрет не снят");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            keychain
                .inner
                .get(&secret_key("c1", SSH_PASSWORD))
                .unwrap()
                .as_deref(),
            Some("saved-pw")
        );
        assert!(
            !keychain
                .writes
                .lock()
                .unwrap()
                .iter()
                .any(|w| w.contains("c1/")),
            "записи сохранённого подключения не трогались"
        );
        tunnels.close("c1");
    }

    /// Версию OpenSSH узнать не удалось, а туннель поднялся: шаг SSH говорит только о туннеле, без пустой версии.
    #[tokio::test]
    async fn ssh_step_without_a_known_version_says_only_that_the_tunnel_is_up() {
        use crate::net::ssh::tunnel::tests::{UNREACHABLE, profile as ssh_profile, registry};
        let dir = tempfile::tempdir().unwrap();
        let (tunnels, _) = registry(
            dir.path(),
            vec![
                ("FAKE_SSH_VERSION", "not an OpenSSH client".into()),
                ("FAKE_SSH_UPSTREAM", UNREACHABLE.into()),
            ],
        );
        let mut profile = ssh_profile(SshAuth::Agent);
        profile.pg.ssl_mode = SslMode::Disable;
        let events = std::sync::Mutex::new(Vec::new());
        let emit = |e: TestEvent| events.lock().unwrap().push(e);
        run(
            profile,
            Arc::new(MemoryStore::default()),
            tunnels.clone(),
            &emit,
        )
        .await
        .unwrap();
        let events = events.into_inner().unwrap();
        let ok = events
            .iter()
            .find(|e| e.step == TestStep::Ssh && e.status == StepStatus::Ok)
            .and_then(|e| e.message.clone())
            .expect("туннель поднят");
        assert_eq!(ok.key, "test.sshOkNoVersion");
        assert!(ok.params.is_empty(), "{ok:?}");
        assert!(tunnels.open_ids().is_empty());
    }

    /// Туннель не поднялся — проверка останавливается на шаге SSH с его ошибкой.
    #[tokio::test]
    async fn failed_tunnel_stops_the_check_at_the_ssh_step() {
        use crate::net::ssh::tunnel::tests::{profile as ssh_profile, registry};
        let dir = tempfile::tempdir().unwrap();
        let (tunnels, _) = registry(dir.path(), vec![("FAKE_SSH_MODE", "auth".into())]);
        let mut profile = ssh_profile(SshAuth::Agent);
        profile.id = "unsaved-x".into();
        let events = std::sync::Mutex::new(Vec::new());
        let emit = |e: TestEvent| events.lock().unwrap().push(e);
        let report = run(
            profile,
            Arc::new(MemoryStore::default()),
            tunnels.clone(),
            &emit,
        )
        .await
        .unwrap();
        assert!(!report.ok);
        let events = events.into_inner().unwrap();
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(
            (events[1].step, events[1].status),
            (TestStep::Ssh, StepStatus::Failed)
        );
        assert_eq!(
            events[1].error.as_ref().unwrap().key,
            "errors.ssh.authFailed"
        );
        assert!(tunnels.open_ids().is_empty());
    }

    #[tokio::test]
    async fn unreachable_target_is_an_error_not_a_panic() {
        let profile = sample_profile("c");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
        let e = readiness_items(&profile, &secrets, None, &[], "m", Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(e.code, crate::error::ErrorCode::Internal);
    }
}
