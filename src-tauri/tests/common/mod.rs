#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glafira_pg_lib::conn::pg::ConnectSpec;
use glafira_pg_lib::conn::pool::PoolRegistry;
use glafira_pg_lib::conn::profile::{
    ConnectionKind, ConnectionProfile, Environment, HostOverride, HostPort, PatroniSettings,
    PgSettings, PgTls, SshAuth, SshSettings, SslMode, TargetServerType,
};
use glafira_pg_lib::net::ssh::tunnel::{TunnelConfig, TunnelRegistry};
use glafira_pg_lib::secrets::{MemoryStore, PG_PASSWORD, SecretStore, secret_key};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

pub const PG_USER: &str = "glafira_diag";
pub const PG_PASSWORD_VALUE: &str = "glafira-secret";

/// Значение переменной окружения; без неё интеграционный тест пропускается.
pub fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Адреса узлов матрицы версий PostgreSQL 13–18 (`GPG_IT_MATRIX=localhost:5513,…,localhost:5518`, профиль
/// `matrix` стенда); без переменной список пуст и тесты матрицы ничего не проверяют.
pub fn matrix_addrs() -> Vec<String> {
    env("GPG_IT_MATRIX")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub fn host_port(s: &str) -> (String, u16) {
    let (h, p) = s.trim().rsplit_once(':').expect("host:port");
    (h.to_string(), p.parse().expect("port"))
}

pub const SUPERUSER: &str = "postgres";
pub const SUPERUSER_PASSWORD: &str = "postgres-secret";
pub const APP_USER: &str = "glafira_app";
pub const APP_PASSWORD: &str = "app-secret";

/// Соединение с узлом стенда под заданной ролью (без TLS), с параметрами сессии продукта (`SESSION_OPTIONS`):
/// так сборщики диагностики проверяются в тех же условиях, что в приложении. Соединение живёт, пока жив клиент.
pub async fn connect_as(
    addr: &str,
    db: &str,
    user: &str,
    password: &str,
) -> tokio_postgres::Client {
    let mut p = server_profile("it-as", addr);
    p.pg.ssl_mode = SslMode::Disable;
    p.pg.username = user.into();
    let s = MemoryStore::default();
    s.set(&secret_key("it-as", PG_PASSWORD), password).unwrap();
    let (h, port) = host_port(addr);
    ConnectSpec::build(&p, &h, port, db, &s)
        .unwrap()
        .connect()
        .await
        .unwrap_or_else(|e| panic!("{addr} {user}: {e:?}"))
}

/// Сессия для сценариев стенда без ограничений диагностики. `connect_as` открывает соединение с `SESSION_OPTIONS`
/// продукта (только чтение, `statement_timeout`, `lock_timeout`, `search_path = pg_catalog`, `application_name =
/// glafira-pg`): под ними нельзя писать и ждать блокировку дольше секунды, а сессия «приложения» не должна
/// считаться собственной сессией Glafira. Стартовые параметры не сбрасываются через `RESET`, поэтому значения
/// заданы явно.
async fn unrestricted(c: tokio_postgres::Client) -> tokio_postgres::Client {
    c.batch_execute(
        "SET default_transaction_read_only = off; SET statement_timeout = 0; SET lock_timeout = 0; \
         SET search_path = \"$user\", public; SET application_name = 'glafira-it'",
    )
    .await
    .unwrap();
    c
}

/// Суперпользователь стенда (`postgres`) — сессия без ограничений, см. `unrestricted`.
pub async fn superuser(addr: &str, db: &str) -> tokio_postgres::Client {
    unrestricted(connect_as(addr, db, SUPERUSER, SUPERUSER_PASSWORD).await).await
}

/// Сессия приложения (`glafira_app`) в демо-базе `orders` — без ограничений, см. `unrestricted`.
pub async fn app(addr: &str) -> tokio_postgres::Client {
    unrestricted(connect_as(addr, "orders", APP_USER, APP_PASSWORD).await).await
}

pub fn lab_addr() -> Option<String> {
    env("GPG_IT_LAB")
}

/// Узел Patroni, принимающий запись: лидер может быть любым из трёх.
pub async fn patroni_leader_addr() -> Option<String> {
    env("GPG_IT_PATRONI")?;
    for addr in ["localhost:5441", "localhost:5442", "localhost:5443"] {
        let c = superuser(addr, "postgres").await;
        let in_recovery: bool = c
            .query_one("SELECT pg_is_in_recovery()", &[])
            .await
            .unwrap()
            .get(0);
        if !in_recovery {
            return Some(addr.to_string());
        }
    }
    None
}

/// Пишущие узлы стенда, для которых заданы переменные: одиночный, primary пары, лидер Patroni, лаборатория, матрица.
pub async fn writable_addrs() -> Vec<String> {
    let mut out: Vec<String> = ["GPG_IT_SINGLE", "GPG_IT_PRIMARY", "GPG_IT_LAB"]
        .into_iter()
        .filter_map(env)
        .collect();
    out.extend(patroni_leader_addr().await);
    out.extend(matrix_addrs());
    out
}

/// Роль `glafira_app` и её права на схему `shop` — на уже поднятом стенде, созданном до P2 (`init.sql` работает только
/// при первой инициализации). Идемпотентно; только на пишущем узле.
pub async fn ensure_fixtures(addr: &str) {
    let c = superuser(addr, "postgres").await;
    c.batch_execute(
        "DO $$ BEGIN \
           IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'glafira_app') THEN \
             CREATE ROLE glafira_app LOGIN PASSWORD 'app-secret'; \
           END IF; \
         END $$",
    )
    .await
    .unwrap();
    let o = superuser(addr, "orders").await;
    o.batch_execute(
        "GRANT USAGE ON SCHEMA shop TO glafira_app; \
         GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA shop TO glafira_app; \
         GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA shop TO glafira_app;",
    )
    .await
    .unwrap();
}

pub fn server_profile(id: &str, addr: &str) -> ConnectionProfile {
    let (host, port) = host_port(addr);
    ConnectionProfile {
        id: id.into(),
        name: id.into(),
        color: "#4f8cff".into(),
        environment: Environment::Dev,
        read_only: false,
        kind: ConnectionKind::Server,
        hosts: vec![HostPort { host, port }],
        target_server_type: TargetServerType::Primary,
        patroni: None,
        pg: PgSettings {
            username: PG_USER.into(),
            database: "postgres".into(),
            ssl_mode: SslMode::Prefer,
            tls: PgTls::default(),
            connect_timeout_s: 5,
            databases: None,
        },
        ssh: None,
        folder_id: None,
        order: 0,
        extra: Default::default(),
    }
}

/// Реестр туннелей для тестов без ssh: каталоги во временной папке процесса (без профилей с ssh реестр файлов
/// не пишет).
pub fn no_tunnels(pools: Arc<PoolRegistry>) -> Arc<TunnelRegistry> {
    let dir = std::env::temp_dir().join(format!("glafira-it-tunnels-{}", std::process::id()));
    TunnelRegistry::new(
        TunnelConfig::new(dir.join("data"), dir.join("logs")),
        pools,
        Arc::new(MemoryStore::default()),
    )
}

/// Адрес bastion (`GPG_IT_BASTION=localhost:2222`) и каталог ключей (`GPG_IT_SSH_KEYS`, абсолютный путь); без
/// любой из переменных тест пропускается.
pub fn bastion_settings(auth: SshAuth) -> Option<SshSettings> {
    let (host, port) = host_port(&env("GPG_IT_BASTION")?);
    let keys = ssh_keys_dir()?;
    Some(SshSettings {
        target: host,
        port: Some(port),
        user: Some("tester".into()),
        auth,
        key_path: match auth {
            SshAuth::Key => Some(keys.join("tester_ed25519").to_string_lossy().into_owned()),
            _ => None,
        },
    })
}

pub fn ssh_keys_dir() -> Option<PathBuf> {
    env("GPG_IT_SSH_KEYS").map(PathBuf::from)
}

/// Реестр туннелей для тестов с настоящим OpenSSH: свой known_hosts во временном каталоге (файл разработчика не
/// трогается), секреты для askpass — в файле 0600 (keychain на CI недоступен).
pub fn tunnel_registry_for_tests(dir: &Path, pools: Arc<PoolRegistry>) -> Arc<TunnelRegistry> {
    let mut cfg = TunnelConfig::new(dir.join("data"), dir.join("logs"));
    cfg.known_hosts = Some(dir.join("known_hosts"));
    cfg.stage_secrets_in_file = true;
    // Askpass умеет только бинарь приложения; cargo собирает его для интеграционных тестов и даёт путь.
    cfg.askpass_bin = Some(PathBuf::from(env!("CARGO_BIN_EXE_glafira-pg")));
    TunnelRegistry::new(cfg, pools, Arc::new(MemoryStore::default()))
}

pub fn secrets_with_password(id: &str) -> Arc<MemoryStore> {
    let s = MemoryStore::default();
    s.set(&secret_key(id, PG_PASSWORD), PG_PASSWORD_VALUE)
        .unwrap();
    Arc::new(s)
}

/// TCP-прокси перед стендом, который умеет «заморозить» уже открытые соединения: байты перестают
/// пересылаться в обе стороны, а сокеты остаются открытыми, без RST и FIN — как молчащий путь
/// (оборванный NAT, пропавший туннель). Соединения, открытые после заморозки, работают как обычно.
/// Замороженное соединение читает и выбрасывает байты, поэтому закрытие сокета клиентом заметно:
/// его считает `closed_connections`.
pub struct PausableProxy {
    port: u16,
    flags: Arc<Mutex<Vec<Arc<AtomicBool>>>>,
    closed: Arc<AtomicUsize>,
    accepted: Arc<AtomicUsize>,
    accept: JoinHandle<()>,
}

impl PausableProxy {
    /// Слушает случайный порт на 127.0.0.1 и пересылает соединения на `upstream` (`host:port`).
    pub async fn start(upstream: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
        let port = listener.local_addr().expect("proxy addr").port();
        let flags: Arc<Mutex<Vec<Arc<AtomicBool>>>> = Arc::default();
        let closed: Arc<AtomicUsize> = Arc::default();
        let accepted: Arc<AtomicUsize> = Arc::default();
        let upstream = upstream.to_string();
        let accept = tokio::spawn({
            let (flags, closed, accepted) = (flags.clone(), closed.clone(), accepted.clone());
            async move {
                while let Ok((client, _)) = listener.accept().await {
                    accepted.fetch_add(1, Ordering::SeqCst);
                    let Ok(server) = TcpStream::connect(upstream.as_str()).await else {
                        continue;
                    };
                    let frozen = Arc::new(AtomicBool::new(false));
                    flags.lock().unwrap().push(frozen.clone());
                    let (client_r, client_w) = client.into_split();
                    let (server_r, server_w) = server.into_split();
                    tokio::spawn(pipe(
                        client_r,
                        server_w,
                        frozen.clone(),
                        Some(closed.clone()),
                    ));
                    tokio::spawn(pipe(server_r, client_w, frozen, None));
                }
            }
        });
        Self {
            port,
            flags,
            closed,
            accepted,
            accept,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Сколько TCP-соединений клиент открыл через прокси (попыток подключения).
    pub fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// Замораживает все соединения, открытые к этому моменту; новые не затрагиваются.
    pub fn freeze_existing(&self) {
        for f in self.flags.lock().unwrap().iter() {
            f.store(true, Ordering::SeqCst);
        }
    }

    /// Сколько соединений клиент уже закрыл (замороженные тоже).
    pub fn closed_connections(&self) -> usize {
        self.closed.load(Ordering::SeqCst)
    }

    /// Ждёт, пока клиент закроет не меньше `n` соединений, но не дольше `limit`.
    pub async fn wait_closed(&self, n: usize, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while self.closed_connections() < n {
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        true
    }
}

impl Drop for PausableProxy {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

/// Пересылка одного направления. Замороженное соединение читает и выбрасывает байты («чёрная
/// дыра»): ничего не доходит ни до сервера, ни до клиента, но конец потока виден. `closed`
/// (направление клиент → сервер) считает соединения, которые клиент закрыл.
/// Задачи замороженных соединений доживают до закрытия сокетов клиентом или до конца runtime теста.
async fn pipe(
    mut from: OwnedReadHalf,
    mut to: OwnedWriteHalf,
    frozen: Arc<AtomicBool>,
    closed: Option<Arc<AtomicUsize>>,
) {
    let mut buf = [0u8; 8192];
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        // Заморозка могла наступить, пока ждали данные: они «теряются» вместе с путём.
        if frozen.load(Ordering::SeqCst) {
            continue;
        }
        if to.write_all(&buf[..n]).await.is_err() {
            break;
        }
    }
    if let Some(closed) = closed {
        closed.fetch_add(1, Ordering::SeqCst);
    }
    let _ = to.shutdown().await;
}

/// «Сервер PostgreSQL», который отвергает любой вход: на `SSLRequest` отвечает `N`, на стартовое сообщение —
/// `ErrorResponse` 28P01 (неверный пароль). По `accepted` видно, сколько попыток входа сделал клиент.
pub struct RejectingPg {
    port: u16,
    accepted: Arc<AtomicUsize>,
    accept: JoinHandle<()>,
}

impl RejectingPg {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let accepted: Arc<AtomicUsize> = Arc::default();
        let accept = tokio::spawn({
            let accepted = accepted.clone();
            async move {
                while let Ok((tcp, _)) = listener.accept().await {
                    accepted.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(reject_login(tcp));
                }
            }
        });
        Self {
            port,
            accepted,
            accept,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }
}

impl Drop for RejectingPg {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

async fn reject_login(mut tcp: TcpStream) {
    for _ in 0..2 {
        let mut len = [0u8; 4];
        if tcp.read_exact(&mut len).await.is_err() {
            return;
        }
        let mut body = vec![0u8; (u32::from_be_bytes(len) as usize).saturating_sub(4)];
        if tcp.read_exact(&mut body).await.is_err() {
            return;
        }
        // SSLRequest (код 80877103) — без TLS; иначе это стартовое сообщение.
        if body == 80_877_103u32.to_be_bytes() {
            if tcp.write_all(b"N").await.is_err() {
                return;
            }
            continue;
        }
        break;
    }
    let mut fields = Vec::new();
    for (tag, value) in [
        (b'S', "FATAL"),
        (b'V', "FATAL"),
        (b'C', "28P01"),
        (b'M', "password authentication failed"),
    ] {
        fields.push(tag);
        fields.extend_from_slice(value.as_bytes());
        fields.push(0);
    }
    fields.push(0);
    let mut msg = vec![b'E'];
    msg.extend_from_slice(&((fields.len() + 4) as u32).to_be_bytes());
    msg.extend_from_slice(&fields);
    let _ = tcp.write_all(&msg).await;
    let _ = tcp.shutdown().await;
}

/// Keychain в памяти, который считает чтения пароля PostgreSQL.
#[derive(Default)]
pub struct CountingSecrets {
    pub inner: MemoryStore,
    pg_password_reads: AtomicUsize,
}

impl CountingSecrets {
    pub fn pg_password_reads(&self) -> usize {
        self.pg_password_reads.load(Ordering::SeqCst)
    }
}

impl SecretStore for CountingSecrets {
    fn get(&self, key: &str) -> glafira_pg_lib::error::AppResult<Option<String>> {
        if key.ends_with(PG_PASSWORD) {
            self.pg_password_reads.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.get(key)
    }
    fn set(&self, key: &str, value: &str) -> glafira_pg_lib::error::AppResult<()> {
        self.inner.set(key, value)
    }
    fn delete(&self, key: &str) -> glafira_pg_lib::error::AppResult<()> {
        self.inner.delete(key)
    }
}

/// Настройки кластера стенда: участники отдают внутренние имена patroniN — подменяем их на порты хоста.
pub fn patroni_settings() -> Option<PatroniSettings> {
    let urls = env("GPG_IT_PATRONI")?;
    Some(PatroniSettings {
        urls: urls.split(',').map(|u| u.trim().to_string()).collect(),
        username: Some("patroni".into()),
        tls: None,
        host_overrides: (1..=3)
            .map(|i| HostOverride {
                member: format!("patroni{i}"),
                host: Some("localhost".into()),
                port: Some(5440 + i as u16),
                api_url: Some(format!("http://localhost:801{i}")),
            })
            .collect(),
    })
}

pub const PATRONI_PASSWORD_VALUE: &str = "patroni-secret";

/// `docker …`; `Err` с текстом stderr, если команда не удалась.
pub fn try_docker(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("docker")
        .args(args)
        .output()
        .map_err(|e| format!("docker {args:?}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "docker {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// `docker …`; паника, если команда не удалась.
pub fn docker(args: &[&str]) -> String {
    try_docker(args).unwrap_or_else(|e| panic!("{e}"))
}

/// Ждёт готовности стенда (`scripts/wait-stand.sh`): после остановок и переключений следующий тест
/// должен видеть лидера и две потоковые реплики.
pub fn wait_stand() {
    let wait = concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/wait-stand.sh");
    let out = std::process::Command::new(wait)
        .output()
        .expect("wait-stand.sh");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn patronictl_args<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut full = vec![
        "exec",
        "gp-patroni1",
        "/opt/patroni/bin/patronictl",
        "-c",
        "/etc/patroni/patroni.yml",
    ];
    full.extend_from_slice(args);
    full
}

pub fn patronictl(args: &[&str]) -> String {
    docker(&patronictl_args(args))
}

/// `patronictl …` без паники: для восстановления стенда, где «уже восстановлено» — не ошибка.
pub fn try_patronictl(args: &[&str]) -> Result<String, String> {
    try_docker(&patronictl_args(args))
}

/// Возвращает стенд в рабочее состояние при выходе из области видимости — и когда проверка теста
/// упала с паникой. На обычном выходе сбой восстановления роняет тест; при уже идущей панике он
/// только выводится (вторая паника в `Drop` прервала бы процесс и скрыла исходную ошибку).
#[must_use = "без привязки к переменной восстановление выполнится сразу"]
pub struct RestoreGuard<F: FnOnce()> {
    restore: Option<F>,
}

pub fn restore_on_drop<F: FnOnce()>(restore: F) -> RestoreGuard<F> {
    RestoreGuard {
        restore: Some(restore),
    }
}

impl<F: FnOnce()> Drop for RestoreGuard<F> {
    fn drop(&mut self) {
        let Some(restore) = self.restore.take() else {
            return;
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(restore));
        if let Err(cause) = result {
            if std::thread::panicking() {
                eprintln!("не удалось восстановить стенд после сбоя теста");
            } else {
                std::panic::resume_unwind(cause);
            }
        }
    }
}

/// Список узлов стенда без Patroni.
pub fn host_list_profile(id: &str) -> Option<ConnectionProfile> {
    let (primary, replica) = (env("GPG_IT_PRIMARY")?, env("GPG_IT_REPLICA")?);
    let mut p = server_profile(id, &primary);
    p.kind = ConnectionKind::HostList;
    let (h, port) = host_port(&replica);
    p.hosts.push(HostPort { host: h, port });
    Some(p)
}

/// Возвращает пару primary/реплика стенда в исходное состояние (после promote и остановок).
pub fn reset_replication_pair() {
    let compose = concat!(env!("CARGO_MANIFEST_DIR"), "/../dev/docker-compose.yml");
    docker(&[
        "compose",
        "-f",
        compose,
        "rm",
        "-sfv",
        "pg-primary",
        "pg-replica",
    ]);
    docker(&[
        "compose",
        "-f",
        compose,
        "up",
        "-d",
        "pg-primary",
        "pg-replica",
    ]);
    wait_stand();
}
