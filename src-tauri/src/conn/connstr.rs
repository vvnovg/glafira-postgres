//! Строка подключения → поля формы (спец. P1 §4): JDBC, URI `postgresql://`, conninfo `key=value`.
//! Пароль из строки возвращается отдельно (форма отправит его в keychain), нераспознанные параметры —
//! только именами: значения могут содержать секреты. По той же причине ошибки разбора не повторяют
//! кусков строки: в них только имена параметров, номер узла или позиция (неверно заключённый в кавычки
//! или неэкранированный пароль сдвигает разбор, и его часть оказалась бы «портом» или «рядом с …»).

use serde::Serialize;

use crate::conn::profile::{
    CertFormat, ClientCert, ConnectionKind, HostPort, SslMode, TargetServerType,
};
use crate::error::{AppError, AppResult, ErrorCode};

const DEFAULT_PORT: u16 = 5432;
const MAX_HOSTS: usize = 16;
const MAX_TIMEOUT_S: u64 = 120;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePatch {
    pub kind: ConnectionKind,
    pub hosts: Vec<HostPort>,
    pub target_server_type: Option<TargetServerType>,
    pub username: Option<String>,
    pub database: Option<String>,
    pub ssl_mode: Option<SslMode>,
    pub ca_path: Option<String>,
    pub client_cert: Option<ClientCert>,
    pub connect_timeout_s: Option<u64>,
}

#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedConnection {
    pub patch: ProfilePatch,
    pub password: Option<String>,
    pub ignored: Vec<String>,
}

// Пароль в отладочный вывод (журнал, сообщения тестов) не попадает.
impl std::fmt::Debug for ParsedConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParsedConnection")
            .field("patch", &self.patch)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("ignored", &self.ignored)
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Dialect {
    Libpq,
    Jdbc,
}

/// Строка до сопоставления с полями: хосты с необязательными портами и параметры по порядку.
struct Raw {
    hosts: Vec<(String, Option<u16>)>,
    user: Option<String>,
    password: Option<String>,
    database: Option<String>,
    params: Vec<(String, String)>,
}

fn invalid(text: crate::i18n::LocalizedText) -> AppError {
    AppError::localized(ErrorCode::InvalidInput, text)
}

fn unix_socket() -> AppError {
    invalid(ltext!(
        "errors.connstr.unixSocket",
        "Unix sockets are not supported"
    ))
}

/// Ошибка синтаксиса conninfo; `position` — номер символа (с 1).
fn syntax(position: usize) -> AppError {
    invalid(ltext!(
        "errors.connstr.syntax",
        "Connection string syntax error at position {{position}}",
        position = position
    ))
}

/// Узел номер `n` (с 1) в списке хостов URI записан неверно.
fn bad_host(n: usize) -> AppError {
    invalid(ltext!(
        "errors.connstr.badHost",
        "Host {{n}} in the connection string is written incorrectly",
        n = n
    ))
}

pub fn parse_connection_string(text: &str) -> AppResult<ParsedConnection> {
    let t = text.trim();
    if let Some(rest) = strip_prefix_ci(t, "jdbc:postgresql://") {
        return map(parse_uri_body(rest)?, Dialect::Jdbc);
    }
    for scheme in ["postgresql://", "postgres://"] {
        if let Some(rest) = strip_prefix_ci(t, scheme) {
            return map(parse_uri_body(rest)?, Dialect::Libpq);
        }
    }
    // Пробел без «=» — тоже conninfo с ошибкой (`host h`), а не нераспознанный текст (`hello`).
    if t.contains('=') || t.chars().any(char::is_whitespace) {
        return map(parse_conninfo(t)?, Dialect::Libpq);
    }
    Err(invalid(ltext!(
        "errors.connstr.unrecognized",
        "Unrecognized connection string: expected jdbc:postgresql://, postgresql:// or key=value pairs"
    )))
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &s[prefix.len()..])
}

fn decode(s: &str) -> AppResult<String> {
    percent_encoding::percent_decode_str(s)
        .decode_utf8()
        .map(|c| c.into_owned())
        .map_err(|_| bad_encoding())
        .and_then(|v| {
            // `%zz` percent_decode пропускает как есть: такой текст — тоже ошибка кодировки.
            if has_bad_escape(s) {
                Err(bad_encoding())
            } else {
                Ok(v)
            }
        })
}

