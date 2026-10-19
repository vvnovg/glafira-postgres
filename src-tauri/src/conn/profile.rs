//! Профиль подключения (спец. P1 §3.1): вид, узлы, Patroni REST API, параметры PostgreSQL.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult, ErrorCode};
use crate::secrets::{PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, PG_KEY_PASSWORD, PG_PASSWORD};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Local,
    Dev,
    Test,
    Stage,
    Prod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionKind {
    Server,
    HostList,
    Patroni,
}

/// Узел по умолчанию для списка узлов — как `targetServerType` JDBC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetServerType {
    #[default]
    Primary,
    Standby,
    PreferStandby,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPort {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CertFormat {
    #[default]
    Pem,
    Pkcs12,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCert {
    pub format: CertFormat,
    pub certificate_path: String,
    /// Только для PEM.
    #[serde(default)]
    pub key_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PgTls {
    /// PEM-файл CA; пусто — системные корневые сертификаты.
    #[serde(default)]
    pub ca_path: Option<String>,
    #[serde(default)]
    pub client_cert: Option<ClientCert>,
}

fn default_database() -> String {
    "postgres".into()
}

fn default_connect_timeout() -> u64 {
    10
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PgSettings {
    pub username: String,
    #[serde(default = "default_database")]
    pub database: String,
    #[serde(default)]
    pub ssl_mode: SslMode,
    #[serde(default)]
    pub tls: PgTls,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_s: u64,
    /// `None` — все БД с `datallowconn`; список — только эти.
    #[serde(default)]
    pub databases: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiTls {
    #[serde(default)]
    pub ca_path: Option<String>,
    #[serde(default = "default_true")]
    pub verify_hostname: bool,
    #[serde(default)]
    pub client_cert: Option<ClientCert>,
}

/// Подмена адресов участника Patroni (pod IP, NAT): PG и/или REST API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostOverride {
    pub member: String,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub api_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatroniSettings {
    pub urls: Vec<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub tls: Option<ApiTls>,
    #[serde(default)]
    pub host_overrides: Vec<HostOverride>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub color: String,
    pub environment: Environment,
    pub read_only: bool,
    pub kind: ConnectionKind,
    #[serde(default)]
    pub hosts: Vec<HostPort>,
    #[serde(default)]
    pub target_server_type: TargetServerType,
    #[serde(default)]
    pub patroni: Option<PatroniSettings>,
    pub pg: PgSettings,
    #[serde(default)]
    pub folder_id: Option<String>,
    #[serde(default)]
    pub order: i64,
    /// Поля из более новой версии Glafira: сохраняются при записи как есть.
    /// `ConnectionStore::upsert` входящий `extra` не принимает.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ConnectionProfile {
    /// Нужен ли профилю секрет `field`; ненужные секреты при сохранении удаляются.
    pub fn uses_secret(&self, field: &str) -> bool {
        let api_tls = self.patroni.as_ref().and_then(|p| p.tls.as_ref());
        match field {
            PG_PASSWORD => true,
            PG_KEY_PASSWORD => self.pg.tls.client_cert.is_some(),
            PATRONI_PASSWORD => self.patroni.as_ref().is_some_and(|p| p.username.is_some()),
            PATRONI_KEY_PASSWORD => api_tls.is_some_and(|t| t.client_cert.is_some()),
            _ => false,
        }
    }
}

/// Совпадают ли настройки, от которых зависит соединение с кластером: вид, адреса, целевой узел,
/// Patroni и PostgreSQL. Имя, цвет, окружение, read-only, папка и порядок в дереве на соединение не
/// влияют: их правка не должна его переоткрывать. Поля перечислены явно — новое поле профиля не
/// скомпилируется, пока его не отнесут к одной из групп.
pub fn same_connection_settings(a: &ConnectionProfile, b: &ConnectionProfile) -> bool {
    let ConnectionProfile {
        id: _,
        name: _,
        color: _,
        environment: _,
        read_only: _,
        kind,
        hosts,
        target_server_type,
        patroni,
        pg,
        folder_id: _,
        order: _,
        extra: _,
    } = a;
    *kind == b.kind
        && *hosts == b.hosts
        && *target_server_type == b.target_server_type
        && *patroni == b.patroni
        && *pg == b.pg
}

fn trim_opt(v: &mut Option<String>) {
    *v = v
        .take()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
}

fn normalize_cert(c: &mut Option<ClientCert>) {
    if let Some(cert) = c {
        cert.certificate_path = cert.certificate_path.trim().to_string();
        trim_opt(&mut cert.key_path);
        if cert.format == CertFormat::Pkcs12 {
            cert.key_path = None;
        }
    }
}

/// Приводит профиль из формы к хранимому виду: обрезает пробелы, убирает поля чужого вида.
pub fn normalize_profile(p: &mut ConnectionProfile) {
    p.name = p.name.trim().to_string();
    for h in &mut p.hosts {
        h.host = h.host.trim().to_string();
    }
    match p.kind {
        ConnectionKind::Patroni => p.hosts.clear(),
        ConnectionKind::Server | ConnectionKind::HostList => p.patroni = None,
    }
    if let Some(pt) = &mut p.patroni {
        pt.urls = pt
            .urls
            .iter()
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty())
            .collect();
        trim_opt(&mut pt.username);
        if let Some(tls) = &mut pt.tls {
            trim_opt(&mut tls.ca_path);
            normalize_cert(&mut tls.client_cert);
        }
        for o in &mut pt.host_overrides {
            o.member = o.member.trim().to_string();
            trim_opt(&mut o.host);
            trim_opt(&mut o.api_url);
            if let Some(u) = &mut o.api_url {
                *u = u.trim_end_matches('/').to_string();
            }
        }
    }
    p.pg.username = p.pg.username.trim().to_string();
    p.pg.database = p.pg.database.trim().to_string();
    trim_opt(&mut p.pg.tls.ca_path);
    normalize_cert(&mut p.pg.tls.client_cert);
    if let Some(list) = &mut p.pg.databases {
        *list = list
            .iter()
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty())
            .collect();
        if list.is_empty() {
            p.pg.databases = None;
        }
    }
}

fn invalid(text: crate::i18n::LocalizedText) -> AppError {
    AppError::localized(ErrorCode::InvalidInput, text)
}

fn validate_path(path: &str) -> AppResult<()> {
    if Path::new(path).is_absolute() {
        Ok(())
    } else {
        Err(invalid(ltext!(
            "errors.connections.pathNotAbsolute",
            "The file path must be absolute: {{path}}",
            path = path
        )))
    }
}

fn validate_tls(ca: Option<&String>, cert: Option<&ClientCert>) -> AppResult<()> {
    if let Some(ca) = ca {
        validate_path(ca)?;
    }
    if let Some(c) = cert {
        validate_path(&c.certificate_path)?;
        match (c.format, &c.key_path) {
            (CertFormat::Pem, None) => {
                return Err(invalid(ltext!(
                    "errors.connections.keyRequired",
                    "Specify the client key file for a PEM certificate"
                )));
            }
            (CertFormat::Pem, Some(k)) => validate_path(k)?,
            (CertFormat::Pkcs12, _) => {}
        }
    }
    Ok(())
}

fn validate_http_url(url: &str) -> AppResult<()> {
    let ok = (url.starts_with("http://") || url.starts_with("https://"))
        && url.split("://").nth(1).is_some_and(|rest| !rest.is_empty());
    if ok {
        Ok(())
    } else {
        Err(invalid(ltext!(
            "errors.connections.patroniUrl",
            "The Patroni API address must start with http:// or https://: {{url}}",
            url = url
        )))
    }
}

pub fn validate_profile(p: &ConnectionProfile) -> AppResult<()> {
    if p.id.trim().is_empty() {
        return Err(invalid(ltext!(
            "errors.connections.noId",
            "The connection has no ID"
        )));
    }
    if p.name.trim().is_empty() {
        return Err(invalid(ltext!(
            "errors.connections.emptyName",
            "Connection name cannot be empty"
        )));
    }
    match p.kind {
        ConnectionKind::Server if p.hosts.len() != 1 => {
            return Err(invalid(ltext!(
                "errors.connections.serverHosts",
                "A server connection needs exactly one host"
            )));
        }
        ConnectionKind::HostList if !(1..=16).contains(&p.hosts.len()) => {
            return Err(invalid(ltext!(
                "errors.connections.hostListCount",
                "Specify from 1 to 16 hosts"
            )));
        }
        ConnectionKind::Patroni => validate_patroni(p.patroni.as_ref())?,
        _ => {}
    }
    if p.kind != ConnectionKind::Patroni {
        for h in &p.hosts {
            if h.host.is_empty() || h.host.chars().any(char::is_whitespace) {
                return Err(invalid(ltext!(
                    "errors.connections.hostInvalid",
                    "Invalid host name: “{{host}}”",
                    host = h.host
                )));
            }
            if h.port == 0 {
                return Err(invalid(ltext!(
                    "errors.connections.portInvalid",
                    "Port must be from 1 to 65535"
                )));
            }
        }
    }
    if p.pg.username.trim().is_empty() {
        return Err(invalid(ltext!(
            "errors.connections.pgUserRequired",
            "Specify the PostgreSQL user"
        )));
    }
    if p.pg.database.trim().is_empty() {
        return Err(invalid(ltext!(
            "errors.connections.pgDatabaseRequired",
            "Specify the default database"
        )));
    }
    if !(1..=120).contains(&p.pg.connect_timeout_s) {
        return Err(invalid(ltext!(
            "errors.connections.timeoutRange",
            "Connection timeout must be from 1 to 120 seconds"
        )));
    }
    validate_tls(p.pg.tls.ca_path.as_ref(), p.pg.tls.client_cert.as_ref())
}

fn validate_patroni(pt: Option<&PatroniSettings>) -> AppResult<()> {
    let urls_err = || {
        invalid(ltext!(
            "errors.connections.patroniUrls",
            "Specify from 1 to 8 Patroni REST API addresses"
        ))
    };
    let pt = pt.ok_or_else(urls_err)?;
    if !(1..=8).contains(&pt.urls.len()) {
        return Err(urls_err());
    }
    for u in &pt.urls {
        validate_http_url(u)?;
    }
    if let Some(tls) = &pt.tls {
        validate_tls(tls.ca_path.as_ref(), tls.client_cert.as_ref())?;
    }
    let mut seen = HashSet::new();
    for o in &pt.host_overrides {
        if o.member.trim().is_empty() {
            return Err(invalid(ltext!(
                "errors.connections.overrideMember",
                "Specify the member name for the address override"
            )));
        }
        if !seen.insert(o.member.as_str()) {
            return Err(invalid(ltext!(
                "errors.connections.overrideDuplicate",
                "Address override for {{member}} is specified twice",
                member = o.member
            )));
        }
        if o.port == Some(0) {
            return Err(invalid(ltext!(
                "errors.connections.portInvalid",
                "Port must be from 1 to 65535"
            )));
        }
        if let Some(u) = &o.api_url {
            validate_http_url(u)?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub fn sample_profile(id: &str) -> ConnectionProfile {
    ConnectionProfile {
        id: id.to_string(),
        name: format!("conn-{id}"),
        color: "#4f8cff".into(),
        environment: Environment::Dev,
        read_only: false,
        kind: ConnectionKind::Server,
        hosts: vec![HostPort {
            host: "localhost".into(),
            port: 5432,
        }],
        target_server_type: TargetServerType::Primary,
        patroni: None,
        pg: PgSettings {
            username: "glafira_diag".into(),
            database: "postgres".into(),
            ssl_mode: SslMode::Prefer,
            tls: PgTls::default(),
            connect_timeout_s: 10,
            databases: None,
        },
        folder_id: None,
        order: 0,
        extra: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, PG_KEY_PASSWORD, PG_PASSWORD};

    /// Название случая (ключ ожидаемой ошибки) и правка профиля.
    type Case = (&'static str, Box<dyn Fn(&mut ConnectionProfile)>);

    fn abs(p: &str) -> String {
        if cfg!(windows) {
            format!("C:\\{p}")
        } else {
            format!("/{p}")
        }
    }

    fn patroni(mut p: ConnectionProfile) -> ConnectionProfile {
        p.kind = ConnectionKind::Patroni;
        p.patroni = Some(PatroniSettings {
            urls: vec!["https://pg-1:8008".into()],
            username: Some("patroni".into()),
            tls: None,
            host_overrides: vec![],
        });
        p
    }

    #[test]
    fn json_shape_matches_frontend_contract() {
        let mut p = sample_profile("a");
        p.kind = ConnectionKind::HostList;
        p.target_server_type = TargetServerType::PreferStandby;
        p.pg.ssl_mode = SslMode::VerifyFull;
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["kind"], "hostList");
        assert_eq!(v["targetServerType"], "preferStandby");
        assert_eq!(v["pg"]["sslMode"], "verify-full");
        assert_eq!(v["pg"]["connectTimeoutS"], 10);
        assert_eq!(v["hosts"][0]["host"], "localhost");
        assert!(v.get("password").is_none());
    }

    #[test]
    fn missing_fields_get_defaults() {
        let p: ConnectionProfile = serde_json::from_value(serde_json::json!({
            "id": "a", "name": "A", "color": "#fff", "environment": "dev", "readOnly": false,
            "kind": "server", "hosts": [{"host": "h", "port": 5432}],
            "pg": {"username": "u"}
        }))
        .unwrap();
        assert_eq!(p.pg.database, "postgres");
        assert_eq!(p.pg.ssl_mode, SslMode::Prefer);
        assert_eq!(p.pg.connect_timeout_s, 10);
        assert_eq!(p.target_server_type, TargetServerType::Primary);
        assert!(p.patroni.is_none());
    }

    #[test]
    fn spec_example_parses() {
        let p: ConnectionProfile = serde_json::from_value(serde_json::json!({
            "id": "6f1c", "name": "billing-prod", "color": "#e11d48", "environment": "prod",
            "readOnly": true, "kind": "patroni", "hosts": [], "targetServerType": "primary",
            "patroni": {
                "urls": ["https://pg-1:8008", "https://pg-2:8008"], "username": "patroni",
                "tls": {"caPath": "/certs/ca.crt", "verifyHostname": true, "clientCert": null},
                "hostOverrides": [{"member": "pg-1", "host": "10.1.2.3", "port": 5432, "apiUrl": "https://10.1.2.3:8008"}]
            },
            "pg": {
                "username": "glafira_diag", "database": "postgres", "sslMode": "verify-full",
                "tls": {"caPath": "/certs/ca.crt", "clientCert": {"format": "pem", "certificatePath": "/c", "keyPath": "/k"}},
                "connectTimeoutS": 10, "databases": null
            },
            "folderId": null, "order": 0
        }))
        .unwrap();
        assert_eq!(p.kind, ConnectionKind::Patroni);
        assert_eq!(
            p.patroni.as_ref().unwrap().host_overrides[0].port,
            Some(5432)
        );
        assert_eq!(
            p.pg.tls.client_cert.as_ref().unwrap().format,
            CertFormat::Pem
        );
    }

    #[test]
    fn normalize_keeps_only_fields_of_the_kind() {
        let mut p = patroni(sample_profile("a"));
        normalize_profile(&mut p);
        assert!(p.hosts.is_empty(), "у Patroni узлы приходят из /cluster");

        let mut s = sample_profile("b");
        s.patroni = patroni(sample_profile("x")).patroni;
        s.name = "  b  ".into();
        s.hosts[0].host = " db1 ".into();
        s.pg.databases = Some(vec![" orders ".into(), "".into()]);
        s.pg.tls.ca_path = Some("".into());
        normalize_profile(&mut s);
        assert!(s.patroni.is_none());
        assert_eq!(s.name, "b");
        assert_eq!(s.hosts[0].host, "db1");
        assert_eq!(s.pg.databases, Some(vec!["orders".to_string()]));
        assert_eq!(s.pg.tls.ca_path, None);

        let mut e = sample_profile("c");
        e.pg.databases = Some(vec![" ".into()]);
        normalize_profile(&mut e);
        assert_eq!(e.pg.databases, None, "пустой фильтр — все базы");

        let mut u = patroni(sample_profile("d"));
        u.patroni.as_mut().unwrap().urls = vec![" https://pg-1:8008/ ".into(), "".into()];
        u.patroni.as_mut().unwrap().username = Some("".into());
        normalize_profile(&mut u);
        assert_eq!(
            u.patroni.as_ref().unwrap().urls,
            vec!["https://pg-1:8008".to_string()]
        );
        assert_eq!(u.patroni.as_ref().unwrap().username, None);
    }

    #[test]
    fn validation_rules() {
        let cases: Vec<Case> = vec![
            (
                "errors.connections.emptyName",
                Box::new(|p| p.name = " ".into()),
            ),
            (
                "errors.connections.serverHosts",
                Box::new(|p| {
                    p.hosts.push(HostPort {
                        host: "b".into(),
                        port: 1,
                    })
                }),
            ),
            (
                "errors.connections.hostListCount",
                Box::new(|p| {
                    p.kind = ConnectionKind::HostList;
                    p.hosts.clear();
                }),
            ),
            (
                "errors.connections.hostListCount",
                Box::new(|p| {
                    p.kind = ConnectionKind::HostList;
                    p.hosts = (0..17)
                        .map(|i| HostPort {
                            host: format!("h{i}"),
                            port: 5432,
                        })
                        .collect();
                }),
            ),
            (
                "errors.connections.hostInvalid",
                Box::new(|p| p.hosts[0].host = "a b".into()),
            ),
            (
                "errors.connections.portInvalid",
                Box::new(|p| p.hosts[0].port = 0),
            ),
            (
                "errors.connections.pgUserRequired",
                Box::new(|p| p.pg.username = "".into()),
            ),
            (
                "errors.connections.pgDatabaseRequired",
                Box::new(|p| p.pg.database = "".into()),
            ),
            (
                "errors.connections.timeoutRange",
                Box::new(|p| p.pg.connect_timeout_s = 0),
            ),
            (
                "errors.connections.timeoutRange",
                Box::new(|p| p.pg.connect_timeout_s = 121),
            ),
            (
                "errors.connections.pathNotAbsolute",
                Box::new(|p| p.pg.tls.ca_path = Some("ca.pem".into())),
            ),
            (
                "errors.connections.keyRequired",
                Box::new(|p| {
                    p.pg.tls.client_cert = Some(ClientCert {
                        format: CertFormat::Pem,
                        certificate_path: abs("c.pem"),
                        key_path: None,
                    })
                }),
            ),
            (
                "errors.connections.patroniUrls",
                Box::new(|p| {
                    *p = patroni(p.clone());
                    p.patroni.as_mut().unwrap().urls.clear();
                }),
            ),
            (
                "errors.connections.patroniUrl",
                Box::new(|p| {
                    *p = patroni(p.clone());
                    p.patroni.as_mut().unwrap().urls = vec!["pg-1:8008".into()];
                }),
            ),
            (
                "errors.connections.overrideMember",
                Box::new(|p| {
                    *p = patroni(p.clone());
                    p.patroni.as_mut().unwrap().host_overrides = vec![HostOverride {
                        member: "".into(),
                        host: Some("h".into()),
                        port: None,
                        api_url: None,
                    }];
                }),
            ),
            (
                "errors.connections.overrideDuplicate",
                Box::new(|p| {
                    *p = patroni(p.clone());
                    let o = HostOverride {
                        member: "pg-1".into(),
                        host: Some("h".into()),
                        port: None,
                        api_url: None,
                    };
                    p.patroni.as_mut().unwrap().host_overrides = vec![o.clone(), o];
                }),
            ),
            (
                "errors.connections.patroniUrl",
                Box::new(|p| {
                    *p = patroni(p.clone());
                    p.patroni.as_mut().unwrap().host_overrides = vec![HostOverride {
                        member: "pg-1".into(),
                        host: None,
                        port: None,
                        api_url: Some("pg-1".into()),
                    }];
                }),
            ),
        ];
        for (key, change) in cases {
            let mut p = sample_profile("a");
            change(&mut p);
            let err = validate_profile(&p).unwrap_err();
            assert_eq!(err.key, key, "{p:?}");
            assert_eq!(err.code, crate::error::ErrorCode::InvalidInput);
        }
        validate_profile(&sample_profile("a")).unwrap();
        validate_profile(&patroni(sample_profile("p")))
            .map(|_| ())
            .unwrap_or_else(|e| {
                // У Patroni список hosts игнорируется после normalize, но и без него профиль валиден.
                panic!("{e:?}")
            });
        let mut pkcs = sample_profile("k");
        pkcs.pg.tls.client_cert = Some(ClientCert {
            format: CertFormat::Pkcs12,
            certificate_path: abs("c.p12"),
            key_path: None,
        });
        validate_profile(&pkcs).unwrap();
    }

    #[test]
    fn used_secrets_depend_on_profile() {
        let p = sample_profile("a");
        assert!(p.uses_secret(PG_PASSWORD));
        assert!(!p.uses_secret(PG_KEY_PASSWORD));
        assert!(!p.uses_secret(PATRONI_PASSWORD));
        let mut q = patroni(sample_profile("b"));
        q.pg.tls.client_cert = Some(ClientCert {
            format: CertFormat::Pkcs12,
            certificate_path: abs("c.p12"),
            key_path: None,
        });
        q.patroni.as_mut().unwrap().tls = Some(ApiTls {
            ca_path: None,
            verify_hostname: true,
            client_cert: Some(ClientCert {
                format: CertFormat::Pkcs12,
                certificate_path: abs("a.p12"),
                key_path: None,
            }),
        });
        assert!(q.uses_secret(PG_KEY_PASSWORD));
        assert!(q.uses_secret(PATRONI_PASSWORD));
        assert!(q.uses_secret(PATRONI_KEY_PASSWORD));
        q.patroni.as_mut().unwrap().username = None;
        assert!(!q.uses_secret(PATRONI_PASSWORD));
    }

    #[test]
    fn cosmetic_edits_keep_the_connection_settings() {
        let base = patroni(sample_profile("a"));
        let mut cosmetic = base.clone();
        cosmetic.id = "other-id-is-not-compared".into();
        cosmetic.name = "renamed".into();
        cosmetic.color = "#000000".into();
        cosmetic.environment = Environment::Stage;
        cosmetic.read_only = !base.read_only;
        cosmetic.folder_id = Some("folder".into());
        cosmetic.order = 7;
        cosmetic
            .extra
            .insert("future".into(), serde_json::json!(true));
        assert!(same_connection_settings(&base, &cosmetic));
        assert!(same_connection_settings(&cosmetic, &base));
    }

    #[test]
    fn connection_settings_changes_are_detected() {
        let base = patroni(sample_profile("a"));
        let edits: Vec<Case> = vec![
            ("kind", Box::new(|p| p.kind = ConnectionKind::HostList)),
            (
                "hosts",
                Box::new(|p| {
                    p.hosts.push(HostPort {
                        host: "h".into(),
                        port: 1,
                    })
                }),
            ),
            (
                "targetServerType",
                Box::new(|p| p.target_server_type = TargetServerType::Standby),
            ),
            (
                "patroni urls",
                Box::new(|p| {
                    p.patroni
                        .as_mut()
                        .unwrap()
                        .urls
                        .push("http://x:8008".into())
                }),
            ),
            ("patroni", Box::new(|p| p.patroni = None)),
            ("pg user", Box::new(|p| p.pg.username = "another".into())),
            ("pg database", Box::new(|p| p.pg.database = "orders".into())),
            ("pg ssl", Box::new(|p| p.pg.ssl_mode = SslMode::VerifyFull)),
            ("pg timeout", Box::new(|p| p.pg.connect_timeout_s = 3)),
        ];
        for (what, edit) in edits {
            let mut changed = base.clone();
            edit(&mut changed);
            assert!(!same_connection_settings(&base, &changed), "{what}");
            assert!(!same_connection_settings(&changed, &base), "{what}");
        }
    }
}
