//! Строка подключения → поля формы (спец. P1 §4): JDBC, URI `postgresql://`, conninfo `key=value`.
//! Пароль из строки возвращается отдельно (форма отправит его в keychain), нераспознанные параметры —
//! только именами: значения могут содержать секреты.

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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedConnection {
    pub patch: ProfilePatch,
    pub password: Option<String>,
    pub ignored: Vec<String>,
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

fn syntax(near: &str) -> AppError {
    invalid(ltext!(
        "errors.connstr.syntax",
        "Connection string syntax error near “{{near}}”",
        near = near
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
                "Invalid port: {{port}}",
                port = s
            ))
        })
}

fn parse_host_port(s: &str) -> AppResult<(String, Option<u16>)> {
    let s = s.trim();
    if s.is_empty() || s.starts_with('/') {
        return Err(unix_socket());
    }
    if let Some(rest) = s.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or_else(|| syntax(s))?;
        let port = match after.strip_prefix(':') {
            Some(p) => Some(parse_port(p)?),
            None if after.is_empty() => None,
            None => return Err(syntax(s)),
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
        for part in hostspec.split(',') {
            hosts.push(parse_host_port(&decode(part)?)?);
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

fn parse_conninfo(s: &str) -> AppResult<Raw> {
    let mut params = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.peek().is_none() {
            break;
        }
        let mut key = String::new();
        while let Some(c) = chars.next_if(|c| *c != '=' && !c.is_whitespace()) {
            key.push(c);
        }
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.next() != Some('=') {
            return Err(syntax(&key));
        }
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let mut value = String::new();
        if chars.next_if_eq(&'\'').is_some() {
            loop {
                match chars.next() {
                    Some('\\') => value.push(chars.next().ok_or_else(|| syntax(&key))?),
                    Some('\'') => break,
                    Some(c) => value.push(c),
                    None => return Err(syntax(&key)),
                }
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                if c == '\\' {
                    if let Some(n) = chars.next() {
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
    let ports: Vec<u16> = match take(params, "port") {
        Some(p) => p.split(',').map(parse_port).collect::<AppResult<_>>()?,
        None => Vec::new(),
    };
    match ports.len() {
        0 => Ok(names.into_iter().map(|n| (n, None)).collect()),
        1 => Ok(names.into_iter().map(|n| (n, Some(ports[0]))).collect()),
        k if k == names.len() => Ok(names
            .into_iter()
            .zip(ports)
            .map(|(n, p)| (n, Some(p)))
            .collect()),
        k => Err(invalid(ltext!(
            "errors.connstr.portsMismatch",
            "The number of ports ({{ports}}) does not match the number of hosts ({{hosts}})",
            ports = k,
            hosts = names.len()
        ))),
    }
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
            "Unknown sslmode: {{value}}",
            value = v
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
            "Unknown target_session_attrs: {{value}}",
            value = v
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
            "Unknown targetServerType: {{value}}",
            value = v
        ))),
    }
}

fn parse_timeout(v: &str) -> AppResult<Option<u64>> {
    let n: u64 = v.trim().parse().map_err(|_| {
        invalid(ltext!(
            "errors.connstr.badTimeout",
            "Invalid connect timeout: {{value}}",
            value = v
        ))
    })?;
    Ok((n > 0).then(|| n.min(MAX_TIMEOUT_S)))
}

fn map(mut raw: Raw, d: Dialect) -> AppResult<ParsedConnection> {
    if raw.hosts.is_empty() {
        raw.hosts = hosts_from_params(&mut raw.params)?;
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
