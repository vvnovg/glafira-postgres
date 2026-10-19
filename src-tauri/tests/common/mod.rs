#![allow(dead_code)]

use std::sync::Arc;

use glafira_pg_lib::conn::profile::{
    ConnectionKind, ConnectionProfile, Environment, HostPort, PgSettings, PgTls, SslMode,
    TargetServerType,
};
use glafira_pg_lib::secrets::{MemoryStore, PG_PASSWORD, SecretStore, secret_key};

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