fn has_bad_escape(s: &str) -> bool {
    let b = s.as_bytes();
    b.iter().enumerate().any(|(i, &c)| {
        c == b'%'
            && !(b.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && b.get(i + 2).is_some_and(u8::is_ascii_hexdigit))
    })
}

fn bad_encoding() -> AppError {
    invalid(ltext!(
        "errors.connstr.badEncoding",
        "Invalid percent-encoding in the connection string"
    ))
}

fn parse_port(s: &str) -> AppResult<u16> {
    s.trim()
        .parse::<u16>()
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| {
            invalid(ltext!(
                "errors.connstr.badPort",
                "Invalid port in the connection string: a number from 1 to 65535 is expected"
            ))
        })
}

/// `host[:port]` или `[ipv6][:port]` — узел номер `n` (с 1).
fn parse_host_port(s: &str, n: usize) -> AppResult<(String, Option<u16>)> {
    let s = s.trim();
    if s.is_empty() || s.starts_with('/') {
        return Err(unix_socket());
    }
    if let Some(rest) = s.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or_else(|| bad_host(n))?;
        let port = match after.strip_prefix(':') {
            Some(p) => Some(parse_port(p)?),
            None if after.is_empty() => None,
            None => return Err(bad_host(n)),
        };
        return Ok((host.to_string(), port));
    }
    match s.rsplit_once(':') {
        Some(("", _)) => Err(unix_socket()),
        Some((h, p)) if !h.contains(':') => Ok((h.to_string(), Some(parse_port(p)?))),
        // IPv6 без скобок: порта нет.
        _ => Ok((s.to_string(), None)),
    }
}

fn parse_uri_body(rest: &str) -> AppResult<Raw> {
    let (main, query) = match rest.split_once('?') {
        Some((m, q)) => (m, Some(q)),
        None => (rest, None),
    };
    let (authority, path) = match main.find('/') {
        Some(i) => (&main[..i], Some(&main[i + 1..])),
        None => (main, None),
    };
    let (userinfo, hostspec) = match authority.rfind('@') {
        Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
        None => (None, authority),
    };
    let (user, password) = match userinfo {
        Some(u) => match u.split_once(':') {
            Some((name, pw)) => (Some(decode(name)?), Some(decode(pw)?)),
            None => (Some(decode(u)?), None),
        },
        None => (None, None),
    };
    let mut hosts = Vec::new();
    if !hostspec.is_empty() {
        for (i, part) in hostspec.split(',').enumerate() {
            hosts.push(parse_host_port(&decode(part)?, i + 1)?);
        }
    }
    let database = path.map(decode).transpose()?.filter(|d| !d.is_empty());
    let mut params = Vec::new();
    for pair in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        params.push((decode(k)?, decode(v)?));
    }
    Ok(Raw {
        hosts,
        user: user.filter(|u| !u.is_empty()),
        password,
        database,
        params,
    })
}

/// Символы conninfo вместе с номером последнего прочитанного (с 1) — для сообщения об ошибке.
struct Cursor<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    read: usize,
}

impl Cursor<'_> {
    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.chars.next();
        self.read += usize::from(c.is_some());
        c
    }

    fn next_if(&mut self, f: impl FnOnce(&char) -> bool) -> Option<char> {
        let c = self.chars.next_if(f);
        self.read += usize::from(c.is_some());
        c
    }

    fn skip_whitespace(&mut self) {
        while self.next_if(|c| c.is_whitespace()).is_some() {}
    }

    /// Ошибка у последнего прочитанного символа или, если строка кончилась, сразу за ней.
    fn error(&self, at_end: bool) -> AppError {
        syntax(self.read + usize::from(at_end))
    }
}

