//! Клиент Patroni REST API (спец. P1 §6.2): участники — `GET /cluster`, детали — `GET /patroni`.

use std::net::SocketAddr;
use std::time::Duration;

use serde::Deserialize;

use crate::conn::errors::{chain, stream_closed};
use crate::conn::profile::{ConnectionKind, PatroniSettings};
use crate::conn::tls;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::sync::MutexExt;
use crate::topology::BoxFuture;
use crate::topology::model::{Member, Role, Topology, TopologyWarning, system_id_warning};

pub const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
/// Общий предел обхода адресов `/cluster`: адресов API может быть много (профиль плюс прежние участники),
/// и каждый ждёт до `HTTP_TIMEOUT` — без общего срока один опрос мог бы длиться дольше минуты. Запросы
/// `/patroni` к участникам после обхода идут параллельно, и каждый ограничен своим `HTTP_TIMEOUT`.
pub const DISCOVER_TIMEOUT: Duration = Duration::from_secs(20);
/// Сколько текста ответа с ошибкой сохраняется в `details`.
const BODY_SNIPPET: usize = 500;

/// GET JSON из Patroni; трейт — чтобы подменять сеть в тестах.
pub trait PatroniHttp: Send + Sync {
    fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, AppResult<serde_json::Value>>;
}

pub struct ReqwestHttp {
    client: reqwest::Client,
    auth: Option<(String, String)>,
    /// Все адреса API в профиле — https. Тогда пароль не уходит по адресу слабее (`http://` участника, который
    /// прислал `/cluster`): это было бы понижение защиты для учётных данных.
    https_only: bool,
    /// Адреса, о которых уже предупредили в журнале: опрос идёт каждые 10 с.
    warned: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Запросы идут через SOCKS5 туннеля ssh (`socks5h`: имена разрешает бастион).
    proxy: Option<SocketAddr>,
    /// Цель ssh туннеля — для сообщения о сбое проброса (`with_ssh_target`).
    ssh_target: String,
}

fn is_https(url: &str) -> bool {
    url.trim_start()
        .get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
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
    /// Блокирующий вызов (файлы сертификатов). `password` — секрет `patroniPassword`; `proxy` — порт SOCKS5
    /// туннеля подключения (адреса API разрешает бастион, TLS проверяется по ним же).
    pub fn new(
        settings: &PatroniSettings,
        password: Option<String>,
        key_password: Option<&str>,
        proxy: Option<SocketAddr>,
    ) -> AppResult<Self> {
        let mut b = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .connect_timeout(HTTP_TIMEOUT);
        if let Some(p) = proxy {
            b = b.proxy(reqwest::Proxy::all(format!("socks5h://{p}")).map_err(http_setup)?);
        }
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
        let mut profile_urls = settings.urls.iter().chain(
            settings
                .host_overrides
                .iter()
                .filter_map(|o| o.api_url.as_ref()),
        );
        Ok(Self {
            client: b.build().map_err(http_setup)?,
            auth: settings
                .username
                .clone()
                .map(|u| (u, password.unwrap_or_default())),
            https_only: profile_urls.all(|u| is_https(u)),
            warned: Default::default(),
            proxy,
            ssh_target: String::new(),
        })
    }

    /// Цель ssh туннеля для сообщений о сбое проброса.
    pub fn with_ssh_target(mut self, target: &str) -> Self {
        self.ssh_target = target.to_string();
        self
    }

    /// Ошибка запроса. Через туннель обрыв потока (`ssh -D` закрывает соединение, когда бастион не достучался до
    /// узла: клиент OpenSSH 10.x — посреди рукопожатия SOCKS5, старый — сразу после его успеха) — сбой проброса, отказ порта туннеля (процесс ssh умер) — ошибка SOCKS; срок и TLS — как обычно.
    fn request_error(&self, url: &str, e: &reqwest::Error) -> AppError {
        if self.proxy.is_some() && !e.is_timeout() && !is_certificate_error(e) {
            let parsed = reqwest::Url::parse(url).ok();
            let host = parsed
                .as_ref()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_default();
            let port = parsed
                .as_ref()
                .and_then(|u| u.port_or_known_default())
                .unwrap_or_default();
            let cause = format!(
                "{}: {}",
                crate::conn::profile::strip_url_credentials(url),
                chain(e)
            );
            if refused(e) {
                return crate::net::ssh::socks::socks_error(&host, port, &cause);
            }
            if stream_closed(e) || handshake_dropped(e) {
                return crate::net::ssh::socks::forward_failed(
                    &self.ssh_target,
                    &host,
                    port,
                    &cause,
                );
            }
        }
        request_error(url, e)
    }

    /// Пароль API не отправляется на этот адрес: профиль только https, а адрес — нет.
    fn withholds_credentials(&self, url: &str) -> bool {
        self.auth.is_some() && self.https_only && !is_https(url)
    }
}

fn is_certificate_error(e: &reqwest::Error) -> bool {
    chain(e).to_lowercase().contains("certificate")
}

/// Не удалось открыть TCP-соединение с прокси — портом туннеля (через `socks5h` клиент сам соединяется только с
/// ним). SOCKS-клиент reqwest (hyper-util) не отдаёт исходную ошибку ввода-вывода в цепочке причин, поэтому
/// узнаётся и по его тексту.
fn refused(e: &reqwest::Error) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if err
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::ConnectionRefused)
        {
            return true;
        }
        cur = err.source();
    }
    chain(e).contains("failed to create underlying connection")
}

