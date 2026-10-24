//! Подключения через bastion стенда (`GPG_IT_BASTION=localhost:2222`, `GPG_IT_SSH_KEYS=<путь к dev/ssh/keys>`):
//! ключ, ключ с пассфразой и пароль через askpass, неизвестный ключ хоста и доверие, неверный пароль,
//! недоступный узел за бастионом, Patroni и список узлов через туннель. Имена узлов (`pg-single`, `patroni1`…)
//! разрешает бастион. Тесты запускают настоящий OpenSSH из PATH.
//!
//! Запускать с `--test-threads=1`: туннели занимают свободные порты и делят каталог подготовленных секретов.
//! Нужны обе переменные, `GPG_IT_BASTION` и `GPG_IT_SSH_KEYS`: без любой из них каждый тест пропускается.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use glafira_pg_lib::conn::pg::ConnectSpec;
use glafira_pg_lib::conn::pool::PoolRegistry;
use glafira_pg_lib::conn::profile::{
    ConnectionKind, ConnectionProfile, HostPort, PatroniSettings, SshAuth, SslMode,
};
use glafira_pg_lib::error::{AppError, ErrorCode};
use glafira_pg_lib::net::ssh::tunnel::TunnelRegistry;
use glafira_pg_lib::secrets::{SSH_PASSPHRASE, SSH_PASSWORD};
use glafira_pg_lib::topology::connector::AdhocConnector;
use glafira_pg_lib::topology::model::Role;
use glafira_pg_lib::topology::patroni::{PatroniHttp, ReqwestHttp, discover};
use glafira_pg_lib::topology::probe::observe_hosts;

const TIMEOUT: Duration = Duration::from_secs(20);

struct Setup {
    /// Временный каталог: данные туннеля, логи ssh и свой `known_hosts`.
    dir: tempfile::TempDir,
    reg: Arc<TunnelRegistry>,
    profile: ConnectionProfile,
}

fn setup(auth: SshAuth) -> Option<Setup> {
    let ssh = common::bastion_settings(auth)?;
    let dir = tempfile::tempdir().unwrap();
    let reg = common::tunnel_registry_for_tests(dir.path(), Arc::new(PoolRegistry::default()));
    let mut profile = common::server_profile("ssh", "pg-single:5432");
    profile.ssh = Some(ssh);
    Some(Setup { dir, reg, profile })
}

/// Первое подключение к bastion: ключ хоста неизвестен (свой `known_hosts` пуст) — `hostKeyUnknown` с отпечатком;
/// после доверия отпечатку ключ хоста попадает в `known_hosts` и следующий `open` проходит.
async fn trust_host_key(s: &Setup) {
    let e = s.reg.open(&s.profile, TIMEOUT).await.unwrap_err();
    assert_eq!(
        (e.code, e.params.get("kind").map(String::as_str)),
        (ErrorCode::SshError, Some("hostKeyUnknown")),
        "{e:?}"
    );
    let fingerprint = &e.params["fingerprint"];
    assert!(fingerprint.starts_with("SHA256:"), "{e:?}");
    s.reg
        .trust(&s.profile.ssh.as_ref().unwrap().target, fingerprint);
}

async fn open_trusting(s: &Setup) -> SocketAddr {
    trust_host_key(s).await;
    s.reg.open(&s.profile, TIMEOUT).await.unwrap()
}

/// Ключ хоста записал сам ssh в `known_hosts` теста (имя может быть хешировано — проверяем тип ключа).
fn assert_host_key_recorded(s: &Setup) {
    let known = std::fs::read_to_string(s.dir.path().join("known_hosts")).unwrap();
    assert!(known.contains("ssh-ed25519"), "known_hosts: {known:?}");
}

fn spec_via(p: &ConnectionProfile, host: &str, port: u16, socks: SocketAddr) -> ConnectSpec {
    let secrets = common::secrets_with_password(&p.id);
    ConnectSpec::build_via(p, host, port, "postgres", secrets.as_ref(), Some(socks)).unwrap()
}