fn parse_conninfo(s: &str) -> AppResult<Raw> {
    let mut params = Vec::new();
    let mut cur = Cursor {
        chars: s.chars().peekable(),
        read: 0,
    };
    loop {
        cur.skip_whitespace();
        if cur.peek().is_none() {
            break;
        }
        let mut key = String::new();
        while let Some(c) = cur.next_if(|c| *c != '=' && !c.is_whitespace()) {
            key.push(c);
        }
        cur.skip_whitespace();
        match cur.next() {
            Some('=') => {}
            other => return Err(cur.error(other.is_none())),
        }
        cur.skip_whitespace();
        let mut value = String::new();
        if cur.next_if(|c| *c == '\'').is_some() {
            loop {
                match cur.next() {
                    Some('\\') => match cur.next() {
                        Some(c) => value.push(c),
                        None => return Err(cur.error(true)),
                    },
                    Some('\'') => break,
                    Some(c) => value.push(c),
                    None => return Err(cur.error(true)),
                }
            }
        } else {
            while let Some(c) = cur.next_if(|c| !c.is_whitespace()) {
                if c == '\\' {
                    if let Some(n) = cur.next() {
                        value.push(n);
                    }
                } else {
                    value.push(c);
                }
            }
        }
        params.push((key, value));
    }
    Ok(Raw {
        hosts: Vec::new(),
        user: None,
        password: None,
        database: None,
        params,
    })
}

fn take(params: &mut Vec<(String, String)>, name: &str) -> Option<String> {
    let i = params.iter().position(|(k, _)| k == name)?;
    Some(params.remove(i).1)
}

/// Порт каждого из `hosts` хостов по значению параметра `port`: одно значение — для всех,
/// список — по позиции; иное число значений — `portsMismatch`.
fn ports_per_host(value: &str, hosts: usize) -> AppResult<Vec<u16>> {
    let ports = value
        .split(',')
        .map(parse_port)
        .collect::<AppResult<Vec<u16>>>()?;
    match ports.len() {
        1 => Ok(vec![ports[0]; hosts]),
        k if k == hosts => Ok(ports),
        k => Err(invalid(ltext!(
            "errors.connstr.portsMismatch",
            "The number of ports ({{ports}}) does not match the number of hosts ({{hosts}})",
            ports = k,
            hosts = hosts
        ))),
    }
}

/// Хосты из параметров `host` / `hostaddr` / `port` (conninfo и `?host=` в URI).
fn hosts_from_params(params: &mut Vec<(String, String)>) -> AppResult<Vec<(String, Option<u16>)>> {
    let names = match take(params, "host") {
        Some(h) => h,
        None => match take(params, "hostaddr") {
            Some(a) => a,
            None => return Ok(Vec::new()),
        },
    };
    let names: Vec<String> = names.split(',').map(|n| n.trim().to_string()).collect();
    if names.iter().any(|n| n.is_empty() || n.starts_with('/')) {
        return Err(unix_socket());
    }
    let ports: Vec<Option<u16>> = match take(params, "port") {
        Some(p) => ports_per_host(&p, names.len())?
            .into_iter()
            .map(Some)
            .collect(),
        None => vec![None; names.len()],
    };
    Ok(names.into_iter().zip(ports).collect())
}

fn parse_sslmode(v: &str) -> AppResult<SslMode> {
    match v.to_ascii_lowercase().as_str() {
        "disable" => Ok(SslMode::Disable),
        "allow" | "prefer" => Ok(SslMode::Prefer),
        "require" => Ok(SslMode::Require),
        "verify-ca" => Ok(SslMode::VerifyCa),
        "verify-full" => Ok(SslMode::VerifyFull),
        _ => Err(invalid(ltext!(
            "errors.connstr.badSslMode",
            "Unknown sslmode value: use disable, allow, prefer, require, verify-ca or verify-full"
        ))),
    }
}

fn parse_target_session_attrs(v: &str) -> AppResult<TargetServerType> {
    match v.to_ascii_lowercase().as_str() {
        "read-write" | "primary" => Ok(TargetServerType::Primary),
        "read-only" | "standby" => Ok(TargetServerType::Standby),
        "prefer-standby" => Ok(TargetServerType::PreferStandby),
        "any" => Ok(TargetServerType::Any),
        _ => Err(invalid(ltext!(
            "errors.connstr.badTargetSessionAttrs",
            "Unknown target_session_attrs value: use read-write, read-only, primary, standby, prefer-standby or any"
        ))),
    }
}

fn parse_target_server_type(v: &str) -> AppResult<TargetServerType> {
    match v.to_ascii_lowercase().as_str() {
        "primary" | "master" | "preferprimary" => Ok(TargetServerType::Primary),
        "secondary" | "standby" => Ok(TargetServerType::Standby),
        "prefersecondary" | "preferstandby" => Ok(TargetServerType::PreferStandby),
        "any" => Ok(TargetServerType::Any),
        _ => Err(invalid(ltext!(
            "errors.connstr.badTargetServerType",
            "Unknown targetServerType value: use primary, secondary, preferSecondary, preferPrimary or any"
        ))),
    }
}