/// Порт туннеля принял соединение, но рукопожатие SOCKS5 оборвалось (клиент reqwest/hyper-util пишет «io error
/// during SOCKS handshake» и не отдаёт исходную ошибку ввода-вывода в цепочке причин). OpenSSH 10.x отвечает SOCKS5
/// только после открытия канала к узлу, а когда бастион до узла не достучался (имя не разрешилось, порт закрыт),
/// закрывает соединение без ответа. Кроме этого текста сбоем проброса считается любая ошибка SOCKS-клиента в цепочке
/// («SOCKS error: …»: ответ SOCKS с отказом, разбор ответа в других версиях клиента), кроме отказа соединиться с
/// самим портом туннеля — тот остаётся делом `refused`.
fn handshake_dropped(e: &reqwest::Error) -> bool {
    socks_failure_text(&chain(e))
}

/// Текст цепочки причин — сбой SOCKS-клиента после соединения с портом туннеля (чистая функция: проверяется без сети).
fn socks_failure_text(text: &str) -> bool {
    if text.contains("failed to create underlying connection") {
        return false;
    }
    text.contains("io error during SOCKS handshake") || text.contains("SOCKS error:")
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
    } else if is_certificate_error(e) {
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

/// `/patroni` у узла с остановленным PostgreSQL отвечает 503, но с обычным JSON состояния
/// (`state`, `role`). Любой другой 503 — например, `{"message": "Service Unavailable"}` от шлюза
/// или 503 на `/cluster` — остаётся ошибкой, чтобы опрос перешёл к следующему адресу.
fn is_patroni_state(url: &str, v: &serde_json::Value) -> bool {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    path.ends_with("/patroni")
        && v.as_object()
            .is_some_and(|o| o.contains_key("state") || o.contains_key("role"))
}

impl PatroniHttp for ReqwestHttp {
    fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, AppResult<serde_json::Value>> {
        Box::pin(async move {
            let mut req = self.client.get(url);
            let withheld = self.withholds_credentials(url);
            if withheld {
                let clean = crate::conn::profile::strip_url_credentials(url);
                if self.warned.lock_safe().insert(clean.clone()) {
                    tracing::warn!(
                        "Patroni API {clean}: not https while the connection uses https only; basic auth is not sent"
                    );
                }
            } else if let Some((u, p)) = &self.auth {
                req = req.basic_auth(u, Some(p));
            }
            let resp = req.send().await.map_err(|e| self.request_error(url, &e))?;
            let status = resp.status();
            let body = resp.text().await.map_err(|e| self.request_error(url, &e))?;
            let parsed = serde_json::from_str::<serde_json::Value>(&body);
            match (status.as_u16(), parsed) {
                (200..=299, Ok(v)) => Ok(v),
                (503, Ok(v)) if is_patroni_state(url, &v) => Ok(v),
                (200..=299, Err(e)) => Err(bad_json(url, e)),
                (401 | 403, _) if withheld => Err(app_err!(
                    ErrorCode::AuthFailed,
                    "errors.patroni.credentialsWithheld",
                    "The Patroni API at {{url}} asks for credentials over plain HTTP; they are not sent because the connection uses https: set an https API address for the member in “Node address overrides”",
                    url = url
                )),
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
    /// Обязательное поле: Patroni присылает его всегда, даже `[]`. Без него ответ — не Patroni
    /// (шлюз, чужой сервис), и пустой кластер из него делать нельзя.
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
/// Сегмент `/patroni` отбрасывается только в пути: хост может называться `patroni`.
pub fn api_base(url: &str) -> String {
    let url = url.trim();
    let (scheme, rest) = url.find("://").map_or(("", url), |i| url.split_at(i + 3));
    let (authority, path) = rest.find('/').map_or((rest, ""), |i| rest.split_at(i));
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix("/patroni").unwrap_or(path);
    format!("{scheme}{authority}{}", path.trim_end_matches('/'))
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

/// Участники из `/cluster` с подменой адресов из профиля.
pub fn members_from_cluster(c: &ClusterJson, settings: &PatroniSettings) -> Vec<Member> {
    c.members
        .iter()
        .map(|m| {
            let ov = settings.host_overrides.iter().find(|o| o.member == m.name);
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

/// Порядок опроса `/cluster` без повторов. Обычно (спец. P1 §6.2) — лидер прошлого опроса, остальные участники,
/// затем адреса профиля. Участники, чей API в прошлый раз не ответил (`reachable == false`), идут после адресов
/// профиля. После `PatroniUnreachable` недоступны все участники, и первыми идут адреса профиля. `api_url` из
/// `/cluster` бывают недоступны снаружи (адреса подов за NAT или в Kubernetes без подмены). Если обходить такие
/// адреса первыми, они съедают весь `DISCOVER_TIMEOUT` раньше, чем очередь дойдёт до рабочего адреса профиля.
/// Тогда подключение не восстановится до переоткрытия вручную, а при правиле «только после `PatroniUnreachable`»
/// опросы чередовались бы: удачный, провал по сроку, снова удачный.
pub fn cluster_bases(settings: &PatroniSettings, previous: Option<&Topology>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |u: &str| {
        let b = api_base(u);
        if !b.is_empty() && !out.contains(&b) {
            out.push(b);
        }
    };
    // Участники прошлого опроса, лидер первым (повтор адреса отсекает `add`).
    let members: Vec<&Member> = previous
        .map(|t| t.leader().into_iter().chain(&t.members).collect())
        .unwrap_or_default();
    let api_urls = |answered: bool| {
        members
            .iter()
            .filter(move |m| m.reachable == answered)
            .filter_map(|m| m.api_url.as_deref())
    };
    for u in api_urls(true) {
        add(u);
    }
    for u in &settings.urls {
        add(u);
    }
    for u in api_urls(false) {
        add(u);
    }
    out
}

/// Первый ответ `/cluster` по адресам `bases`. В `tried` — адреса, к которым уже обращались (вместе с тем, чей
/// ответ ещё ждут): по ним видно, где повис обход, если его прервал общий срок.
async fn fetch_cluster(
    http: &dyn PatroniHttp,
    bases: &[String],
    tried: &mut Vec<String>,
) -> AppResult<ClusterJson> {
    let mut last = None;
    for base in bases {
        tried.push(crate::conn::profile::strip_url_credentials(base));
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
    let bases = cluster_bases(settings, previous);
    let mut tried = Vec::new();
    let fetched =
        tokio::time::timeout(DISCOVER_TIMEOUT, fetch_cluster(http, &bases, &mut tried)).await;
    let fetched = match fetched {
        Ok(r) => r,
        Err(_) => Err(app_err!(
            ErrorCode::PatroniError,
            "errors.patroni.discoverTimeout",
            "Patroni API did not answer within {{secs}} s",
            secs = DISCOVER_TIMEOUT.as_secs()
        )
        .with_details(format!(
            "tried: {} (deadline {} s)",
            tried.join(", "),
            DISCOVER_TIMEOUT.as_secs()
        ))),
    };
    let cluster = match fetched {
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
    use crate::conn::profile::{ApiTls, CertFormat, ClientCert, HostOverride};
    use crate::conn::testutil;

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

    /// Кластер `cluster_v3()` по адресу `http://s:8008`, API всех трёх участников отвечает.
    fn healthy_v3() -> FakeHttp {
        FakeHttp::new(vec![
            ("http://s:8008/cluster", Ok(cluster_v3())),
            (
                "http://10.0.0.1:8008/patroni",
                Ok(patroni_json(170_002, "7400")),
            ),
            (
                "http://10.0.0.2:8008/patroni",
                Ok(patroni_json(170_002, "7400")),
            ),
            (
                "http://10.0.0.3:8008/patroni",
                Ok(patroni_json(170_002, "7400")),
            ),
        ])
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
                 "receive_lag": 10, "replay_lag": 20, "lag": 30}
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
        assert_eq!(m[1].lag_bytes, Some(20), "replay_lag важнее lag");
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

        let prev = discover(&healthy_v3(), &settings(&["http://s:8008"]), "c", None, 1)
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

    /// На каждый запрос «думает» `delay` и отвечает отказом.
    struct SlowHttp {
        delay: Duration,
    }

    impl PatroniHttp for SlowHttp {
        fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, AppResult<serde_json::Value>> {
            Box::pin(async move {
                tokio::time::sleep(self.delay).await;
                Err(unreachable_err(url))
            })
        }
    }

    /// Адресов много, каждый ждёт до HTTP_TIMEOUT: обнаружение ограничено общим сроком.
    #[tokio::test(start_paused = true)]
    async fn discover_has_an_overall_deadline() {
        let urls: Vec<String> = (1..=8).map(|i| format!("http://pg-{i}:8008")).collect();
        let s = settings(&urls.iter().map(String::as_str).collect::<Vec<_>>());
        let http = SlowHttp {
            delay: HTTP_TIMEOUT,
        };
        let started = tokio::time::Instant::now();
        let e = discover(&http, &s, "c", None, 0).await.unwrap_err();
        assert_eq!(e.key, "errors.patroni.discoverTimeout");
        assert_eq!(e.code, ErrorCode::PatroniError);
        assert_eq!(
            started.elapsed(),
            DISCOVER_TIMEOUT,
            "а не 8 × {HTTP_TIMEOUT:?}"
        );

        // С прежней топологией — как при любом отказе API: участники недоступны, предупреждение одно.
        let prev = discover(
            &FakeHttp::new(vec![("http://s:8008/cluster", Ok(cluster_v3()))]),
            &settings(&["http://s:8008"]),
            "c",
            None,
            0,
        )
        .await
        .unwrap();
        let started = tokio::time::Instant::now();
        let t = discover(&http, &s, "c", Some(&prev), 1).await.unwrap();
        assert_eq!(started.elapsed(), DISCOVER_TIMEOUT);
        assert_eq!(t.warnings, vec![TopologyWarning::PatroniUnreachable]);
        assert_eq!(t.members.len(), 3);
        assert!(
            t.members.iter().all(|m| !m.reachable
                && m.error.as_ref().unwrap().key == "errors.patroni.discoverTimeout")
        );
    }

    /// Адреса с префиксом из `hanging` «думают» `delay` и отвечают отказом, остальные — как `answers`.
    struct HangingHttp {
        answers: FakeHttp,
        hanging: Vec<String>,
        delay: Duration,
    }

    impl PatroniHttp for HangingHttp {
        fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, AppResult<serde_json::Value>> {
            if !self
                .hanging
                .iter()
                .any(|b| url.starts_with(&format!("{b}/")))
            {
                return self.answers.get_json(url);
            }
            self.answers.calls.lock_safe().push(url.to_string());
            Box::pin(async move {
                tokio::time::sleep(self.delay).await;
                Err(unreachable_err(url))
            })
        }
    }

    /// Пять участников, лидер — `pg-1`; `api_url` — адреса подов `http://10.0.0.N:8008`.
    fn cluster_of_five() -> serde_json::Value {
        let members: Vec<serde_json::Value> = (1..=5)
            .map(|i| {
                serde_json::json!({
                    "name": format!("pg-{i}"),
                    "role": if i == 1 { "leader" } else { "replica" },
                    "state": if i == 1 { "running" } else { "streaming" },
                    "api_url": format!("http://10.0.0.{i}:8008/patroni"),
                    "host": format!("10.0.0.{i}"),
                    "port": 5432
                })
            })
            .collect();
        serde_json::json!({"members": members, "scope": "billing"})
    }

    /// `api_url` участников перестали отвечать снаружи (поды за NAT), а адрес профиля работает. Обход
    /// участников в порядке спецификации упирается в общий срок, но следующий опрос начинает с адреса профиля
    /// и восстанавливается в пределах срока. И опрос после него тоже: участники, не ответившие в прошлый раз,
    /// идут после адресов профиля, поэтому опросы не чередуются «удачный — провал по сроку».
    #[tokio::test(start_paused = true)]
    async fn discover_recovers_through_profile_url_when_member_api_urls_hang() {
        let s = settings(&["http://lb:8008"]);
        let pods: Vec<String> = (1..=5).map(|i| format!("http://10.0.0.{i}:8008")).collect();
        let mut answers = vec![("http://lb:8008/cluster", Ok(cluster_of_five()))];
        let patroni_urls: Vec<String> = pods.iter().map(|b| format!("{b}/patroni")).collect();
        for u in &patroni_urls {
            answers.push((u.as_str(), Ok(patroni_json(170_002, "7400"))));
        }
        // Первое подключение: всё отвечает.
        let healthy = FakeHttp::new(answers);
        let prev = discover(&healthy, &s, "c", None, 1).await.unwrap();
        assert!(prev.members.iter().all(|m| m.reachable));
        assert!(prev.warnings.is_empty(), "{:?}", prev.warnings);

        let http = HangingHttp {
            answers: FakeHttp::new(vec![("http://lb:8008/cluster", Ok(cluster_of_five()))]),
            hanging: pods.clone(),
            delay: HTTP_TIMEOUT,
        };
        // Участники обходятся первыми (лидер прошлого опроса — первым) и съедают весь срок.
        let started = tokio::time::Instant::now();
        let failed = discover(&http, &s, "c", Some(&prev), 2).await.unwrap();
        assert_eq!(started.elapsed(), DISCOVER_TIMEOUT);
        assert_eq!(failed.warnings, vec![TopologyWarning::PatroniUnreachable]);
        let e = failed.members[0].error.clone().unwrap();
        assert_eq!(e.key, "errors.patroni.discoverTimeout");
        // В подробностях — адреса по порядку обхода: видно, какие повисли.
        assert_eq!(
            e.details.as_deref(),
            Some(
                "tried: http://10.0.0.1:8008, http://10.0.0.2:8008, http://10.0.0.3:8008, \
                 http://10.0.0.4:8008, http://10.0.0.5:8008 (deadline 20 s)"
            )
        );

        // Следующий опрос начинается с адреса профиля и укладывается в срок.
        http.answers.calls.lock_safe().clear();
        let started = tokio::time::Instant::now();
        let recovered = discover(&http, &s, "c", Some(&failed), 3).await.unwrap();
        assert!(
            started.elapsed() < DISCOVER_TIMEOUT,
            "{:?}",
            started.elapsed()
        );
        assert_eq!(http.answers.calls.lock_safe()[0], "http://lb:8008/cluster");
        assert!(
            !recovered
                .warnings
                .contains(&TopologyWarning::PatroniUnreachable),
            "{:?}",
            recovered.warnings
        );
        assert_eq!(recovered.members.len(), 5);
        assert_eq!(recovered.observed_at_ms, 3);
        // `/patroni` участников по-прежнему молчит: это видно как недоступность участников, а не всего API.
        assert!(recovered.members.iter().all(|m| !m.reachable));

        // И следующий за ним опрос тоже не упирается в срок.
        http.answers.calls.lock_safe().clear();
        let started = tokio::time::Instant::now();
        let again = discover(&http, &s, "c", Some(&recovered), 4).await.unwrap();
        assert!(
            started.elapsed() < DISCOVER_TIMEOUT,
            "{:?}",
            started.elapsed()
        );
        assert_eq!(http.answers.calls.lock_safe()[0], "http://lb:8008/cluster");
        assert!(
            !again
                .warnings
                .contains(&TopologyWarning::PatroniUnreachable),
            "{:?}",
            again.warnings
        );
    }

    /// Кластер из трёх участников, лидер — второй (не первый в списке).
    fn cluster_leader_second() -> serde_json::Value {
        serde_json::json!({
            "members": [
                {"name": "pg-1", "role": "replica", "state": "streaming", "api_url": "http://10.0.0.1:8008/patroni",
                 "host": "10.0.0.1", "port": 5432},
                {"name": "pg-2", "role": "leader", "state": "running", "api_url": "http://10.0.0.2:8008/patroni",
                 "host": "10.0.0.2", "port": 5432},
                {"name": "pg-3", "role": "replica", "state": "streaming", "api_url": "http://10.0.0.3:8008/patroni",
                 "host": "10.0.0.3", "port": 5432}
            ],
            "scope": "billing"
        })
    }

    #[tokio::test]
    async fn later_polls_start_with_leader_then_members_then_profile_urls() {
        let ok = FakeHttp::new(vec![
            ("http://s:8008/cluster", Ok(cluster_leader_second())),
            (
                "http://10.0.0.1:8008/patroni",
                Ok(patroni_json(170_002, "1")),
            ),
            (
                "http://10.0.0.2:8008/patroni",
                Ok(patroni_json(170_002, "1")),
            ),
            (
                "http://10.0.0.3:8008/patroni",
                Ok(patroni_json(170_002, "1")),
            ),
        ]);
        // Профиль: адрес одного из участников (повтор) и посторонний шлюз.
        let s = settings(&[
            "http://10.0.0.3:8008/patroni",
            "http://s:8008",
            "http://gw:8008/",
        ]);
        let prev = discover(&ok, &s, "c", None, 1).await.unwrap();
        assert_eq!(prev.leader().unwrap().name, "pg-2");
        assert_eq!(prev.members[0].name, "pg-1", "лидер не первый в списке");
        assert!(prev.members.iter().all(|m| m.reachable));

        let expected = [
            "http://10.0.0.2:8008",
            "http://10.0.0.1:8008",
            "http://10.0.0.3:8008",
            "http://s:8008",
            "http://gw:8008",
        ];
        assert_eq!(cluster_bases(&s, Some(&prev)), expected);
        assert_eq!(
            cluster_bases(&s, None),
            ["http://10.0.0.3:8008", "http://s:8008", "http://gw:8008"],
            "без прошлой топологии — только адреса профиля"
        );

        // Все адреса молчат: `discover` обходит их ровно в этом порядке.
        let down = FakeHttp::new(vec![]);
        let t = discover(&down, &s, "c", Some(&prev), 2).await.unwrap();
        assert_eq!(t.warnings, vec![TopologyWarning::PatroniUnreachable]);
        let calls: Vec<String> = down.calls.lock_safe().clone();
        let want: Vec<String> = expected.iter().map(|b| format!("{b}/cluster")).collect();
        assert_eq!(calls, want);

        // Участник, чей API не ответил, — после адресов профиля; остальные на прежних местах.
        let mut one_down = prev.clone();
        one_down.members[0].reachable = false;
        assert_eq!(
            cluster_bases(&s, Some(&one_down)),
            [
                "http://10.0.0.2:8008",
                "http://10.0.0.3:8008",
                "http://s:8008",
                "http://gw:8008",
                "http://10.0.0.1:8008",
            ]
        );
        // После `PatroniUnreachable` недоступны все участники: первыми — адреса профиля, затем лидер и остальные.
        assert_eq!(
            cluster_bases(&s, Some(&t)),
            [
                "http://10.0.0.3:8008",
                "http://s:8008",
                "http://gw:8008",
                "http://10.0.0.2:8008",
                "http://10.0.0.1:8008",
            ]
        );
    }

    #[tokio::test]
    async fn cluster_answer_without_members_is_an_error_and_next_url_is_tried() {
        // Шлюз ответил `{}` — это не Patroni; пустой кластер из него не получается.
        let only_empty = FakeHttp::new(vec![("http://s:8008/cluster", Ok(serde_json::json!({})))]);
        let err = discover(&only_empty, &settings(&["http://s:8008"]), "c", None, 1)
            .await
            .unwrap_err();
        assert_eq!(
            (err.code, err.key.as_str()),
            (ErrorCode::PatroniError, "errors.patroni.badJson")
        );

        // Первый адрес отвечает `{}`, второй — настоящим кластером.
        let http = FakeHttp::new(vec![
            ("http://gw:8008/cluster", Ok(serde_json::json!({}))),
            ("http://s:8008/cluster", Ok(cluster_v3())),
        ]);
        let t = discover(
            &http,
            &settings(&["http://gw:8008", "http://s:8008"]),
            "c",
            None,
            1,
        )
        .await
        .unwrap();
        assert_eq!(t.members.len(), 3);
        assert_eq!(
            http.calls.lock_safe()[..2],
            ["http://gw:8008/cluster", "http://s:8008/cluster"]
        );

        // Настоящий кластер без участников (`"members": []`) — допустимый ответ.
        let empty = FakeHttp::new(vec![(
            "http://s:8008/cluster",
            Ok(serde_json::json!({"members": [], "scope": "x"})),
        )]);
        let t = discover(&empty, &settings(&["http://s:8008"]), "c", None, 1)
            .await
            .unwrap();
        assert!(t.members.is_empty());
    }

    #[tokio::test]
    async fn later_poll_with_members_less_answers_degrades() {
        let ok = FakeHttp::new(vec![("http://s:8008/cluster", Ok(cluster_v3()))]);
        let s = settings(&["http://s:8008"]);
        let prev = discover(&ok, &s, "c", None, 1).await.unwrap();

        let empty = serde_json::json!({});
        let gateway = FakeHttp::new(vec![
            ("http://10.0.0.1:8008/cluster", Ok(empty.clone())),
            ("http://10.0.0.2:8008/cluster", Ok(empty.clone())),
            ("http://10.0.0.3:8008/cluster", Ok(empty.clone())),
            ("http://s:8008/cluster", Ok(empty)),
        ]);
        let t = discover(&gateway, &s, "c", Some(&prev), 2).await.unwrap();
        assert_eq!(t.warnings, vec![TopologyWarning::PatroniUnreachable]);
        assert_eq!(t.members.len(), 3, "участники прошлого опроса сохранены");
        assert!(t.members.iter().all(|m| !m.reachable));
        assert!(t.members.iter().all(|m| {
            m.error
                .as_ref()
                .is_some_and(|e| e.key == "errors.patroni.badJson")
        }));
        assert_eq!(t.observed_at_ms, 2);
    }

    #[tokio::test]
    async fn gateway_503_on_cluster_falls_through_to_next_base() {
        // Первый адрес — шлюз: 503 с JSON, но не от Patroni.
        let gateway = routes_server(vec![(
            "/cluster",
            503,
            serde_json::json!({"message": "Service Unavailable"}),
        )])
        .await;
        // Второй — настоящий Patroni (без api_url у участника: детали не запрашиваются).
        let patroni = routes_server(vec![(
            "/cluster",
            200,
            serde_json::json!({"members": [
                {"name": "pg-1", "role": "leader", "state": "running", "host": "10.0.0.1", "port": 5432}
            ], "scope": "billing"}),
        )])
        .await;
        let s = settings(&[&gateway, &patroni]);
        let http = ReqwestHttp::new(&s, None, None, None).unwrap();
        let t = discover(&http, &s, "c", None, 1).await.unwrap();
        assert_eq!(t.members.len(), 1);
        assert_eq!(t.leader().unwrap().name, "pg-1");
        assert_eq!(t.scope.as_deref(), Some("billing"));

        // 200 `{}` от шлюза — тоже не кластер.
        let gateway = routes_server(vec![("/cluster", 200, serde_json::json!({}))]).await;
        let s = settings(&[&gateway, &patroni]);
        let t = discover(&http, &s, "c", None, 1).await.unwrap();
        assert_eq!(t.members.len(), 1);

        // Если настоящего Patroni нет, первый опрос — ошибка, а не пустой кластер.
        let s = settings(&[&gateway]);
        let err = discover(&http, &s, "c", None, 1).await.unwrap_err();
        assert_eq!(err.key, "errors.patroni.badJson");
    }

    /// Минимальный HTTP-сервер: `handler(путь, заголовок авторизации)` возвращает статус и тело ответа.
    async fn serve(
        handler: impl Fn(&str, &str) -> (u16, String) + Send + Sync + 'static,
    ) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let handler = std::sync::Arc::new(handler);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let handler = handler.clone();
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
                    let (status, body) = handler(&path, &auth);
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

    /// Сервер с заготовленными ответами по путям; неизвестный путь — 500.
    async fn routes_server(routes: Vec<(&str, u16, serde_json::Value)>) -> String {
        let routes: HashMap<String, (u16, String)> = routes
            .into_iter()
            .map(|(p, status, body)| (p.to_string(), (status, body.to_string())))
            .collect();
        serve(move |path, _| {
            routes
                .get(path)
                .cloned()
                .unwrap_or((500, "boom".to_string()))
        })
        .await
    }

    /// Сервер для проверки соответствия статусов ответа ошибкам; `/echo` возвращает заголовок авторизации.
    async fn stub_server() -> String {
        serve(|path, auth| match path {
            "/ok" => (200, r#"{"state":"running"}"#.to_string()),
            "/patroni" => (503, r#"{"state":"stopped","role":"replica"}"#.to_string()),
            "/denied" => (401, "no".to_string()),
            "/forbidden" => (403, "no".to_string()),
            "/garbage" => (200, "<html>".to_string()),
            "/unavailable" => (503, "<html>bad gateway</html>".to_string()),
            // Шлюз перед Patroni: 503 с JSON, но не от Patroni.
            "/cluster" => (503, r#"{"state":"stopped"}"#.to_string()),
            "/gateway/patroni" => (503, r#"{"message":"Service Unavailable"}"#.to_string()),
            "/echo" => (200, serde_json::json!({ "auth": auth }).to_string()),
            _ => (500, "boom".to_string()),
        })
        .await
    }

    #[tokio::test]
    async fn reqwest_http_maps_statuses_and_sends_basic_auth() {
        let base = stub_server().await;
        let mut s = settings(&[&base]);
        s.username = Some("patroni".into());
        let http = ReqwestHttp::new(&s, Some("secret".into()), None, None).unwrap();

        assert_eq!(
            http.get_json(&format!("{base}/ok")).await.unwrap()["state"],
            "running"
        );
        let degraded = http.get_json(&format!("{base}/patroni")).await.unwrap();
        assert_eq!(
            degraded["state"], "stopped",
            "503 с JSON на /patroni — ответ узла с остановленным PostgreSQL"
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
        let forbidden = http
            .get_json(&format!("{base}/forbidden"))
            .await
            .unwrap_err();
        assert_eq!(
            (forbidden.code, forbidden.key.as_str()),
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

        // 503 не с JSON — обычная ошибка статуса, а не ответ узла.
        let unavailable = http
            .get_json(&format!("{base}/unavailable"))
            .await
            .unwrap_err();
        assert_eq!(
            (unavailable.code, unavailable.key.as_str()),
            (ErrorCode::PatroniError, "errors.patroni.httpStatus")
        );
        assert_eq!(unavailable.params["status"], "503");
    }

    #[tokio::test]
    async fn credentials_are_not_sent_to_a_weaker_scheme_than_the_profile() {
        let base = stub_server().await;
        // Профиль — только https; участник прислал в `/cluster` адрес http://.
        let mut s = settings(&["https://pg-1:8008", "https://pg-2:8008"]);
        s.username = Some("patroni".into());
        let http = ReqwestHttp::new(&s, Some("secret".into()), None, None).unwrap();
        assert_eq!(
            http.get_json(&format!("{base}/echo")).await.unwrap()["auth"],
            "",
            "пароль не ушёл по http"
        );
        let denied = http.get_json(&format!("{base}/denied")).await.unwrap_err();
        assert_eq!(
            (denied.code, denied.key.as_str()),
            (ErrorCode::AuthFailed, "errors.patroni.credentialsWithheld")
        );
        assert!(!format!("{denied:?}").contains("secret"));

        // Подмена адреса участника на http — выбор пользователя: профиль уже не «только https».
        s.host_overrides = vec![HostOverride {
            member: "pg-1".into(),
            host: None,
            port: None,
            api_url: Some(base.clone()),
        }];
        let mixed = ReqwestHttp::new(&s, Some("secret".into()), None, None).unwrap();
        assert_eq!(
            mixed.get_json(&format!("{base}/echo")).await.unwrap()["auth"],
            "Basic cGF0cm9uaTpzZWNyZXQ="
        );
        // Без пользователя API и не о чем говорить: 401 — обычный отказ.
        let anonymous =
            ReqwestHttp::new(&settings(&["https://pg-1:8008"]), None, None, None).unwrap();
        assert_eq!(
            anonymous
                .get_json(&format!("{base}/denied"))
                .await
                .unwrap_err()
                .key,
            "errors.patroni.unauthorized"
        );
        assert!(is_https("HTTPS://x") && is_https(" https://x") && !is_https("http://x"));
    }

    #[tokio::test]
    async fn status_503_with_json_is_only_a_patroni_node_answer() {
        let base = stub_server().await;
        let http = ReqwestHttp::new(&settings(&[&base]), None, None, None).unwrap();
        // 503 на /cluster — даже с JSON — ошибка: опрос должен перейти к следующему адресу.
        let cluster = http.get_json(&format!("{base}/cluster")).await.unwrap_err();
        assert_eq!(
            (cluster.code, cluster.key.as_str()),
            (ErrorCode::PatroniError, "errors.patroni.httpStatus")
        );
        // 503 на /patroni с JSON без `state` и `role` (шлюз) — тоже ошибка.
        let gateway = http
            .get_json(&format!("{base}/gateway/patroni"))
            .await
            .unwrap_err();
        assert_eq!(
            (gateway.code, gateway.key.as_str()),
            (ErrorCode::PatroniError, "errors.patroni.httpStatus")
        );
    }

    #[tokio::test]
    async fn reqwest_http_times_out_on_silent_server() {
        let server = testutil::silent_server();
        let http = ReqwestHttp::new(&settings(&["http://x"]), None, None, None).unwrap();
        let started = std::time::Instant::now();
        let err = http
            .get_json(&format!("http://127.0.0.1:{}/cluster", server.port))
            .await
            .unwrap_err();
        let took = started.elapsed();
        assert_eq!(
            (err.code, err.key.as_str()),
            (ErrorCode::PatroniError, "errors.patroni.timeout")
        );
        assert!(
            took >= Duration::from_secs(4) && took < Duration::from_secs(7),
            "таймаут ≈ 5 с, получилось {took:?}"
        );
    }

    #[tokio::test]
    async fn reqwest_http_reports_refused_connection() {
        let port = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let http = ReqwestHttp::new(&settings(&["http://x"]), None, None, None).unwrap();
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

    /// Через туннель адрес API разрешает бастион. Узел за бастионом недоступен (`ssh -D` закрыл поток после успеха
    /// SOCKS) — сбой проброса; порт туннеля закрыт (процесс ssh умер) — ошибка SOCKS.
    #[tokio::test]
    async fn reqwest_http_goes_through_the_tunnel() {
        use crate::conn::profile::SshAuth;
        use crate::net::ssh::tunnel::tests::{closed_port, profile as ssh_profile, registry};
        let base = stub_server().await;
        let dir = tempfile::tempdir().unwrap();
        let (tunnels, _) = registry(
            dir.path(),
            vec![(
                "FAKE_SSH_UPSTREAM",
                base.trim_start_matches("http://").into(),
            )],
        );
        let p = ssh_profile(SshAuth::Agent);
        let socks = tunnels.open(&p, Duration::from_secs(5)).await.unwrap();
        let s = settings(&["http://patroni1:8008"]);
        let http = ReqwestHttp::new(&s, None, None, Some(socks))
            .unwrap()
            .with_ssh_target("bastion");
        // Имя patroni1 локально не разрешается: ответ пришёл — значит, его разрешил прокси.
        let v = http.get_json("http://patroni1:8008/ok").await.unwrap();
        assert_eq!(v["state"], "running");
        tunnels.close("c1");

        let dir = tempfile::tempdir().unwrap();
        let (tunnels, _) = registry(
            dir.path(),
            vec![("FAKE_SSH_UPSTREAM", format!("127.0.0.1:{}", closed_port()))],
        );
        let socks = tunnels.open(&p, Duration::from_secs(5)).await.unwrap();
        let http = ReqwestHttp::new(&s, None, None, Some(socks))
            .unwrap()
            .with_ssh_target("bastion");
        let e = http
            .get_json("http://patroni1:8008/cluster")
            .await
            .unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::SshError, "errors.ssh.forwardFailed"),
            "{e:?}"
        );
        assert_eq!(
            e.params.get("kind"),
            Some(&"forwardFailed".to_string()),
            "{e:?}"
        );
        assert_eq!(e.params["target"], "bastion");
        assert!(
            e.details.as_deref().unwrap().contains("patroni1:8008"),
            "{e:?}"
        );
        tunnels.close("c1");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while tokio::net::TcpStream::connect(socks).await.is_ok() {
            assert!(std::time::Instant::now() < deadline, "ssh не завершился");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let e = http
            .get_json("http://patroni1:8008/cluster")
            .await
            .unwrap_err();
        assert_eq!(
            (
                e.key.as_str(),
                e.params["host"].as_str(),
                e.params["port"].as_str()
            ),
            ("errors.ssh.socks", "patroni1", "8008"),
            "{e:?}"
        );
    }

    /// OpenSSH 10.x отвечает SOCKS5 только после открытия канала к узлу, а когда бастион до узла не достучался,
    /// закрывает соединение без ответа на запрос CONNECT: порт туннеля принял соединение, рукопожатие оборвалось.
    /// Это сбой проброса (`forwardFailed` с узлом в `details`), а не «Patroni недоступен».
    #[tokio::test]
    async fn dropped_socks_handshake_is_a_forward_failure() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                let mut buf = [0u8; 512];
                let _ = tcp.read(&mut buf).await;
                let _ = tcp.write_all(&[5, 0]).await;
                let _ = tcp.read(&mut buf).await;
            }
        });
        let s = settings(&["http://patroni1:8008"]);
        let http = ReqwestHttp::new(&s, None, None, Some(proxy))
            .unwrap()
            .with_ssh_target("bastion");
        let e = http
            .get_json("http://patroni1:8008/cluster")
            .await
            .unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::SshError, "errors.ssh.forwardFailed"),
            "{e:?}"
        );
        assert_eq!(
            e.params.get("kind"),
            Some(&"forwardFailed".to_string()),
            "{e:?}"
        );
        assert_eq!(e.params["target"], "bastion");
        assert!(
            e.details.as_deref().unwrap().contains("patroni1:8008"),
            "{e:?}"
        );
    }

    /// Сбой SOCKS-клиента после соединения с портом туннеля — сбой проброса; отказ соединиться с самим портом
    /// туннеля («failed to create underlying connection») и посторонние ошибки — нет.
    #[test]
    fn socks_failure_text_recognizes_client_variants() {
        for text in [
            "error sending request: SOCKS error: io error during SOCKS handshake",
            "error sending request: SOCKS error: failed parsing server response: bad version",
            "error sending request: SOCKS error: host unreachable",
            "SOCKS error: could not resolve to acceptable address type",
        ] {
            assert!(socks_failure_text(text), "{text}");
        }
        for text in [
            "error sending request: SOCKS error: failed to create underlying connection: Connection refused",
            "error sending request: operation timed out",
            "error sending request: connection closed before message completed",
        ] {
            assert!(!socks_failure_text(text), "{text}");
        }
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
        let http = ReqwestHttp::new(&tls_settings(&ca_path, true, None), None, None, None).unwrap();
        assert_eq!(http.get_json(&url(port)).await.unwrap()["state"], "running");

        // Чужой CA: сертификат не доверенный — TlsError, даже если проверка имени выключена.
        let other_ca = testutil::make_ca("other-ca");
        let other_path = pem_file(dir.path(), "other.pem", &other_ca.cert.to_pem().unwrap());
        let http =
            ReqwestHttp::new(&tls_settings(&other_path, false, None), None, None, None).unwrap();
        let err = http.get_json(&url(port)).await.unwrap_err();
        assert_eq!(
            (err.code, err.key.as_str()),
            (ErrorCode::TlsError, "errors.patroni.tls")
        );

        // Имя в сертификате не совпадает: с проверкой — TlsError, без неё — ответ.
        let port = https_stub(&testutil::make_server_cert(&ca, "other.example"), None);
        let strict =
            ReqwestHttp::new(&tls_settings(&ca_path, true, None), None, None, None).unwrap();
        let err = strict.get_json(&url(port)).await.unwrap_err();
        assert_eq!(
            (err.code, err.key.as_str()),
            (ErrorCode::TlsError, "errors.patroni.tls")
        );
        let lax = ReqwestHttp::new(&tls_settings(&ca_path, false, None), None, None, None).unwrap();
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
        let without =
            ReqwestHttp::new(&tls_settings(&ca_path, true, None), None, None, None).unwrap();
        assert!(
            without.get_json(&url(port)).await.is_err(),
            "без клиентского сертификата сервер отказывает"
        );
        let with =
            ReqwestHttp::new(&tls_settings(&ca_path, true, Some(cc)), None, None, None).unwrap();
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
        ReqwestHttp::new(&s, None, None, None).unwrap();
        s.tls = Some(ApiTls {
            ca_path: None,
            verify_hostname: true,
            client_cert: None,
        });
        ReqwestHttp::new(&s, None, None, None).unwrap();
    }

    #[test]
    fn api_base_strips_patroni_only_from_path() {
        // Хост может называться `patroni`: сегмент отбрасывается только после адреса.
        assert_eq!(api_base("http://patroni"), "http://patroni");
        assert_eq!(api_base("https://patroni/"), "https://patroni");
        assert_eq!(api_base("http://patroni:8008"), "http://patroni:8008");
        assert_eq!(
            api_base("http://patroni:8008/patroni"),
            "http://patroni:8008"
        );
        assert_eq!(api_base("http://h:8008/patroni/"), "http://h:8008");
        assert_eq!(api_base(" http://h:8008/patroni "), "http://h:8008");
        assert_eq!(
            api_base("http://h:8008/prefix/patroni"),
            "http://h:8008/prefix"
        );
        assert_eq!(
            api_base("http://h:8008/mypatroni"),
            "http://h:8008/mypatroni"
        );
        assert_eq!(api_base("h:8008/patroni"), "h:8008");
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