async fn query_version(socks: SocketAddr, p: &ConnectionProfile) -> String {
    let c = spec_via(p, "pg-single", 5432, socks)
        .connect()
        .await
        .unwrap();
    c.query_one("SELECT version()", &[])
        .await
        .unwrap()
        .get::<_, String>(0)
}

/// Недоступный с бастиона узел — сбой проброса (`forwardFailed`), а не ошибка PostgreSQL или Patroni. Форма ошибки
/// зависит от клиента OpenSSH: новый (10.x) отвечает SOCKS5 только после открытия канала и при отказе закрывает
/// соединение посреди рукопожатия (`errors.ssh.socks`, узел — в `params`); старый отвечает успехом сразу и закрывает
/// поток позже (`errors.ssh.forwardFailed`, узел — в `details`). Вид один и тот же.
fn assert_forward_failed(e: &AppError, host: &str, port: u16) {
    assert_eq!(
        (e.code, e.params.get("kind").map(String::as_str)),
        (ErrorCode::SshError, Some("forwardFailed")),
        "{e:?}"
    );
    let in_params = e.params.get("host").map(String::as_str) == Some(host)
        && e.params.get("port") == Some(&port.to_string());
    let in_details = e
        .details
        .as_deref()
        .is_some_and(|d| d.contains(&format!("{host}:{port}")));
    assert!(
        in_params || in_details,
        "узел {host}:{port} не назван: {e:?}"
    );
}

#[tokio::test]
async fn key_without_passphrase_opens_a_tunnel_to_a_single_server() {
    let Some(s) = setup(SshAuth::Key) else { return };
    let socks = open_trusting(&s).await;
    assert_host_key_recorded(&s);
    assert!(
        query_version(socks, &s.profile)
            .await
            .contains("PostgreSQL 15")
    );
    // Тот же туннель при повторном открытии: живой процесс, тот же порт.
    assert_eq!(s.reg.open(&s.profile, TIMEOUT).await.unwrap(), socks);
    s.reg.close("ssh");
}

#[tokio::test]
async fn key_with_passphrase_uses_askpass() {
    let Some(mut s) = setup(SshAuth::Key) else {
        return;
    };
    s.profile.ssh.as_mut().unwrap().key_path = Some(
        common::ssh_keys_dir()
            .unwrap()
            .join("tester_ed25519_pass")
            .to_string_lossy()
            .into_owned(),
    );
    s.reg
        .stage_secret("ssh", SSH_PASSPHRASE, "test-passphrase")
        .unwrap();
    let socks = open_trusting(&s).await;
    assert!(
        query_version(socks, &s.profile)
            .await
            .contains("PostgreSQL")
    );
    s.reg.close("ssh");
    s.reg.unstage_secrets("ssh");
}

#[tokio::test]
async fn wrong_passphrase_is_auth_failed() {
    let Some(mut s) = setup(SshAuth::Key) else {
        return;
    };
    s.profile.ssh.as_mut().unwrap().key_path = Some(
        common::ssh_keys_dir()
            .unwrap()
            .join("tester_ed25519_pass")
            .to_string_lossy()
            .into_owned(),
    );
    s.reg
        .stage_secret("ssh", SSH_PASSPHRASE, "not-the-passphrase")
        .unwrap();
    trust_host_key(&s).await;
    let e = s.reg.open(&s.profile, TIMEOUT).await.unwrap_err();
    assert_eq!(
        (e.code, e.params.get("kind").map(String::as_str)),
        (ErrorCode::SshError, Some("authFailed")),
        "{e:?}"
    );
    s.reg.unstage_secrets("ssh");
}

#[tokio::test]
async fn password_login_uses_askpass_and_a_wrong_password_is_auth_failed() {
    let Some(s) = setup(SshAuth::Password) else {
        return;
    };
    s.reg.stage_secret("ssh", SSH_PASSWORD, "wrong").unwrap();
    trust_host_key(&s).await;
    let e = s.reg.open(&s.profile, TIMEOUT).await.unwrap_err();
    assert_eq!(
        (e.code, e.params.get("kind").map(String::as_str)),
        (ErrorCode::SshError, Some("authFailed")),
        "{e:?}"
    );
    // Неудачный туннель в реестре не остаётся.
    assert_eq!(s.reg.status("ssh"), None);
    s.reg
        .stage_secret("ssh", SSH_PASSWORD, "tester-secret")
        .unwrap();
    let socks = s.reg.open(&s.profile, TIMEOUT).await.unwrap();
    assert_host_key_recorded(&s);
    assert!(
        query_version(socks, &s.profile)
            .await
            .contains("PostgreSQL")
    );
    s.reg.close("ssh");
    s.reg.unstage_secrets("ssh");
}

