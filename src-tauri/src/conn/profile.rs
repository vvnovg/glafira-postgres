//! Профиль подключения (спец. P1 §3.1): вид, узлы, Patroni REST API, параметры PostgreSQL.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult, ErrorCode};
use crate::secrets::{
    PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, PG_KEY_PASSWORD, PG_PASSWORD, SSH_PASSPHRASE,
    SSH_PASSWORD,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SshAuth {
    Agent,
    Key,
    Password,
}

/// Туннель через системный OpenSSH (спец. P1 §3.1, §5). `target` — хост или алиас из `~/.ssh/config`;
/// `port` и `user` — `None` = как в конфиге ssh; `key_path` — только для `Key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshSettings {
    pub target: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub user: Option<String>,
    pub auth: SshAuth,
    #[serde(default)]
    pub key_path: Option<String>,
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
    /// Туннель SSH до кластера; `None` — подключение напрямую (профили P1a без поля).
    #[serde(default)]
    pub ssh: Option<SshSettings>,
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
            SSH_PASSWORD => self
                .ssh
                .as_ref()
                .is_some_and(|s| s.auth == SshAuth::Password),
            SSH_PASSPHRASE => self.ssh.as_ref().is_some_and(|s| s.auth == SshAuth::Key),
            _ => false,
        }
    }
}

