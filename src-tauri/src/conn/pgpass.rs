//! Пароль из `~/.pgpass` (или файла `PGPASSFILE`) по правилам libpq: первая подходящая строка
//! `host:port:database:user:password`, `*` — любое значение, `\:`, `\\` и `\*` — экранирование.

use std::path::{Path, PathBuf};

/// Поле строки .pgpass: `*` без экранирования — любое значение, `\*` — буквальная звёздочка (как в libpq).
struct Field {
    bytes: Vec<u8>,
    wildcard: bool,
}

/// Делит строку по неэкранированным `:`, снимая экранирование. Работает с байтами, как libpq: `\` и `:` в
/// UTF-8 не встречаются внутри многобайтовых символов, поэтому для текста в UTF-8 результат тот же, что и
/// посимвольно, а байты не в UTF-8 доходят до поля без искажений.
fn split_fields(line: &[u8]) -> Vec<Field> {
    // (байты, был ли в них экранированный символ)
    let mut fields: Vec<(Vec<u8>, bool)> = vec![(Vec::new(), false)];
    let mut bytes = line.iter().copied();
    while let Some(b) = bytes.next() {
        let last = fields.last_mut().expect("at least one field");
        match b {
            b'\\' => match bytes.next() {
                Some(n) => {
                    last.0.push(n);
                    last.1 = true;
                }
                // Одинокий обратный слеш в конце строки libpq оставляет как есть.
                None => last.0.push(b'\\'),
            },
            b':' => fields.push((Vec::new(), false)),
            b => last.0.push(b),
        }
    }
    fields
        .into_iter()
        .map(|(bytes, escaped)| Field {
            wildcard: bytes == b"*" && !escaped,
            bytes,
        })
        .collect()
}

/// Пароль из содержимого файла в UTF-8 (см. `lookup_bytes`).
pub fn lookup(content: &str, host: &str, port: u16, database: &str, user: &str) -> Option<String> {
    lookup_bytes(content.as_bytes(), host, port, database, user)
}

/// Первая подходящая строка содержимого `.pgpass`. Поля сравниваются побайтно, как в libpq. Пароль не в UTF-8
/// (файл в cp1251 и т. п.) передать серверу без искажения нельзя: libpq отправил бы сырые байты, а замена на
/// U+FFFD дала бы неверный пароль, и отказ во входе пул запомнил бы до переоткрытия подключения. Такая строка
/// пропускается с предупреждением в журнале, поиск идёт дальше.
fn lookup_bytes(
    content: &[u8],
    host: &str,
    port: u16,
    database: &str,
    user: &str,
) -> Option<String> {
    let port = port.to_string();
    let matches = |f: &Field, value: &str| f.wildcard || f.bytes == value.as_bytes();
    for (n, line) in content.split(|&b| b == b'\n').enumerate() {
        // Как libpq (`pg_strip_crlf`): все `\r` в конце строки.
        let mut line = line;
        while let Some(l) = line.strip_suffix(b"\r") {
            line = l;
        }
        if line.is_empty() || line.starts_with(b"#") {
            continue;
        }
        let f = split_fields(line);
        if f.len() < 5
            || !(matches(&f[0], host)
                && matches(&f[1], &port)
                && matches(&f[2], database)
                && matches(&f[3], user))
        {
            continue;
        }
        match String::from_utf8(f[4].bytes.clone()) {
            Ok(password) => return Some(password),
            Err(_) => tracing::warn!(
                line = n + 1,
                "pgpass line skipped: the password is not valid UTF-8"
            ),
        }
    }
    None
}

pub fn default_path() -> Option<PathBuf> {
    default_path_from(std::env::var_os("PGPASSFILE"))
}

/// `PGPASSFILE`: пустое значение — как не заданное (libpq берёт файл по умолчанию).
fn default_path_from(env: Option<std::ffi::OsString>) -> Option<PathBuf> {
    if let Some(p) = env.filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    #[cfg(windows)]
    {
        dirs::config_dir().map(|d| d.join("postgresql").join("pgpass.conf"))
    }
    #[cfg(not(windows))]
    {
        dirs::home_dir().map(|h| h.join(".pgpass"))
    }
}

/// Пароль для подключения или `None`. Блокирующий вызов (чтение файла).
pub fn find_password(host: &str, port: u16, database: &str, user: &str) -> Option<String> {
    find_password_in(&default_path()?, host, port, database, user)
}

