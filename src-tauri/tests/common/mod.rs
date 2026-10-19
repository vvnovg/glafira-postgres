#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use glafira_pg_lib::conn::profile::{
    ConnectionKind, ConnectionProfile, Environment, HostOverride, HostPort, PatroniSettings,
    PgSettings, PgTls, SslMode, TargetServerType,
};
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

pub fn host_port(s: &str) -> (String, u16) {
    let (h, p) = s.trim().rsplit_once(':').expect("host:port");
    (h.to_string(), p.parse().expect("port"))
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
        folder_id: None,
        order: 0,
        extra: Default::default(),
    }
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
pub struct PausableProxy {
    port: u16,
    flags: Arc<Mutex<Vec<Arc<AtomicBool>>>>,
    accept: JoinHandle<()>,
}

impl PausableProxy {
    /// Слушает случайный порт на 127.0.0.1 и пересылает соединения на `upstream` (`host:port`).
    pub async fn start(upstream: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
        let port = listener.local_addr().expect("proxy addr").port();
        let flags: Arc<Mutex<Vec<Arc<AtomicBool>>>> = Arc::default();
        let upstream = upstream.to_string();
        let accept = tokio::spawn({
            let flags = flags.clone();
            async move {
                while let Ok((client, _)) = listener.accept().await {
                    let Ok(server) = TcpStream::connect(upstream.as_str()).await else {
                        continue;
                    };
                    let frozen = Arc::new(AtomicBool::new(false));
                    flags.lock().unwrap().push(frozen.clone());
                    let (client_r, client_w) = client.into_split();
                    let (server_r, server_w) = server.into_split();
                    tokio::spawn(pipe(client_r, server_w, frozen.clone()));
                    tokio::spawn(pipe(server_r, client_w, frozen));
                }
            }
        });
        Self {
            port,
            flags,
            accept,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Замораживает все соединения, открытые к этому моменту; новые не затрагиваются.
    pub fn freeze_existing(&self) {
        for f in self.flags.lock().unwrap().iter() {
            f.store(true, Ordering::SeqCst);
        }
    }
}

impl Drop for PausableProxy {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

/// Пересылка одного направления; замороженное соединение ничего не читает и не отдаёт дальше.
/// Задачи замороженных соединений доживают до конца runtime теста.
async fn pipe(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, frozen: Arc<AtomicBool>) {
    let mut buf = [0u8; 8192];
    loop {
        while frozen.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        // Заморозка могла наступить, пока ждали данные: они «теряются» вместе с путём.
        while frozen.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if to.write_all(&buf[..n]).await.is_err() {
            break;
        }
    }
    let _ = to.shutdown().await;
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

/// `docker …`; паника, если команда не удалась.
pub fn docker(args: &[&str]) -> String {
    let out = std::process::Command::new("docker")
        .args(args)
        .output()
        .expect("docker");
    assert!(
        out.status.success(),
        "docker {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
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

pub fn patronictl(args: &[&str]) -> String {
    let mut full = vec![
        "exec",
        "gp-patroni1",
        "/opt/patroni/bin/patronictl",
        "-c",
        "/etc/patroni/patroni.yml",
    ];
    full.extend_from_slice(args);
    docker(&full)
}
