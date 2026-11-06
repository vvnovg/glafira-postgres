//! Шлюз MCP Glafira без окна — для прогона моделей (`dev/llm-eval`). Тот же код, что в приложении (`AppState`,
//! `McpController`, инструменты `glafira-pg-mcp`), но: каталог данных — `$EVAL_HOME/Library/Application Support/<id>`
//! (шим `glafira-pg mcp`, запущенный с `HOME=$EVAL_HOME`, находит там `mcp.json`); пароли стенда — в памяти, без
//! связки ключей; три подключения стенда с уровнем «Диагностика»; новый клиент подтверждается автоматически.
//!
//! `cargo run --example llm_eval_host` (из `src-tauri`), остановка — Ctrl+C или SIGTERM. Переменные: `EVAL_HOME`
//! (по умолчанию `~/.glafira-llm-eval`: путь сокета должен уложиться в 100 байт), `EVAL_LAB` (`localhost:5470`),
//! `EVAL_PERF` (`localhost:5471`), `EVAL_PATRONI` (`http://localhost:8011,http://localhost:8012,http://localhost:8013`).
//! Только для разработки: пароли — известные пароли стенда (`README.md`, «Стенд»).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use glafira_pg_lib::conn::profile::{
    ConnectionKind, ConnectionProfile, Environment, HostOverride, HostPort, LlmAccess,
    PatroniSettings, PgSettings, PgTls, SslMode, TargetServerType,
};
use glafira_pg_lib::secrets::{
    MemoryStore, PATRONI_PASSWORD, PG_PASSWORD, SecretStore, secret_key,
};
use glafira_pg_lib::state::AppState;

const PG_USER: &str = "glafira_diag";
const PG_PASSWORD_VALUE: &str = "glafira-secret";
const PATRONI_USER: &str = "patroni";
const PATRONI_PASSWORD_VALUE: &str = "patroni-secret";

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn host_port(addr: &str) -> HostPort {
    let (host, port) = addr.rsplit_once(':').expect("адрес вида host:port");
    HostPort {
        host: host.to_string(),
        port: port.parse().expect("порт"),
    }
}

fn profile(id: &str, database: &str) -> ConnectionProfile {
    ConnectionProfile {
        id: id.into(),
        name: id.into(),
        color: "#4f8cff".into(),
        environment: Environment::Dev,
        read_only: false,
        kind: ConnectionKind::Server,
        hosts: Vec::new(),
        target_server_type: TargetServerType::Primary,
        patroni: None,
        pg: PgSettings {
            username: PG_USER.into(),
            database: database.into(),
            ssl_mode: SslMode::Disable,
            tls: PgTls::default(),
            connect_timeout_s: 5,
            databases: None,
        },
        ssh: None,
        folder_id: None,
        order: 0,
        llm_access: LlmAccess::Diagnostics,
        extra: Default::default(),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,glafira_pg_lib=info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let home = PathBuf::from(env_or(
        "EVAL_HOME",
        &dirs::home_dir()
            .expect("HOME")
            .join(".glafira-llm-eval")
            .to_string_lossy(),
    ));
    let data_dir = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        home.join(".local/share")
    }
    .join(glafira_pg_lib::product::IDENTIFIER);
    std::fs::create_dir_all(&data_dir).expect("каталог данных");

    let secrets = MemoryStore::default();
    for id in ["lab", "demo", "perf"] {
        secrets
            .set(&secret_key(id, PG_PASSWORD), PG_PASSWORD_VALUE)
            .expect("пароль");
    }
    secrets
        .set(
            &secret_key("demo", PATRONI_PASSWORD),
            PATRONI_PASSWORD_VALUE,
        )
        .expect("пароль Patroni");
    let st = AppState::new(&data_dir, Arc::new(secrets)).expect("AppState");

    let mut lab = profile("lab", "orders");
    lab.hosts = vec![host_port(&env_or("EVAL_LAB", "localhost:5470"))];
    let mut perf = profile("perf", "perf");
    perf.hosts = vec![host_port(&env_or("EVAL_PERF", "localhost:5471"))];
    let mut demo = profile("demo", "orders");
    demo.kind = ConnectionKind::Patroni;
    demo.patroni = Some(PatroniSettings {
        urls: env_or(
            "EVAL_PATRONI",
            "http://localhost:8011,http://localhost:8012,http://localhost:8013",
        )
        .split(',')
        .map(|u| u.trim().to_string())
        .collect(),
        username: Some(PATRONI_USER.into()),
        tls: None,
        host_overrides: (1..=3)
            .map(|i| HostOverride {
                member: format!("patroni{i}"),
                host: Some("localhost".into()),
                port: Some(5440 + i as u16),
                api_url: Some(format!("http://localhost:801{i}")),
            })
            .collect(),
    });
    for p in [lab, demo, perf] {
        let name = p.name.clone();
        st.connections.upsert(p.clone()).expect("подключение");
        if let Err(e) = st.runtimes.connect(p).await {
            eprintln!(
                "llm_eval_host: {name} не открылось сразу ({e}); откроется при первом вызове"
            );
        }
    }

    let status = st
        .mcp
        .set_enabled(st.clone(), true)
        .await
        .expect("MCP-сервер");
    eprintln!(
        "llm_eval_host: сервер {:?}, данные {}, подключений для LLM {}",
        status.server,
        data_dir.display(),
        status.open_connections
    );
    println!("READY {}", home.display());

    // Подтверждение новых клиентов — вместо диалога Glafira. Процесс работает, пока его не остановят (SIGINT/SIGTERM);
    // оставшийся `mcp.json` следующий запуск заменит сам (сервер из файла не отвечает — файл удаляется до bind).
    loop {
        for p in st.mcp.status().pending {
            let _ = st.mcp.answer_approval(&p.session_id, true);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
