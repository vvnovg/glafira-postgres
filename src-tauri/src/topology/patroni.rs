//! Клиент Patroni REST API (спец. P1 §6.2): участники — `GET /cluster`, детали — `GET /patroni`.

use std::time::Duration;

use serde::Deserialize;

use crate::conn::errors::chain;
use crate::conn::profile::{ConnectionKind, HostOverride, PatroniSettings};
use crate::conn::tls;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::topology::BoxFuture;
use crate::topology::model::{Member, Role, Topology, TopologyWarning, system_id_warning};

pub const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
/// Сколько текста ответа с ошибкой сохраняется в `details`.
const BODY_SNIPPET: usize = 500;

/// GET JSON из Patroni; трейт — чтобы подменять сеть в тестах.
pub trait PatroniHttp: Send + Sync {
    fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, AppResult<serde_json::Value>>;
}

pub struct ReqwestHttp {
    client: reqwest::Client,
    auth: Option<(String, String)>,
}

fn http_setup(e: impl std::fmt::Display) -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.patroni.setup",
        "Could not prepare the Patroni API client"
    )
    .with_details(e.to_string())
}

impl ReqwestHttp {
    /// Блокирующий вызов (файлы сертификатов). `password` — секрет `patroniPassword`.
    pub fn new(
        settings: &PatroniSettings,
        password: Option<String>,
        key_password: Option<&str>,
    ) -> AppResult<Self> {
        let mut b = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .connect_timeout(HTTP_TIMEOUT);
        if let Some(t) = &settings.tls {
            // CA-файл заменяет системные корни (как для PostgreSQL). Без проверки имени reqwest/rustls
            // принимает только явный набор корней, поэтому системные в этом случае подставляем сами.
            let roots = if let Some(ca) = &t.ca_path {
                let pem = tls::ca_pem_bundle(ca)?;
                reqwest::Certificate::from_pem_bundle(&pem).map_err(http_setup)?
            } else if !t.verify_hostname {
                tls::system_roots_der()
                    .iter()
                    .filter_map(|der| reqwest::Certificate::from_der(der).ok())
                    .collect()
            } else {
                Vec::new()
            };
            if !roots.is_empty() {
                b = b.tls_certs_only(roots);
            }
            if !t.verify_hostname {
                b = b.tls_danger_accept_invalid_hostnames(true);
            }
            if let Some(cc) = &t.client_cert {
                let pem = tls::identity_pem(cc, key_password)?;
                b = b.identity(reqwest::Identity::from_pem(&pem).map_err(http_setup)?);
            }
        }
        Ok(Self {
            client: b.build().map_err(http_setup)?,
            auth: settings
                .username
                .clone()
                .map(|u| (u, password.unwrap_or_default())),
        })
    }
}

fn request_error(url: &str, e: &reqwest::Error) -> AppError {
    let text = chain(e);
    if e.is_timeout() {
        app_err!(
            ErrorCode::PatroniError,
            "errors.patroni.timeout",
            "Patroni API at {{url}} did not answer within 5 s",
            url = url
        )
    } else if text.to_lowercase().contains("certificate") {
        app_err!(
            ErrorCode::TlsError,
            "errors.patroni.tls",
            "TLS error talking to the Patroni API at {{url}}",
            url = url
        )
    } else {
        app_err!(
            ErrorCode::PatroniError,
            "errors.patroni.unreachable",
            "Patroni API at {{url}} is unreachable",
            url = url
        )
    }
    .with_details(text)
}

fn bad_json(url: &str, e: impl std::fmt::Display) -> AppError {
    app_err!(
        ErrorCode::PatroniError,
        "errors.patroni.badJson",
        "Patroni API at {{url}} returned an unexpected response",
        url = url
    )
    .with_details(e.to_string())
}

