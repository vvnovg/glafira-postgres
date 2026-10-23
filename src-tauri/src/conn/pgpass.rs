//! Пароль из `~/.pgpass` (или файла `PGPASSFILE`) по правилам libpq: первая подходящая строка
//! `host:port:database:user:password`, `*` — любое значение, `\:`, `\\` и `\*` — экранирование.

use std::path::{Path, PathBuf};

/// Поле строки .pgpass: `*` без экранирования — любое значение, `\*` — буквальная звёздочка (как в libpq).
struct Field {
    text: String,
    wildcard: bool,
}

/// Делит строку по неэкранированным `:`, снимая экранирование.
fn split_fields(line: &str) -> Vec<Field> {
    // (текст, был ли в нём экранированный символ)
    let mut fields: Vec<(String, bool)> = vec![(String::new(), false)];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        let last = fields.last_mut().expect("at least one field");
        match c {
            '\\' => match chars.next() {
                Some(n) => {
                    last.0.push(n);
                    last.1 = true;
                }
                // Одинокий обратный слеш в конце строки libpq оставляет как есть.
                None => last.0.push('\\'),
            },
            ':' => fields.push((String::new(), false)),
            c => last.0.push(c),
        }
    }
    fields
        .into_iter()
        .map(|(text, escaped)| Field {
            wildcard: text == "*" && !escaped,
            text,
        })
        .collect()
}

pub fn lookup(content: &str, host: &str, port: u16, database: &str, user: &str) -> Option<String> {
    let port = port.to_string();
    let matches = |f: &Field, value: &str| f.wildcard || f.text == value;
    content
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(split_fields)
        .filter(|f| f.len() >= 5)
        .find(|f| {
            matches(&f[0], host)
                && matches(&f[1], &port)
                && matches(&f[2], database)
                && matches(&f[3], user)
        })
        .map(|f| f[4].text.clone())
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
    lookup(&String::from_utf8_lossy(&bytes), host, port, database, user)
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
        let file = "\\*:5432:db:u:star-host\n*:*:*:u:x\\";
        assert_eq!(
            lookup(file, "*", 5432, "db", "u").as_deref(),
            Some("star-host")
        );
        assert_eq!(lookup(file, "h", 5432, "db", "u").as_deref(), Some("x\\"));
    }

    #[test]
    fn non_utf8_bytes_do_not_hide_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pgpass");
        std::fs::write(&path, b"# \xff\xfe comment\nh:5432:db:u:secret\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(
            find_password_in(&path, "h", 5432, "db", "u").as_deref(),
            Some("secret")
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