/// Как libpq, на Unix файл с доступом для группы или всех игнорируется.
pub fn find_password_in(
    path: &Path,
    host: &str,
    port: u16,
    database: &str,
    user: &str,
) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            tracing::warn!(path = %path.display(), "pgpass ignored: group or world access");
            return None;
        }
    }
    // Байты не в UTF-8 (например, в комментарии) не прячут файл целиком: libpq читает его побайтно.
    let bytes = std::fs::read(path).ok()?;
    lookup_bytes(&bytes, host, port, database, user)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
# комментарий
db1:5432:orders:app:first

db1:5432:*:app:second
*:*:*:admin:adm\\:in
h\\:x:5432:db:u:esc\\\\aped
short:line
";

    #[test]
    fn first_matching_line_wins() {
        assert_eq!(
            lookup(FILE, "db1", 5432, "orders", "app").as_deref(),
            Some("first")
        );
        assert_eq!(
            lookup(FILE, "db1", 5432, "billing", "app").as_deref(),
            Some("second")
        );
        assert_eq!(lookup(FILE, "db1", 5433, "orders", "app"), None);
    }

    #[test]
    fn wildcards_and_escapes() {
        assert_eq!(
            lookup(FILE, "any", 1, "x", "admin").as_deref(),
            Some("adm:in")
        );
        assert_eq!(
            lookup(FILE, "h:x", 5432, "db", "u").as_deref(),
            Some("esc\\aped")
        );
        assert_eq!(lookup(FILE, "short", 1, "line", "u"), None);
    }

    #[test]
    fn password_stops_at_unescaped_colon() {
        assert_eq!(
            lookup("h:1:d:u:pa:ss", "h", 1, "d", "u").as_deref(),
            Some("pa")
        );
        assert_eq!(
            lookup("h:1:d:u:p\r\n", "h", 1, "d", "u").as_deref(),
            Some("p")
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_with_group_access_is_ignored() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pgpass");
        std::fs::write(&path, "h:1:d:u:pw\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            find_password_in(&path, "h", 1, "d", "u").as_deref(),
            Some("pw")
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(find_password_in(&path, "h", 1, "d", "u"), None);
    }

    /// libpq: `\*` — буквальная звёздочка, а не любое значение; одинокий обратный слеш в конце остаётся.
    #[test]
    fn escaped_star_is_literal_and_trailing_backslash_is_kept() {
        // `\*` — не шаблон: строка подходит только к хосту с именем `*`.
        let only = "\\*:5432:db:u:star-host";
        assert_eq!(
            lookup(only, "*", 5432, "db", "u").as_deref(),
            Some("star-host")
        );
        assert_eq!(lookup(only, "h", 5432, "db", "u"), None);

        let file = "\\*:5432:db:u:star-host\n*:*:*:u:x\\";
        assert_eq!(
            lookup(file, "*", 5432, "db", "u").as_deref(),
            Some("star-host")
        );
        assert_eq!(lookup(file, "h", 5432, "db", "u").as_deref(), Some("x\\"));
    }

    /// Файл `.pgpass` с правами только для владельца (иначе `find_password_in` его не читает).
    fn private_file(dir: &Path, content: &[u8]) -> PathBuf {
        let path = dir.join("pgpass");
        std::fs::write(&path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        path
    }

    #[test]
    fn non_utf8_bytes_do_not_hide_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = private_file(dir.path(), b"# \xff\xfe comment\nh:5432:db:u:secret\n");
        assert_eq!(
            find_password_in(&path, "h", 5432, "db", "u").as_deref(),
            Some("secret")
        );
    }

    /// Пароль в cp1251 («Пароль») не превращается в U+FFFD: такая строка пропускается, а следующая подходящая
    /// строка в UTF-8 находится.
    #[test]
    fn password_not_in_utf8_skips_the_line() {
        let cp1251 = b"\xcf\xe0\xf0\xee\xeb\xfc";
        let mut only = b"h:5432:db:u:".to_vec();
        only.extend_from_slice(cp1251);
        only.push(b'\n');
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            find_password_in(&private_file(dir.path(), &only), "h", 5432, "db", "u"),
            None
        );

        let mut with_fallback = only.clone();
        with_fallback.extend_from_slice("*:*:*:u:ok-пароль\r\n".as_bytes());
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            find_password_in(
                &private_file(dir.path(), &with_fallback),
                "h",
                5432,
                "db",
                "u"
            )
            .as_deref(),
            Some("ok-пароль")
        );
    }

    #[test]
    fn empty_pgpassfile_means_the_default_location() {
        assert_eq!(default_path_from(Some("".into())), default_path_from(None));
        assert_eq!(
            default_path_from(Some("/x/pgpass".into())),
            Some(PathBuf::from("/x/pgpass"))
        );
    }

    #[test]
    fn missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            find_password_in(&dir.path().join("nope"), "h", 1, "d", "u"),
            None
        );
    }
}