impl PatroniHttp for ReqwestHttp {
    fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, AppResult<serde_json::Value>> {
        Box::pin(async move {
            let mut req = self.client.get(url);
            if let Some((u, p)) = &self.auth {
                req = req.basic_auth(u, Some(p));
            }
            let resp = req.send().await.map_err(|e| request_error(url, &e))?;
            let status = resp.status();
            let body = resp.text().await.map_err(|e| request_error(url, &e))?;
            let parsed = serde_json::from_str::<serde_json::Value>(&body);
            // /patroni у узла с остановленным PostgreSQL отвечает 503, но с тем же JSON.
            match (status.as_u16(), parsed) {
                (200..=299, Ok(v)) => Ok(v),
                (503, Ok(v)) if v.is_object() => Ok(v),
                (200..=299, Err(e)) => Err(bad_json(url, e)),
                (401 | 403, _) => Err(app_err!(
                    ErrorCode::AuthFailed,
                    "errors.patroni.unauthorized",
                    "The Patroni API at {{url}} rejected the credentials",
                    url = url
                )),
                (code, _) => Err(app_err!(
                    ErrorCode::PatroniError,
                    "errors.patroni.httpStatus",
                    "Patroni API at {{url}} answered {{status}}",
                    url = url,
                    status = code
                )
                .with_details(body.chars().take(BODY_SNIPPET).collect::<String>())),
            }
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum LagJson {
    Bytes(i64),
    Text(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemberJson {
    pub name: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub api_url: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub timeline: Option<i64>,
    #[serde(default)]
    pub lag: Option<LagJson>,
    #[serde(default)]
    pub replay_lag: Option<LagJson>,
    #[serde(default)]
    pub tags: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub pending_restart: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClusterJson {
    #[serde(default)]
    pub members: Vec<MemberJson>,
    #[serde(default)]
    pub pause: Option<bool>,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PatroniInfo {
    #[serde(default)]
    version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PatroniJson {
    #[serde(default)]
    server_version: Option<i64>,
    #[serde(default)]
    dcs_last_seen: Option<i64>,
    #[serde(default)]
    database_system_identifier: Option<String>,
    #[serde(default)]
    patroni: Option<PatroniInfo>,
    #[serde(default)]
    pending_restart: Option<bool>,
}

/// Базовый адрес API: `http://h:8008/patroni` → `http://h:8008`.
pub fn api_base(url: &str) -> String {
    url.trim()
        .trim_end_matches('/')
        .trim_end_matches("/patroni")
        .trim_end_matches('/')
        .to_string()
}

pub fn map_role(raw: Option<&str>) -> Role {
    match raw {
        Some("leader" | "master" | "primary") => Role::Primary,
        Some("standby_leader") => Role::StandbyLeader,
        Some("sync_standby") => Role::SyncStandby,
        Some("quorum_standby") => Role::QuorumStandby,
        Some("replica") => Role::Replica,
        _ => Role::Unknown,
    }
}

/// `server_version` из `/patroni` (170002) → `17.2`; до 10-й версии — `9.6.24`.
pub fn format_server_version(v: i64) -> String {
    if v >= 100_000 {
        format!("{}.{}", v / 10_000, v % 100)
    } else {
        format!("{}.{}.{}", v / 10_000, (v / 100) % 100, v % 100)
    }
}

fn lag_bytes(l: Option<&LagJson>) -> Option<i64> {
    match l {
        Some(LagJson::Bytes(b)) => Some(*b),
        _ => None,
    }
}

/// Подмена адресов участника из профиля.
fn override_for<'a>(settings: &'a PatroniSettings, member: &str) -> Option<&'a HostOverride> {
    settings.host_overrides.iter().find(|o| o.member == member)
}

/// Участники из `/cluster` с подменой адресов из профиля.
pub fn members_from_cluster(c: &ClusterJson, settings: &PatroniSettings) -> Vec<Member> {
    c.members
        .iter()
        .map(|m| {
            let ov = override_for(settings, &m.name);
            let host = ov
                .and_then(|o| o.host.clone())
                .or_else(|| m.host.clone())
                .unwrap_or_default();
            let port = ov.and_then(|o| o.port).or(m.port).unwrap_or(5432);
            let mut member = Member::new(&m.name, host, port);
            member.api_url = ov
                .and_then(|o| o.api_url.as_deref())
                .or(m.api_url.as_deref())
                .map(api_base);
            member.role = map_role(m.role.as_deref());
            member.raw_role = m.role.clone();
            member.state = m.state.clone();
            member.timeline = m.timeline;
            member.lag_bytes =
                lag_bytes(m.replay_lag.as_ref()).or_else(|| lag_bytes(m.lag.as_ref()));
            member.tags = m.tags.clone();
            member.pending_restart = m.pending_restart.unwrap_or(false);
            member
        })
        .collect()
}

/// Порядок опроса `/cluster`: лидер прошлого опроса, остальные участники, адреса профиля — без повторов.
pub fn cluster_bases(settings: &PatroniSettings, previous: Option<&Topology>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |u: &str| {
        let b = api_base(u);
        if !b.is_empty() && !out.contains(&b) {
            out.push(b);
        }
    };
    if let Some(t) = previous {
        if let Some(u) = t.leader().and_then(|m| m.api_url.as_deref()) {
            add(u);
        }
        for m in &t.members {
            if let Some(u) = m.api_url.as_deref() {
                add(u);
            }
        }
    }
    for u in &settings.urls {
        add(u);
    }
    out
}

async fn fetch_cluster(http: &dyn PatroniHttp, bases: &[String]) -> AppResult<ClusterJson> {
    let mut last = None;
    for base in bases {
        let url = format!("{base}/cluster");
        match http
            .get_json(&url)
            .await
            .and_then(|v| serde_json::from_value(v).map_err(|e| bad_json(&url, e)))
        {
            Ok(c) => return Ok(c),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        app_err!(
            ErrorCode::PatroniError,
            "errors.patroni.noUrls",
            "No Patroni API addresses"
        )
    }))
}

pub async fn discover(
    http: &dyn PatroniHttp,
    settings: &PatroniSettings,
    connection_id: &str,
    previous: Option<&Topology>,
    now_ms: i64,
) -> AppResult<Topology> {
    let cluster = match fetch_cluster(http, &cluster_bases(settings, previous)).await {
        Ok(c) => c,
        Err(e) => {
            // Первое подключение — ошибка; дальше — прошлые участники как недоступные.
            let Some(prev) = previous else { return Err(e) };
            let mut t = prev.clone();
            t.observed_at_ms = now_ms;
            for m in &mut t.members {
                m.reachable = false;
                m.error = Some(e.clone());
            }
            t.warnings = vec![TopologyWarning::PatroniUnreachable];
            return Ok(t);
        }
    };
    let mut members = members_from_cluster(&cluster, settings);
    let details = futures_util::future::join_all(members.iter().map(|m| {
        let base = m.api_url.clone();
        async move {
            let base = base?;
            let url = format!("{base}/patroni");
            Some(http.get_json(&url).await.and_then(|v| {
                serde_json::from_value::<PatroniJson>(v).map_err(|e| bad_json(&url, e))
            }))
        }
    }))
    .await;
    let mut warnings = Vec::new();
    for (m, d) in members.iter_mut().zip(details) {
        match d {
            Some(Ok(p)) => {
                m.server_version = p.server_version.map(format_server_version);
                m.patroni_version = p.patroni.and_then(|x| x.version);
                m.dcs_last_seen_ms = p.dcs_last_seen.map(|s| s * 1000);
                m.system_id = p.database_system_identifier;
                m.pending_restart |= p.pending_restart.unwrap_or(false);
            }
            Some(Err(e)) => {
                m.reachable = false;
                m.error = Some(e);
                warnings.push(TopologyWarning::MemberUnreachable {
                    name: m.name.clone(),
                });
            }
            None => {}
        }
    }
    if let Some(w) = system_id_warning(&members) {
        warnings.push(w);
    }
    Ok(Topology {
        connection_id: connection_id.to_string(),
        kind: ConnectionKind::Patroni,
        observed_at_ms: now_ms,
        scope: cluster.scope.clone(),
        paused: Some(cluster.pause.unwrap_or(false)),
        members,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;
    use crate::conn::profile::{ApiTls, CertFormat, ClientCert};
    use crate::conn::testutil;
    use crate::sync::MutexExt;

    struct FakeHttp {
        answers: HashMap<String, AppResult<serde_json::Value>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeHttp {
        fn new(answers: Vec<(&str, AppResult<serde_json::Value>)>) -> Self {
            Self {
                answers: answers
                    .into_iter()
                    .map(|(u, a)| (u.to_string(), a))
                    .collect(),
                calls: Mutex::new(vec![]),
            }
        }
    }

    impl PatroniHttp for FakeHttp {
        fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, AppResult<serde_json::Value>> {
            self.calls.lock_safe().push(url.to_string());
            let r = self
                .answers
                .get(url)
                .cloned()
                .unwrap_or_else(|| Err(unreachable_err(url)));
            Box::pin(async move { r })
        }
    }

    fn unreachable_err(url: &str) -> AppError {
        app_err!(
            ErrorCode::PatroniError,
            "errors.patroni.unreachable",
            "Patroni API at {{url}} is unreachable",
            url = url
        )
    }

    fn settings(urls: &[&str]) -> PatroniSettings {
        PatroniSettings {
            urls: urls.iter().map(|u| u.to_string()).collect(),
            username: None,
            tls: None,
            host_overrides: vec![],
        }
    }

    fn cluster_v3() -> serde_json::Value {
        serde_json::json!({
            "members": [
                {"name": "pg-1", "role": "leader", "state": "running", "api_url": "http://10.0.0.1:8008/patroni",
                 "host": "10.0.0.1", "port": 5432, "timeline": 7},
                {"name": "pg-2", "role": "sync_standby", "state": "streaming", "api_url": "http://10.0.0.2:8008/patroni",
                 "host": "10.0.0.2", "port": 5432, "timeline": 7, "lag": 0},
                {"name": "pg-3", "role": "replica", "state": "streaming", "api_url": "http://10.0.0.3:8008/patroni",
                 "host": "10.0.0.3", "port": 5432, "timeline": 7, "lag": "unknown",
                 "tags": {"nofailover": true}, "pending_restart": true}
            ],
            "scope": "billing"
        })
    }

    fn patroni_json(version: i64, system_id: &str) -> serde_json::Value {
        serde_json::json!({
            "state": "running", "role": "replica", "server_version": version, "timeline": 7,
            "dcs_last_seen": 1_760_000_000, "database_system_identifier": system_id,
            "patroni": {"version": "4.0.4", "scope": "billing", "name": "x"}
        })
    }

    #[test]
    fn members_from_patroni3_cluster() {
        let c: ClusterJson = serde_json::from_value(cluster_v3()).unwrap();
        let m = members_from_cluster(&c, &settings(&["http://x:8008"]));
        assert_eq!(m.len(), 3);
        assert_eq!(
            (m[0].role, m[1].role, m[2].role),
            (Role::Primary, Role::SyncStandby, Role::Replica)
        );
        assert_eq!(m[0].api_url.as_deref(), Some("http://10.0.0.1:8008"));
        assert_eq!(m[1].lag_bytes, Some(0));
        assert_eq!(m[2].lag_bytes, None, "\"unknown\" — нет данных");
        assert_eq!(m[2].tags["nofailover"], serde_json::json!(true));
        assert!(m[2].pending_restart);
        assert_eq!(m[2].raw_role.as_deref(), Some("replica"));
    }

    #[test]
    fn members_from_patroni4_cluster_and_overrides() {
        let c: ClusterJson = serde_json::from_value(serde_json::json!({
            "members": [
                {"name": "a", "role": "leader", "state": "running", "api_url": "http://a:8008/patroni", "host": "a", "port": 5432, "timeline": 3},
                {"name": "b", "role": "quorum_standby", "state": "streaming", "api_url": "http://b:8008/patroni", "host": "b", "port": 5432,
                 "receive_lag": 10, "replay_lag": 20, "lag": 20}
            ],
            "pause": true
        }))
        .unwrap();
        let mut s = settings(&["http://a:8008"]);
        s.host_overrides = vec![HostOverride {
            member: "b".into(),
            host: Some("localhost".into()),
            port: Some(5442),
            api_url: Some("http://localhost:8012".into()),
        }];
        let m = members_from_cluster(&c, &s);
        assert_eq!(m[1].role, Role::QuorumStandby);
        assert_eq!(m[1].lag_bytes, Some(20));
        assert_eq!((m[1].host.as_str(), m[1].port), ("localhost", 5442));
        assert_eq!(m[1].api_url.as_deref(), Some("http://localhost:8012"));
        assert_eq!(c.pause, Some(true));
    }

    #[tokio::test]
    async fn discover_tries_next_url_and_fills_details() {
        let http = FakeHttp::new(vec![
            ("http://seed2:8008/cluster", Ok(cluster_v3())),
            (
                "http://10.0.0.1:8008/patroni",
                Ok(patroni_json(170_002, "7400")),
            ),
            (
                "http://10.0.0.2:8008/patroni",
                Ok(patroni_json(170_002, "7400")),
            ),
        ]);
        let t = discover(
            &http,
            &settings(&["http://seed1:8008", "http://seed2:8008/"]),
            "c",
            None,
            5,
        )
        .await
        .unwrap();
        assert_eq!(t.kind, ConnectionKind::Patroni);
        assert_eq!(t.scope.as_deref(), Some("billing"));
        assert_eq!(t.paused, Some(false));
        assert_eq!(t.observed_at_ms, 5);
        let pg1 = t.member("pg-1").unwrap();
        assert_eq!(pg1.server_version.as_deref(), Some("17.2"));
        assert_eq!(pg1.patroni_version.as_deref(), Some("4.0.4"));
        assert_eq!(pg1.dcs_last_seen_ms, Some(1_760_000_000_000));
        let pg3 = t.member("pg-3").unwrap();
        assert!(!pg3.reachable);
        assert_eq!(pg3.error.as_ref().unwrap().code, ErrorCode::PatroniError);
        assert_eq!(
            t.warnings,
            vec![TopologyWarning::MemberUnreachable {
                name: "pg-3".into()
            }]
        );
        assert_eq!(http.calls.lock_safe()[0], "http://seed1:8008/cluster");
    }

    #[tokio::test]
    async fn system_id_mismatch_is_reported() {
        let http = FakeHttp::new(vec![
            ("http://s:8008/cluster", Ok(cluster_v3())),
            (
                "http://10.0.0.1:8008/patroni",
                Ok(patroni_json(160_004, "1")),
            ),
            (
                "http://10.0.0.2:8008/patroni",
                Ok(patroni_json(160_004, "2")),
            ),
            (
                "http://10.0.0.3:8008/patroni",
                Ok(patroni_json(160_004, "1")),
            ),
        ]);
        let t = discover(&http, &settings(&["http://s:8008"]), "c", None, 1)
            .await
            .unwrap();
        assert!(matches!(
            t.warnings.as_slice(),
            [TopologyWarning::SystemIdMismatch { .. }]
        ));
    }

    #[tokio::test]
    async fn unreachable_api_fails_first_time_and_degrades_later() {
        let http = FakeHttp::new(vec![]);
        let err = discover(&http, &settings(&["http://s:8008"]), "c", None, 1)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::PatroniError);

        let ok = FakeHttp::new(vec![("http://s:8008/cluster", Ok(cluster_v3()))]);
        let prev = discover(&ok, &settings(&["http://s:8008"]), "c", None, 1)
            .await
            .unwrap();
        // Первый вызов `discover` тоже обращался к `http`: журнал считаем с чистого листа.
        http.calls.lock_safe().clear();
        let t = discover(&http, &settings(&["http://s:8008"]), "c", Some(&prev), 2)
            .await
            .unwrap();
        assert_eq!(t.warnings, vec![TopologyWarning::PatroniUnreachable]);
        assert!(t.members.iter().all(|m| !m.reachable));
        assert_eq!(t.observed_at_ms, 2);
        // Опрос начался с лидера прошлой топологии.
        assert_eq!(http.calls.lock_safe()[0], "http://10.0.0.1:8008/cluster");
    }

    /// Минимальный HTTP-сервер: на каждый путь отвечает заготовленным ответом, `/echo` возвращает заголовок авторизации.
    async fn stub_server() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let auth = req
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("authorization: ")
                                .or_else(|| l.strip_prefix("Authorization: "))
                        })
                        .unwrap_or("")
                        .to_string();
                    let (status, body) = match path.as_str() {
                        "/ok" => (200, r#"{"state":"running"}"#.to_string()),
                        "/degraded" => (503, r#"{"state":"stopped"}"#.to_string()),
                        "/denied" => (401, "no".to_string()),
                        "/garbage" => (200, "<html>".to_string()),
                        "/echo" => (200, serde_json::json!({ "auth": auth }).to_string()),
                        _ => (500, "boom".to_string()),
                    };
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn reqwest_http_maps_statuses_and_sends_basic_auth() {
        let base = stub_server().await;
        let mut s = settings(&[&base]);
        s.username = Some("patroni".into());
        let http = ReqwestHttp::new(&s, Some("secret".into()), None).unwrap();

        assert_eq!(
            http.get_json(&format!("{base}/ok")).await.unwrap()["state"],
            "running"
        );
        let degraded = http.get_json(&format!("{base}/degraded")).await.unwrap();
        assert_eq!(
            degraded["state"], "stopped",
            "503 с JSON — ответ узла с остановленным PostgreSQL"
        );
        assert_eq!(
            http.get_json(&format!("{base}/echo")).await.unwrap()["auth"],
            "Basic cGF0cm9uaTpzZWNyZXQ="
        );

        let denied = http.get_json(&format!("{base}/denied")).await.unwrap_err();
        assert_eq!(
            (denied.code, denied.key.as_str()),
            (ErrorCode::AuthFailed, "errors.patroni.unauthorized")
        );
        let garbage = http.get_json(&format!("{base}/garbage")).await.unwrap_err();
        assert_eq!(
            (garbage.code, garbage.key.as_str()),
            (ErrorCode::PatroniError, "errors.patroni.badJson")
        );
        let boom = http.get_json(&format!("{base}/boom")).await.unwrap_err();
        assert_eq!(
            (boom.code, boom.key.as_str()),
            (ErrorCode::PatroniError, "errors.patroni.httpStatus")
        );
        assert_eq!(boom.params["status"], "500");
        assert_eq!(boom.details.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn reqwest_http_reports_refused_connection() {
        let port = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let http = ReqwestHttp::new(&settings(&["http://x"]), None, None).unwrap();
        let err = http
            .get_json(&format!("http://127.0.0.1:{port}/cluster"))
            .await
            .unwrap_err();
        assert_eq!(
            (err.code, err.key.as_str()),
            (ErrorCode::PatroniError, "errors.patroni.unreachable")
        );
        assert!(err.details.is_some());
    }

    /// HTTPS-сервер на 127.0.0.1 (отвечает `{"state":"running"}`); `ca` — если задан, требует клиентский сертификат от него.
    fn https_stub(server: &testutil::Issued, client_ca: Option<&testutil::Issued>) -> u16 {
        use openssl::ssl::{SslAcceptor, SslMethod, SslVerifyMode};
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
        acceptor.set_private_key(&server.key).unwrap();
        acceptor.set_certificate(&server.cert).unwrap();
        if let Some(ca) = client_ca {
            acceptor.cert_store_mut().add_cert(ca.cert.clone()).unwrap();
            acceptor.set_verify(SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT);
        }
        let acceptor = acceptor.build();
        std::thread::spawn(move || {
            while let Ok((tcp, _)) = listener.accept() {
                let Ok(mut tls) = acceptor.accept(tcp) else {
                    continue;
                };
                let mut buf = [0u8; 4096];
                let _ = tls.read(&mut buf);
                let body = r#"{"state":"running"}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(resp.as_bytes());
                let _ = tls.shutdown();
            }
        });
        port
    }

    fn pem_file(dir: &std::path::Path, name: &str, data: &[u8]) -> String {
        let p = dir.join(name);
        std::fs::write(&p, data).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn tls_settings(
        ca: &str,
        verify_hostname: bool,
        client: Option<ClientCert>,
    ) -> PatroniSettings {
        let mut s = settings(&["https://localhost"]);
        s.tls = Some(ApiTls {
            ca_path: Some(ca.into()),
            verify_hostname,
            client_cert: client,
        });
        s
    }

    #[tokio::test]
    async fn reqwest_http_tls_ca_hostname_and_client_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let ca = testutil::make_ca("patroni-test-ca");
        let ca_path = pem_file(dir.path(), "ca.pem", &ca.cert.to_pem().unwrap());
        let url = |port: u16| format!("https://localhost:{port}/patroni");

        // Сертификат выдан на localhost, CA указан — запрос проходит.
        let port = https_stub(&testutil::make_server_cert(&ca, "localhost"), None);
        let http = ReqwestHttp::new(&tls_settings(&ca_path, true, None), None, None).unwrap();
        assert_eq!(http.get_json(&url(port)).await.unwrap()["state"], "running");

        // Чужой CA: сертификат не доверенный — TlsError, даже если проверка имени выключена.
        let other_ca = testutil::make_ca("other-ca");
        let other_path = pem_file(dir.path(), "other.pem", &other_ca.cert.to_pem().unwrap());
        let http = ReqwestHttp::new(&tls_settings(&other_path, false, None), None, None).unwrap();
        let err = http.get_json(&url(port)).await.unwrap_err();
        assert_eq!(
            (err.code, err.key.as_str()),
            (ErrorCode::TlsError, "errors.patroni.tls")
        );

        // Имя в сертификате не совпадает: с проверкой — TlsError, без неё — ответ.
        let port = https_stub(&testutil::make_server_cert(&ca, "other.example"), None);
        let strict = ReqwestHttp::new(&tls_settings(&ca_path, true, None), None, None).unwrap();
        let err = strict.get_json(&url(port)).await.unwrap_err();
        assert_eq!(
            (err.code, err.key.as_str()),
            (ErrorCode::TlsError, "errors.patroni.tls")
        );
        let lax = ReqwestHttp::new(&tls_settings(&ca_path, false, None), None, None).unwrap();
        assert_eq!(lax.get_json(&url(port)).await.unwrap()["state"], "running");

        // Сервер требует клиентский сертификат (mTLS).
        let port = https_stub(&testutil::make_server_cert(&ca, "localhost"), Some(&ca));
        let client = testutil::make_server_cert(&ca, "glafira-client");
        let mut chain_pem = client.cert.to_pem().unwrap();
        chain_pem.extend(ca.cert.to_pem().unwrap());
        let cert_path = pem_file(dir.path(), "client.pem", &chain_pem);
        let key_path = pem_file(
            dir.path(),
            "client.key",
            &client.key.private_key_to_pem_pkcs8().unwrap(),
        );
        let cc = ClientCert {
            format: CertFormat::Pem,
            certificate_path: cert_path,
            key_path: Some(key_path),
        };
        let without = ReqwestHttp::new(&tls_settings(&ca_path, true, None), None, None).unwrap();
        assert!(
            without.get_json(&url(port)).await.is_err(),
            "без клиентского сертификата сервер отказывает"
        );
        let with = ReqwestHttp::new(&tls_settings(&ca_path, true, Some(cc)), None, None).unwrap();
        assert_eq!(with.get_json(&url(port)).await.unwrap()["state"], "running");
    }

    #[test]
    fn client_builds_without_ca_file_and_hostname_check() {
        // Без CA-файла и без проверки имени системные корни подставляются явно (требование reqwest/rustls).
        let mut s = settings(&["https://h:8008"]);
        s.tls = Some(ApiTls {
            ca_path: None,
            verify_hostname: false,
            client_cert: None,
        });
        ReqwestHttp::new(&s, None, None).unwrap();
        s.tls = Some(ApiTls {
            ca_path: None,
            verify_hostname: true,
            client_cert: None,
        });
        ReqwestHttp::new(&s, None, None).unwrap();
    }

    #[test]
    fn bases_order_and_formats() {
        assert_eq!(api_base("http://h:8008/patroni"), "http://h:8008");
        assert_eq!(api_base("https://h:8008/"), "https://h:8008");
        assert_eq!(format_server_version(170_002), "17.2");
        assert_eq!(format_server_version(130_015), "13.15");
        assert_eq!(format_server_version(90_624), "9.6.24");
        assert_eq!(map_role(Some("master")), Role::Primary);
        assert_eq!(map_role(Some("standby_leader")), Role::StandbyLeader);
        assert_eq!(map_role(Some("weird")), Role::Unknown);
        assert_eq!(map_role(None), Role::Unknown);
    }
}