fn parse_timeout(v: &str) -> AppResult<Option<u64>> {
    let n: u64 = v.trim().parse().map_err(|_| {
        invalid(ltext!(
            "errors.connstr.badTimeout",
            "Invalid connect timeout: a whole number of seconds is expected"
        ))
    })?;
    Ok((n > 0).then(|| n.min(MAX_TIMEOUT_S)))
}

fn map(mut raw: Raw, d: Dialect) -> AppResult<ParsedConnection> {
    if raw.hosts.is_empty() {
        raw.hosts = hosts_from_params(&mut raw.params)?;
    } else if let Some(value) = take(&mut raw.params, "port") {
        // Хосты из authority: `?port=` задаёт порт только тем, у кого он не указан в `host:port`.
        let ports = ports_per_host(&value, raw.hosts.len())?;
        for ((_, port), p) in raw.hosts.iter_mut().zip(ports) {
            *port = port.or(Some(p));
        }
    }
    if raw.hosts.is_empty() {
        return Err(invalid(ltext!(
            "errors.connstr.noHosts",
            "No hosts in the connection string"
        )));
    }
    if raw.hosts.len() > MAX_HOSTS {
        return Err(invalid(ltext!(
            "errors.connstr.tooManyHosts",
            "More than 16 hosts in the connection string"
        )));
    }
    let (mut username, mut password, mut database) = (raw.user, raw.password, raw.database);
    let mut ssl_mode = None;
    let mut jdbc_ssl = None;
    let (mut ca_path, mut cert, mut key) = (None, None, None);
    let mut target = None;
    let mut timeout = None;
    let mut ignored = Vec::new();
    for (k, v) in raw.params {
        match (d, k.as_str()) {
            (_, "user") => username = username.or(Some(v)),
            (_, "password") => password = password.or(Some(v)),
            (Dialect::Libpq, "dbname") => database = database.or(Some(v)),
            (_, "sslmode") => ssl_mode = Some(parse_sslmode(&v)?),
            (Dialect::Jdbc, "ssl") => jdbc_ssl = Some(v.eq_ignore_ascii_case("true")),
            (_, "sslrootcert") => ca_path = Some(v),
            (_, "sslcert") => cert = Some(v),
            (_, "sslkey") => key = Some(v),
            (Dialect::Libpq, "target_session_attrs") => {
                target = Some(parse_target_session_attrs(&v)?)
            }
            (Dialect::Jdbc, "targetServerType") => target = Some(parse_target_server_type(&v)?),
            (Dialect::Libpq, "connect_timeout") | (Dialect::Jdbc, "connectTimeout") => {
                timeout = parse_timeout(&v)?
            }
            _ => ignored.push(k),
        }
    }
    if ssl_mode.is_none() {
        // pgjdbc: ssl=true без sslmode — verify-full.
        ssl_mode = jdbc_ssl.map(|on| {
            if on {
                SslMode::VerifyFull
            } else {
                SslMode::Disable
            }
        });
    }
    let client_cert = match (cert, key) {
        (Some(c), k) => {
            let lower = c.to_ascii_lowercase();
            if lower.ends_with(".p12") || lower.ends_with(".pfx") {
                if k.is_some() {
                    ignored.push("sslkey".into());
                }
                Some(ClientCert {
                    format: CertFormat::Pkcs12,
                    certificate_path: c,
                    key_path: None,
                })
            } else {
                Some(ClientCert {
                    format: CertFormat::Pem,
                    certificate_path: c,
                    key_path: k,
                })
            }
        }
        (None, Some(_)) => {
            ignored.push("sslkey".into());
            None
        }
        (None, None) => None,
    };
    let hosts: Vec<HostPort> = raw
        .hosts
        .into_iter()
        .map(|(host, port)| HostPort {
            host,
            port: port.unwrap_or(DEFAULT_PORT),
        })
        .collect();
    let kind = if hosts.len() == 1 {
        ConnectionKind::Server
    } else {
        ConnectionKind::HostList
    };
    Ok(ParsedConnection {
        patch: ProfilePatch {
            kind,
            hosts,
            target_server_type: target,
            username: username.filter(|u| !u.is_empty()),
            database: database.filter(|d| !d.is_empty()),
            ssl_mode,
            ca_path,
            client_cert,
            connect_timeout_s: timeout,
        },
        password: password.filter(|p| !p.is_empty()),
        ignored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hp(host: &str, port: u16) -> HostPort {
        HostPort {
            host: host.into(),
            port,
        }
    }

    fn err_key(s: &str) -> String {
        parse_connection_string(s).unwrap_err().key
    }

    #[test]
    fn jdbc_multi_host() {
        let p = parse_connection_string(
            "jdbc:postgresql://h1:5432,h2:5433/orders?user=app&password=s3cr%40t&targetServerType=primary&ssl=true&ApplicationName=x",
        )
        .unwrap();
        assert_eq!(p.patch.kind, ConnectionKind::HostList);
        assert_eq!(p.patch.hosts, vec![hp("h1", 5432), hp("h2", 5433)]);
        assert_eq!(p.patch.database.as_deref(), Some("orders"));
        assert_eq!(p.patch.username.as_deref(), Some("app"));
        assert_eq!(p.password.as_deref(), Some("s3cr@t"));
        assert_eq!(p.patch.target_server_type, Some(TargetServerType::Primary));
        assert_eq!(
            p.patch.ssl_mode,
            Some(SslMode::VerifyFull),
            "ssl=true в pgjdbc — verify-full"
        );
        assert_eq!(p.ignored, vec!["ApplicationName".to_string()]);
    }

    #[test]
    fn uri_with_userinfo_and_default_ports() {
        let p = parse_connection_string(
            "postgresql://app:secret@h1,h2:5433/orders?target_session_attrs=read-write&sslmode=verify-full",
        )
        .unwrap();
        assert_eq!(p.patch.hosts, vec![hp("h1", 5432), hp("h2", 5433)]);
        assert_eq!(p.password.as_deref(), Some("secret"));
        assert_eq!(p.patch.target_server_type, Some(TargetServerType::Primary));
        assert_eq!(p.patch.ssl_mode, Some(SslMode::VerifyFull));
        assert!(p.ignored.is_empty());
    }

    #[test]
    fn single_host_is_server() {
        let p = parse_connection_string("postgres://db.example.com/app").unwrap();
        assert_eq!(p.patch.kind, ConnectionKind::Server);
        assert_eq!(p.patch.hosts, vec![hp("db.example.com", 5432)]);
        assert_eq!(p.patch.database.as_deref(), Some("app"));
        assert_eq!(p.patch.username, None);
        let p = parse_connection_string("JDBC:PostgreSQL://h/db").unwrap();
        assert_eq!(
            p.patch.hosts,
            vec![hp("h", 5432)],
            "схема без учёта регистра"
        );
    }

    #[test]
    fn conninfo_ports_by_position_or_shared() {
        let p = parse_connection_string(
            "host=h1,h2 port=5432,5433 dbname=orders user=app sslmode=require",
        )
        .unwrap();
        assert_eq!(p.patch.hosts, vec![hp("h1", 5432), hp("h2", 5433)]);
        assert_eq!(p.patch.ssl_mode, Some(SslMode::Require));
        assert_eq!(p.patch.database.as_deref(), Some("orders"));
        let p = parse_connection_string("host=a,b,c port=6432").unwrap();
        assert_eq!(
            p.patch.hosts,
            vec![hp("a", 6432), hp("b", 6432), hp("c", 6432)]
        );
        assert_eq!(
            err_key("host=a,b,c port=1,2"),
            "errors.connstr.portsMismatch"
        );
    }

    #[test]
    fn uri_port_param_with_host_list() {
        let p = parse_connection_string("postgresql://h1,h2/db?port=6432").unwrap();
        assert_eq!(p.patch.hosts, vec![hp("h1", 6432), hp("h2", 6432)]);
        assert!(p.ignored.is_empty());
        let p = parse_connection_string("postgresql://h1,h2/db?port=5433,5434").unwrap();
        assert_eq!(p.patch.hosts, vec![hp("h1", 5433), hp("h2", 5434)]);
        assert!(p.ignored.is_empty());
        let p = parse_connection_string("postgresql://h1:7000,h2/db?port=6432").unwrap();
        assert_eq!(p.patch.hosts, vec![hp("h1", 7000), hp("h2", 6432)]);
        assert!(p.ignored.is_empty());
        assert_eq!(
            err_key("postgresql://h1,h2,h3/db?port=1,2"),
            "errors.connstr.portsMismatch"
        );
    }

    #[test]
    fn conninfo_quoting_and_escapes() {
        let p =
            parse_connection_string(r"host = h   password='it\'s a \\ test' user=u\ x").unwrap();
        assert_eq!(p.password.as_deref(), Some(r"it's a \ test"));
        assert_eq!(p.patch.username.as_deref(), Some("u x"));
        assert_eq!(err_key("host='h"), "errors.connstr.syntax");
        assert_eq!(err_key("host h"), "errors.connstr.syntax");
    }

    #[test]
    fn ipv6_hosts() {
        let p = parse_connection_string("postgresql://[::1]:5433,[fe80::1]/db").unwrap();
        assert_eq!(p.patch.hosts, vec![hp("::1", 5433), hp("fe80::1", 5432)]);
    }

    #[test]
    fn unix_sockets_are_rejected() {
        assert_eq!(
            err_key("host=/var/run/postgresql"),
            "errors.connstr.unixSocket"
        );
        assert_eq!(
            err_key("postgresql://%2Fvar%2Frun%2Fpostgresql/db"),
            "errors.connstr.unixSocket"
        );
        assert_eq!(
            err_key("postgresql://:5432/db"),
            "errors.connstr.unixSocket"
        );
    }

    #[test]
    fn certificates() {
        let p = parse_connection_string("postgresql://h/db?sslcert=/c/client.p12&sslkey=/c/k.pem")
            .unwrap();
        let c = p.patch.client_cert.unwrap();
        assert_eq!(c.format, CertFormat::Pkcs12);
        assert_eq!(c.key_path, None);
        assert_eq!(p.ignored, vec!["sslkey".to_string()]);
        let p = parse_connection_string(
            "postgresql://h/db?sslcert=/c/c.pem&sslkey=/c/k.pem&sslrootcert=/c/ca.pem",
        )
        .unwrap();
        let c = p.patch.client_cert.unwrap();
        assert_eq!(c.format, CertFormat::Pem);
        assert_eq!(c.key_path.as_deref(), Some("/c/k.pem"));
        assert_eq!(p.patch.ca_path.as_deref(), Some("/c/ca.pem"));
    }

    #[test]
    fn hostaddr_is_used_without_host() {
        let p = parse_connection_string("hostaddr=10.0.0.1 port=5432 user=u").unwrap();
        assert_eq!(p.patch.hosts, vec![hp("10.0.0.1", 5432)]);
        let p = parse_connection_string("host=h hostaddr=10.0.0.1").unwrap();
        assert_eq!(p.patch.hosts, vec![hp("h", 5432)]);
        assert_eq!(p.ignored, vec!["hostaddr".to_string()]);
    }

    #[test]
    fn timeouts() {
        assert_eq!(
            parse_connection_string("host=h connect_timeout=500")
                .unwrap()
                .patch
                .connect_timeout_s,
            Some(120)
        );
        assert_eq!(
            parse_connection_string("host=h connect_timeout=0")
                .unwrap()
                .patch
                .connect_timeout_s,
            None
        );
        assert_eq!(
            parse_connection_string("jdbc:postgresql://h/db?connectTimeout=7")
                .unwrap()
                .patch
                .connect_timeout_s,
            Some(7)
        );
        assert_eq!(
            err_key("host=h connect_timeout=abc"),
            "errors.connstr.badTimeout"
        );
    }

    #[test]
    fn target_mappings() {
        let t = |s: &str| parse_connection_string(s).unwrap().patch.target_server_type;
        assert_eq!(
            t("host=h target_session_attrs=prefer-standby"),
            Some(TargetServerType::PreferStandby)
        );
        assert_eq!(
            t("host=h target_session_attrs=read-only"),
            Some(TargetServerType::Standby)
        );
        assert_eq!(
            t("host=h target_session_attrs=any"),
            Some(TargetServerType::Any)
        );
        assert_eq!(
            t("jdbc:postgresql://h/db?targetServerType=preferSecondary"),
            Some(TargetServerType::PreferStandby)
        );
        assert_eq!(
            t("jdbc:postgresql://h/db?targetServerType=secondary"),
            Some(TargetServerType::Standby)
        );
        assert_eq!(
            t("jdbc:postgresql://h/db?targetServerType=master"),
            Some(TargetServerType::Primary)
        );
        assert_eq!(
            err_key("host=h target_session_attrs=weird"),
            "errors.connstr.badTargetSessionAttrs"
        );
        assert_eq!(
            err_key("jdbc:postgresql://h/db?targetServerType=weird"),
            "errors.connstr.badTargetServerType"
        );
    }

    #[test]
    fn ssl_modes() {
        let m = |s: &str| parse_connection_string(s).unwrap().patch.ssl_mode;
        assert_eq!(m("host=h sslmode=allow"), Some(SslMode::Prefer));
        assert_eq!(m("host=h sslmode=verify-ca"), Some(SslMode::VerifyCa));
        assert_eq!(
            m("jdbc:postgresql://h/db?ssl=false"),
            Some(SslMode::Disable)
        );
        assert_eq!(
            m("jdbc:postgresql://h/db?ssl=true&sslmode=require"),
            Some(SslMode::Require)
        );
        assert_eq!(m("host=h"), None);
        assert_eq!(err_key("host=h sslmode=on"), "errors.connstr.badSslMode");
    }

    #[test]
    fn errors_never_echo_parts_of_the_string() {
        // Неэкранированный `/`, `?` или пробел в пароле сдвигает разбор: часть пароля оказывается «портом»,
        // «значением sslmode» или «ключом». Ошибка называет параметр, узел или позицию, но не текст.
        for (text, key) in [
            (
                "postgresql://user:s3cr/3t@host/db",
                "errors.connstr.badPort",
            ),
            ("postgresql://user:s3cr?3t@host", "errors.connstr.badPort"),
            ("host=h password=s3cr 3t", "errors.connstr.syntax"),
            (
                "host=h password=s3cr 3t=x sslmode=s3cr3t",
                "errors.connstr.badSslMode",
            ),
            ("host=h connect_timeout=s3cr3t", "errors.connstr.badTimeout"),
            ("host=h port=s3cr3t", "errors.connstr.badPort"),
            (
                "host=h target_session_attrs=s3cr3t",
                "errors.connstr.badTargetSessionAttrs",
            ),
            (
                "jdbc:postgresql://h/db?targetServerType=s3cr3t",
                "errors.connstr.badTargetServerType",
            ),
            ("postgresql://h1,[s3cr3t/db", "errors.connstr.badHost"),
            ("postgresql://[::1]s3cr3t/db", "errors.connstr.badHost"),
            ("host='s3cr3t", "errors.connstr.syntax"),
            ("host=h password='s3cr3t\\", "errors.connstr.syntax"),
        ] {
            let e = parse_connection_string(text).unwrap_err();
            assert_eq!(e.key, key, "{text}");
            let shown = format!("{e:?}");
            for fragment in ["s3cr", "3t"] {
                assert!(!shown.contains(fragment), "{text}: {shown}");
            }
        }
        let e = parse_connection_string("postgresql://h1,[x/db").unwrap_err();
        assert_eq!(e.params["n"], "2", "номер узла");
    }

    #[test]
    fn syntax_errors_point_at_the_position() {
        let at = |s: &str| parse_connection_string(s).unwrap_err().params["position"].clone();
        assert_eq!(at("host h"), "6", "символ вместо «=»");
        assert_eq!(at("host='h"), "8", "строка кончилась внутри кавычек");
        assert_eq!(at("host=h user"), "12", "строка кончилась без «=»");
    }

    #[test]
    fn debug_hides_the_password() {
        let p = parse_connection_string("postgresql://u:s3cr3t@h/db").unwrap();
        assert_eq!(p.password.as_deref(), Some("s3cr3t"));
        let shown = format!("{p:?}");
        assert!(
            !shown.contains("s3cr3t") && shown.contains("***"),
            "{shown}"
        );
    }

    #[test]
    fn malformed_input() {
        assert_eq!(err_key("hello"), "errors.connstr.unrecognized");
        assert_eq!(err_key("dbname=x"), "errors.connstr.noHosts");
        assert_eq!(err_key("postgresql://h:99999/db"), "errors.connstr.badPort");
        assert_eq!(err_key("postgresql://h/%zz"), "errors.connstr.badEncoding");
        let many = (0..17)
            .map(|i| format!("h{i}"))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            err_key(&format!("host={many}")),
            "errors.connstr.tooManyHosts"
        );
    }
}