#[tokio::test]
async fn unreachable_upstream_behind_the_bastion_is_forward_failed() {
    let Some(s) = setup(SshAuth::Key) else { return };
    let socks = open_trusting(&s).await;
    // Имя не разрешается на бастионе и закрытый порт узла сети compose: канал не открылся, `ssh -D` закрывает
    // соединение с клиентом — сбой проброса, а не ошибка PostgreSQL.
    for (host, port) in [("nonexistent.invalid", 5432), ("pg-single", 5999)] {
        let e = spec_via(&s.profile, host, port, socks)
            .connect()
            .await
            .err()
            .unwrap_or_else(|| panic!("{host}:{port}: соединение установлено"));
        assert_forward_failed(&e, host, port);
    }
    // Patroni через тот же туннель: недоступный адрес API — тот же сбой проброса, а не общая ошибка Patroni.
    let target = s.profile.ssh.as_ref().unwrap().target.clone();
    let settings = PatroniSettings {
        urls: vec!["http://nonexistent.invalid:8008".into()],
        username: None,
        tls: None,
        host_overrides: vec![],
    };
    let http = ReqwestHttp::new(&settings, None, None, Some(socks))
        .unwrap()
        .with_ssh_target(&target);
    let e = http
        .get_json("http://nonexistent.invalid:8008/cluster")
        .await
        .unwrap_err();
    assert_forward_failed(&e, "nonexistent.invalid", 8008);
    // Туннель после недоступных узлов жив: соединение с настоящим узлом проходит.
    assert!(
        query_version(socks, &s.profile)
            .await
            .contains("PostgreSQL 15")
    );
    s.reg.close("ssh");
}

#[tokio::test]
async fn patroni_and_host_list_are_observed_through_the_tunnel() {
    let Some(s) = setup(SshAuth::Key) else { return };
    let socks = open_trusting(&s).await;
    // Patroni: адреса API по именам контейнеров, разрешает бастион.
    let settings = PatroniSettings {
        urls: vec![
            "http://patroni1:8008".into(),
            "http://patroni2:8008".into(),
            "http://patroni3:8008".into(),
        ],
        username: Some("patroni".into()),
        tls: None,
        host_overrides: vec![],
    };
    let http = ReqwestHttp::new(
        &settings,
        Some(common::PATRONI_PASSWORD_VALUE.into()),
        None,
        Some(socks),
    )
    .unwrap()
    .with_ssh_target("localhost");
    let t = discover(&http, &settings, "ssh", None, 0).await.unwrap();
    assert_eq!(t.members.len(), 3);
    assert!(t.leader().is_some());
    // Список узлов: primary и реплика по именам.
    let mut hl = s.profile.clone();
    hl.kind = ConnectionKind::HostList;
    hl.hosts = vec![
        HostPort {
            host: "pg-primary".into(),
            port: 5432,
        },
        HostPort {
            host: "pg-replica".into(),
            port: 5432,
        },
    ];
    hl.pg.ssl_mode = SslMode::Prefer;
    let secrets = common::secrets_with_password("ssh");
    let nodes = AdhocConnector {
        profile: hl.clone(),
        secrets,
        socks: Some(socks),
    };
    let t = observe_hosts("ssh", ConnectionKind::HostList, &hl.hosts, &nodes, 0).await;
    assert!(
        t.members.len() >= 2,
        "через туннель наблюдались не все узлы: {:?}",
        t.members
    );
    assert!(t.members.iter().any(|m| m.role == Role::Primary));
    assert!(t.members.iter().any(|m| m.role == Role::Replica));
    s.reg.close("ssh");
}