/// Совпадают ли настройки, от которых зависит соединение с кластером: вид, адреса, целевой узел,
/// Patroni, PostgreSQL и туннель SSH. Имя, цвет, окружение, read-only, папка и порядок в дереве на
/// соединение не влияют: их правка не должна его переоткрывать. Поля перечислены явно — новое поле
/// профиля не скомпилируется, пока его не отнесут к одной из групп.
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
        ssh,
        folder_id: _,
        order: _,
        extra: _,
    } = a;
    *kind == b.kind
        && *hosts == b.hosts
        && *target_server_type == b.target_server_type
        && *patroni == b.patroni
        && *pg == b.pg
        && *ssh == b.ssh
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
    if let Some(s) = &mut p.ssh {
        s.target = s.target.trim().to_string();
        trim_opt(&mut s.user);
        // Путь ключа нужен только способу «ключ»: от других способов он не должен оставаться в файле.
        if s.auth == SshAuth::Key {
            trim_opt(&mut s.key_path);
        } else {
            s.key_path = None;
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

/// Сетевой путь или путь пространства имён устройств Windows: `\\server\share\a.pem`, `//server/share/a.pem`,
/// `\\?\…`, `\\.\…`, `\??\…` (разделители — любые). Такой файл не открывается никогда: обращение к
/// `\\server\…` — это исходящее SMB-соединение к адресу из профиля (утечка NTLM-хеша пользователя, зависание на
/// недоступном хосте), а профиль может прийти из чужого файла обмена. Правило одно на всех платформах; кому
/// сертификаты нужны с общего ресурса, подключает его буквой диска или точкой монтирования.
pub fn is_network_or_device_path(path: &str) -> bool {
    let sep = |c: char| c == '/' || c == '\\';
    let mut chars = path.trim_start().chars();
    match (chars.next(), chars.next()) {
        (Some(a), Some(b)) if sep(a) && sep(b) => true,
        (Some(a), Some('?')) if sep(a) => {
            matches!((chars.next(), chars.next()), (Some('?'), Some(c)) if sep(c))
        }
        _ => false,
    }
}

fn validate_path(path: &str) -> AppResult<()> {
    if is_network_or_device_path(path) {
        return Err(invalid(ltext!(
            "errors.connections.pathNetwork",
            "Certificate and key files must be on this computer, not on a network share or a device path: {{path}}",
            path = path
        )));
    }
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

/// Адрес без `user:password@`: пароль, вписанный в URL, нельзя хранить в профиле (секреты — только в
/// keychain), выносить в файл обмена и класть в архив диагностики. Правило намеренно грубое: концом userinfo
/// считается последний `@` после `://`, даже если до него стоят `/`, `?` или `#`. Пароль с неэкранированным
/// `/` (`https://admin:pa/ss@pg-1:8008`) по RFC читался бы как путь, и адрес с паролем остался бы в профиле.
/// Адресов REST API Patroni с `@` в пути или запросе не бывает, поэтому ложных срабатываний не ждём.
pub fn strip_url_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    match rest.rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => url.to_string(),
    }
}

/// Может ли в адресе быть userinfo (`user:password@`, `user@`): любой `@` после `://`.
pub fn has_url_credentials(url: &str) -> bool {
    strip_url_credentials(url) != url
}

/// Адреса Patroni REST API профиля: списки `urls` и `apiUrl` подмен адресов участников.
fn patroni_urls_mut(p: &mut ConnectionProfile) -> impl Iterator<Item = &mut String> {
    p.patroni.iter_mut().flat_map(|pt| {
        pt.urls.iter_mut().chain(
            pt.host_overrides
                .iter_mut()
                .filter_map(|o| o.api_url.as_mut()),
        )
    })
}

/// Вырезает userinfo из адресов Patroni API (профили с диска не проходят проверку, и такой адрес
/// мог остаться от старой версии). Возвращает `true`, если что-то вырезано: пароль из адреса
/// пропал, и получатель должен ввести его заново.
pub fn strip_patroni_url_credentials(p: &mut ConnectionProfile) -> bool {
    let mut stripped = false;
    for url in patroni_urls_mut(p).filter(|u| has_url_credentials(u)) {
        *url = strip_url_credentials(url);
        stripped = true;
    }
    stripped
}

fn validate_http_url(url: &str) -> AppResult<()> {
    let ok = (url.starts_with("http://") || url.starts_with("https://"))
        && url.split("://").nth(1).is_some_and(|rest| !rest.is_empty());
    if !ok {
        return Err(invalid(ltext!(
            "errors.connections.patroniUrl",
            "The Patroni API address must start with http:// or https://: {{url}}",
            url = url
        )));
    }
    // Адрес в ошибку не подставляется: в нём как раз пароль.
    if has_url_credentials(url) {
        return Err(invalid(ltext!(
            "errors.connections.patroniUrlCredentials",
            "The Patroni API address must not contain “@”, a user name or a password: use the “API user” and “API password” fields"
        )));
    }
    Ok(())
}

/// Id подключения становится частью имён файлов (`ssh-<id>.log`, отчёт askpass, файл секретов проверки) и ключей
/// keychain, а из файла обмена приходит любым: `..\x` на Windows вывел бы путь из каталога. Приложение выдаёт UUID
/// (`unsaved-<uuid>` у проверки), поэтому допустимы только ASCII-буквы, цифры, «_» и «-», не длиннее 64 символов.
fn connection_id_ok(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

pub fn validate_profile(p: &ConnectionProfile) -> AppResult<()> {
    if p.id.trim().is_empty() {
        return Err(invalid(ltext!(
            "errors.connections.noId",
            "The connection has no ID"
        )));
    }
    if !connection_id_ok(&p.id) {
        return Err(invalid(ltext!(
            "errors.connections.id",
            "Invalid connection ID: up to 64 Latin letters, digits, “_” and “-”"
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
    validate_tls(p.pg.tls.ca_path.as_ref(), p.pg.tls.client_cert.as_ref())?;
    validate_ssh(p.ssh.as_ref())
}

/// Цель и пользователь попадают в командную строку ssh как отдельные аргументы, а значения могут прийти из файла
/// обмена. Допускаются только ASCII-буквы, цифры, «.», «_» и «-», и не первым «-» (иначе аргумент стал бы опцией).
/// Это строже `valid_hostname`/`valid_ruser` OpenSSH 9.6: клиенты до 9.6 (в том числе штатные 8.1–9.5 в Windows)
/// подставляют `%h` и `%u` в `ProxyCommand` и `Match exec` через оболочку, и кавычки, `$`, `;`, `|`, `&`, `` ` ``,
/// скобки, `\`, `<`, `>`, пробелы или управляющие символы в цели или имени пользователя стали бы командой оболочки
/// (класс CVE-2023-51385). Для цели «@» не нужен: пользователь передаётся отдельно (`-l`), цель — хост или алиас.
fn ssh_argument_ok(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Имя пользователя: правило цели плюс «@» не первым символом. Бастионы в AD с SSSD и
/// `use_fully_qualified_names = True` пускают только под полным именем `ivanov@corp.example.com`. «@» не метасимвол
/// оболочки, а `valid_ruser` OpenSSH 9.6 его разрешает; пользователь уходит отдельным аргументом `-l`, поэтому
/// второй «@» не путает разбор цели.
fn ssh_user_ok(s: &str) -> bool {
    !s.starts_with('@')
        && !s.is_empty()
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'@'))
}

pub(crate) fn validate_ssh(ssh: Option<&SshSettings>) -> AppResult<()> {
    let Some(s) = ssh else { return Ok(()) };
    if !ssh_argument_ok(&s.target) {
        return Err(invalid(ltext!(
            "errors.connections.sshTarget",
            "SSH target is a host name or an alias from ~/.ssh/config, without a user name (enter the user in the “SSH user” field): Latin letters, digits, “.”, “_” and “-”, not starting with “-”"
        )));
    }
    if let Some(u) = &s.user
        && !ssh_user_ok(u)
    {
        return Err(invalid(ltext!(
            "errors.connections.sshUser",
            "SSH user name may contain only Latin letters, digits, “.”, “_”, “-” and “@” (for example name@domain), not starting with “-” or “@”"
        )));
    }
    if s.port == Some(0) {
        return Err(invalid(ltext!(
            "errors.connections.sshPort",
            "SSH port must be from 1 to 65535"
        )));
    }
    if s.auth == SshAuth::Key {
        match &s.key_path {
            None => {
                return Err(invalid(ltext!(
                    "errors.connections.sshKeyPath",
                    "Choose the private key file for the “key” method"
                )));
            }
            Some(path) => validate_path(path)?,
        }
    }
    Ok(())
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
        ssh: None,
        folder_id: None,
        order: 0,
        extra: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{
        PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, PG_KEY_PASSWORD, PG_PASSWORD, SSH_PASSPHRASE,
        SSH_PASSWORD,
    };

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
            ("errors.connections.noId", Box::new(|p| p.id = " ".into())),
            // Id — часть имён файлов: из файла обмена не должен прийти путь.
            ("errors.connections.id", Box::new(|p| p.id = "..\\x".into())),
            (
                "errors.connections.id",
                Box::new(|p| p.id = "../../etc/x".into()),
            ),
            ("errors.connections.id", Box::new(|p| p.id = "a b".into())),
            ("errors.connections.id", Box::new(|p| p.id = "ид".into())),
            ("errors.connections.id", Box::new(|p| p.id = "a".repeat(65))),
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
            // Пароль в адресе Patroni API: в профиль (connections.json, архив диагностики) он не попадает.
            (
                "errors.connections.patroniUrlCredentials",
                Box::new(|p| {
                    *p = patroni(p.clone());
                    p.patroni.as_mut().unwrap().urls =
                        vec!["https://admin:s3cr3t@pg-1:8008".into()];
                }),
            ),
            (
                "errors.connections.patroniUrlCredentials",
                Box::new(|p| {
                    *p = patroni(p.clone());
                    p.patroni.as_mut().unwrap().urls =
                        vec!["https://pg-1:8008".into(), "http://admin@pg-2:8008".into()];
                }),
            ),
            (
                "errors.connections.patroniUrlCredentials",
                Box::new(|p| {
                    *p = patroni(p.clone());
                    p.patroni.as_mut().unwrap().host_overrides = vec![HostOverride {
                        member: "pg-1".into(),
                        host: None,
                        port: None,
                        api_url: Some("https://admin:s3cr3t@10.0.0.1:8008".into()),
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
        // Id, которые выдаёт приложение: UUID и временный id проверки.
        validate_profile(&sample_profile("0b5f9c2e-6a1d-4f7e-9b3a-2c4d6e8f0a1b")).unwrap();
        validate_profile(&sample_profile(
            "unsaved-0b5f9c2e-6a1d-4f7e-9b3a-2c4d6e8f0a1b",
        ))
        .unwrap();
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
    fn network_and_device_paths_are_recognized() {
        for path in [
            r"\\server\share\ca.pem",
            "//server/share/ca.pem",
            r"\/server/share/ca.pem",
            r"/\server\share\ca.pem",
            "////server/share",
            r"\\?\C:\certs\ca.pem",
            r"\\?\UNC\server\share\ca.pem",
            r"\\.\pipe\x",
            r"\??\C:\certs\ca.pem",
            r"\??\UNC\server\share\ca.pem",
            "/??/C:/certs/ca.pem",
            "  //server/share/ca.pem",
        ] {
            assert!(is_network_or_device_path(path), "{path}");
        }
        for path in [
            "/certs/ca.pem",
            "/",
            "",
            "ca.pem",
            r"C:\certs\ca.pem",
            "C://certs/ca.pem",
            "/a//b/ca.pem",
            "/?/ca.pem",
            "/??x/ca.pem",
            r"relative\dir\ca.pem",
        ] {
            assert!(!is_network_or_device_path(path), "{path}");
        }
    }

    /// Все шесть путей к файлам сертификатов (PG и Patroni API: CA, сертификат, ключ): сохранение, проверка и
    /// импорт проходят через `validate_profile`, поэтому сетевой путь не попадает в профиль ни одним из них.
    #[test]
    fn network_paths_are_rejected_in_every_certificate_field() {
        let unc = r"\\attacker.invalid\x\ca.pem".to_string();
        let pem = |cert: String, key: String| ClientCert {
            format: CertFormat::Pem,
            certificate_path: cert,
            key_path: Some(key),
        };
        let api = |p: &mut ConnectionProfile, tls: ApiTls| {
            *p = patroni(p.clone());
            p.patroni.as_mut().unwrap().tls = Some(tls);
        };
        type Change<'a> = Box<dyn Fn(&mut ConnectionProfile) + 'a>;
        let cases: Vec<Change> = vec![
            Box::new(|p| p.pg.tls.ca_path = Some(unc.clone())),
            Box::new(|p| p.pg.tls.client_cert = Some(pem(unc.clone(), abs("k.pem")))),
            Box::new(|p| p.pg.tls.client_cert = Some(pem(abs("c.pem"), unc.clone()))),
            Box::new(|p| {
                api(
                    p,
                    ApiTls {
                        ca_path: Some("//attacker.invalid/x/ca.pem".into()),
                        verify_hostname: true,
                        client_cert: None,
                    },
                )
            }),
            Box::new(|p| {
                api(
                    p,
                    ApiTls {
                        ca_path: None,
                        verify_hostname: true,
                        client_cert: Some(pem(
                            r"\\?\UNC\attacker.invalid\x\c.pem".into(),
                            abs("k.pem"),
                        )),
                    },
                )
            }),
            Box::new(|p| {
                api(
                    p,
                    ApiTls {
                        ca_path: None,
                        verify_hostname: true,
                        client_cert: Some(pem(
                            abs("c.pem"),
                            r"\??\UNC\attacker.invalid\x\k.pem".into(),
                        )),
                    },
                )
            }),
        ];
        for (i, change) in cases.iter().enumerate() {
            let mut p = sample_profile("a");
            change(&mut p);
            let err = validate_profile(&p).unwrap_err();
            assert_eq!(
                err.key, "errors.connections.pathNetwork",
                "случай {i}: {p:?}"
            );
            assert_eq!(err.code, crate::error::ErrorCode::InvalidInput);
        }
    }

    #[test]
    fn credentials_error_does_not_echo_the_password() {
        let mut p = patroni(sample_profile("a"));
        p.patroni.as_mut().unwrap().urls = vec!["https://admin:s3cr3t@pg-1:8008".into()];
        let err = validate_profile(&p).unwrap_err();
        assert_eq!(err.key, "errors.connections.patroniUrlCredentials");
        assert!(
            !format!("{err:?}").contains("s3cr3t"),
            "пароль не должен попасть в текст ошибки: {err:?}"
        );
    }

    /// Любой `@` после `://` — возможный конец userinfo: пароль с неэкранированными `/`, `?`, `#` иначе
    /// прошёл бы проверку как «путь» и остался бы в профиле.
    #[test]
    fn any_at_sign_is_treated_as_userinfo() {
        for url in [
            "https://admin:pa/ss@pg-1:8008",
            "https://admin:pa?ss@pg-1:8008",
            "https://admin:pa#ss@pg-1:8008",
            "https://admin:12/34@pg-1:8008/patroni",
            "http://pg-2:8008/health?user=a@b",
            "http://[::1]:8008/x@y#z@w",
        ] {
            let mut p = patroni(sample_profile("a"));
            p.patroni.as_mut().unwrap().urls = vec!["https://pg-1:8008".into(), url.into()];
            let err = validate_profile(&p).unwrap_err();
            assert_eq!(err.key, "errors.connections.patroniUrlCredentials", "{url}");
            for fragment in ["pa/ss", "pa?ss", "pa#ss", "12/34"] {
                assert!(!format!("{err:?}").contains(fragment), "{url}: {err:?}");
            }
            let mut q = patroni(sample_profile("a"));
            q.patroni.as_mut().unwrap().host_overrides = vec![HostOverride {
                member: "pg-1".into(),
                host: None,
                port: None,
                api_url: Some(url.into()),
            }];
            assert_eq!(
                validate_profile(&q).unwrap_err().key,
                "errors.connections.patroniUrlCredentials",
                "{url}"
            );
        }
    }

    #[test]
    fn url_credentials_helpers() {
        for (url, clean) in [
            ("https://pg-1:8008", "https://pg-1:8008"),
            ("https://u:p@pg-1:8008", "https://pg-1:8008"),
            ("https://u@pg-1:8008/x", "https://pg-1:8008/x"),
            ("https://u:p@w@pg-1/x", "https://pg-1/x"),
            // Неэкранированные `/`, `?`, `#` в пароле: адрес всё равно очищается от него.
            ("https://u:pa/ss@pg-1:8008", "https://pg-1:8008"),
            ("https://u:p?ss@pg-1:8008/x", "https://pg-1:8008/x"),
            ("https://u:p#ss@pg-1", "https://pg-1"),
            // Грубое правило: `@` в пути или запросе тоже считается концом userinfo.
            ("http://pg-1/a@b", "http://b"),
            ("http://pg-1?x=a@b", "http://b"),
            ("no-scheme@host", "no-scheme@host"),
        ] {
            assert_eq!(strip_url_credentials(url), clean, "{url}");
            assert_eq!(has_url_credentials(url), url != clean, "{url}");
        }
    }

    #[test]
    fn stripping_patroni_url_credentials_reports_what_it_did() {
        let mut p = patroni(sample_profile("a"));
        let pt = p.patroni.as_mut().unwrap();
        pt.urls = vec![
            "https://admin:s3cr3t@pg-1:8008".into(),
            // Пароль с неэкранированным `/`: по RFC это путь, но адрес всё равно очищается.
            "http://admin:s3/cr3t@pg-2:8008".into(),
        ];
        pt.host_overrides = vec![HostOverride {
            member: "pg-1".into(),
            host: None,
            port: None,
            api_url: Some("https://admin:s3cr3t@10.0.0.1:8008".into()),
        }];
        assert!(strip_patroni_url_credentials(&mut p));
        let pt = p.patroni.as_ref().unwrap();
        assert_eq!(pt.urls, ["https://pg-1:8008", "http://pg-2:8008"]);
        assert_eq!(
            pt.host_overrides[0].api_url.as_deref(),
            Some("https://10.0.0.1:8008")
        );
        // Чистый профиль не меняется, и это видно по результату.
        assert!(!strip_patroni_url_credentials(&mut p));
        assert!(!strip_patroni_url_credentials(&mut sample_profile("s")));
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

    #[test]
    fn ssh_settings_round_trip_and_old_files_without_ssh() {
        let mut p = sample_profile("a");
        p.ssh = Some(SshSettings {
            target: "bastion-prod".into(),
            port: Some(2222),
            user: Some("tester".into()),
            auth: SshAuth::Key,
            key_path: Some("/home/me/.ssh/id_ed25519".into()),
        });
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["ssh"]["auth"], "key");
        assert_eq!(json["ssh"]["keyPath"], "/home/me/.ssh/id_ed25519");
        let back: ConnectionProfile = serde_json::from_value(json).unwrap();
        assert_eq!(back.ssh, p.ssh);
        // Файл P1a без поля ssh читается: подключение напрямую.
        let mut old = serde_json::to_value(sample_profile("b")).unwrap();
        old.as_object_mut().unwrap().remove("ssh");
        assert_eq!(
            serde_json::from_value::<ConnectionProfile>(old)
                .unwrap()
                .ssh,
            None
        );
    }

    #[test]
    fn ssh_secrets_follow_the_auth_method() {
        let mut p = sample_profile("a");
        assert!(!p.uses_secret(SSH_PASSWORD) && !p.uses_secret(SSH_PASSPHRASE));
        p.ssh = Some(SshSettings {
            target: "b".into(),
            port: None,
            user: None,
            auth: SshAuth::Password,
            key_path: None,
        });
        assert!(p.uses_secret(SSH_PASSWORD) && !p.uses_secret(SSH_PASSPHRASE));
        p.ssh.as_mut().unwrap().auth = SshAuth::Key;
        assert!(!p.uses_secret(SSH_PASSWORD) && p.uses_secret(SSH_PASSPHRASE));
        p.ssh.as_mut().unwrap().auth = SshAuth::Agent;
        assert!(!p.uses_secret(SSH_PASSWORD) && !p.uses_secret(SSH_PASSPHRASE));
    }

    #[test]
    fn ssh_changes_the_connection_settings() {
        let a = sample_profile("a");
        let mut b = a.clone();
        b.ssh = Some(SshSettings {
            target: "b".into(),
            port: None,
            user: None,
            auth: SshAuth::Agent,
            key_path: None,
        });
        assert!(!same_connection_settings(&a, &b));
    }

    #[test]
    fn ssh_validation() {
        let ssh = |target: &str, user: Option<&str>, auth: SshAuth, key: Option<&str>| {
            let mut p = sample_profile("a");
            p.ssh = Some(SshSettings {
                target: target.into(),
                port: None,
                user: user.map(String::from),
                auth,
                key_path: key.map(String::from),
            });
            validate_profile(&p).map_err(|e| e.key)
        };
        assert_eq!(
            ssh("", None, SshAuth::Agent, None),
            Err("errors.connections.sshTarget".into())
        );
        // Аргументы ssh: ни пробелов, ни ведущего «-» — иначе цель или пользователь стали бы опцией ssh.
        assert_eq!(
            ssh("-oProxyCommand=x", None, SshAuth::Agent, None),
            Err("errors.connections.sshTarget".into())
        );
        assert_eq!(
            ssh("bastion x", None, SshAuth::Agent, None),
            Err("errors.connections.sshTarget".into())
        );
        assert_eq!(
            ssh("bastion", Some("-v"), SshAuth::Agent, None),
            Err("errors.connections.sshUser".into())
        );
        assert_eq!(
            ssh("bastion", None, SshAuth::Key, None),
            Err("errors.connections.sshKeyPath".into())
        );
        assert_eq!(
            ssh("bastion", None, SshAuth::Key, Some("relative/key")),
            Err("errors.connections.pathNotAbsolute".into())
        );
        // Файл ключа из файла обмена не должен вести на сетевой ресурс.
        assert_eq!(
            ssh("bastion", None, SshAuth::Key, Some("//server/share/id")),
            Err("errors.connections.pathNetwork".into())
        );
        // `abs`: на Windows путь с одним «/» не абсолютный.
        let key = abs("k/id_ed25519");
        assert!(ssh("bastion", Some("tester"), SshAuth::Key, Some(&key)).is_ok());
        // Допустимые цели: хост, полное имя, IPv4, алиас из ~/.ssh/config с «-» и «_».
        for target in [
            "bastion",
            "bastion.corp.example",
            "10.0.0.5",
            "jump-host_prod-01",
        ] {
            assert!(
                ssh(target, Some("svc_user.ops-1"), SshAuth::Password, None).is_ok(),
                "{target}"
            );
        }
        // Метасимволы оболочки, кавычки, управляющие символы и не-ASCII — и в цели, и в пользователе: клиенты
        // OpenSSH до 9.6 подставляют `%h` и `%u` в ProxyCommand и Match exec через оболочку (класс CVE-2023-51385).
        for bad in [
            "host;id",
            "host`id`",
            "host$(id)",
            "host${IFS}x",
            "host\nx",
            "host\tx",
            "host\u{7f}x",
            "host|id",
            "host&id",
            "host>x",
            "host<x",
            "host'x",
            "host\"x",
            "host\\x",
            "host(x)",
            "host{x}",
            "хост",
            "-oProxyCommand=sh -c id",
        ] {
            assert_eq!(
                ssh(bad, None, SshAuth::Agent, None),
                Err("errors.connections.sshTarget".into()),
                "{bad:?}"
            );
            assert_eq!(
                ssh("bastion", Some(bad), SshAuth::Agent, None),
                Err("errors.connections.sshUser".into()),
                "{bad:?}"
            );
        }
        // «@» допустим только в имени пользователя и не первым: бастион в AD с SSSD (`use_fully_qualified_names`)
        // ждёт `ivanov@corp.example.com`; в цели «@» — это привычка `ssh user@bastion`, пользователь идёт в своё поле.
        for user in ["ivanov@corp.example.com", "svc_pg-ro", "a@b", "name@domain"] {
            assert!(
                ssh("bastion", Some(user), SshAuth::Agent, None).is_ok(),
                "{user}"
            );
        }
        for bad in [
            "@user",
            "@",
            "user;x",
            "user x",
            "-user",
            "-@user",
            "user@host;id",
            "user@host x",
            "user@хост",
            "user@host\n",
        ] {
            assert_eq!(
                ssh("bastion", Some(bad), SshAuth::Agent, None),
                Err("errors.connections.sshUser".into()),
                "{bad:?}"
            );
        }
        for bad in ["user@host", "user@host-alias.example", "@host", "a@b"] {
            assert_eq!(
                ssh(bad, None, SshAuth::Agent, None),
                Err("errors.connections.sshTarget".into()),
                "{bad:?}"
            );
        }
        // Порт 0 не бывает; пустой «порт по конфигу ssh» (`None`) допустим.
        let mut zero = sample_profile("a");
        zero.ssh = Some(SshSettings {
            target: "bastion".into(),
            port: Some(0),
            user: None,
            auth: SshAuth::Agent,
            key_path: None,
        });
        assert_eq!(
            validate_profile(&zero).map_err(|e| e.key),
            Err("errors.connections.sshPort".into())
        );
        // Ключ без способа «ключ» не хранится.
        let mut p = sample_profile("a");
        p.ssh = Some(SshSettings {
            target: "b".into(),
            port: Some(22),
            user: None,
            auth: SshAuth::Agent,
            key_path: Some("/k".into()),
        });
        normalize_profile(&mut p);
        assert_eq!(p.ssh.as_ref().unwrap().key_path, None);
    }
}
