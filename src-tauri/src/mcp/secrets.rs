//! Вычистка секретов из значений `pg_settings` (спец. P4 §6.2, уточнение 6 плана P4a): строка подключения
//! (`primary_conninfo` и другие `…conninfo`) теряет `password` и `sslpassword`, команды (`*_command`) и значения прочих
//! параметров — значения паролей, токенов и ключей (пары `ИМЯ=значение` и `имя = значение`, опции `--имя значение`,
//! пароль в URI `user:password@`); параметр с секретом в самом имени (`myext.api_token`, `myext.dbpass`, `anon.salt`)
//! и команда с секретом в имени (`ssl_passphrase_command`) скрываются целиком. Функция чистая: `get_settings` зовёт её
//! на каждой строке. Разбор команд — эвристика: секрет без имени (`curl -u user:pw`) она не узнает.

const MASK: &str = "***";
/// Части имени переменной окружения или опции команды, значение которых — секрет (`PGPASSWORD`, `--password`,
/// `--repo1-cipher-pass`, `AWS_SECRET_ACCESS_KEY`, `API_TOKEN`, `-passin`).
const SECRET_WORDS: [&str; 5] = ["pass", "secret", "token", "key", "credential"];
/// Подстроки имени самого параметра сервера, значение которого — секрет; `key` проверяется отдельно
/// (`secret_setting`).
const SECRET_SETTING_WORDS: [&str; 6] = [
    "password",
    "passwd",
    "passphrase",
    "secret",
    "token",
    "credential",
];
/// Окончания слова имени параметра (слова — через `.`, `_`, `-`), значение которого — секрет: `myext.dbpass`,
/// `myext.db_pwd`, `anon.salt`. `pgtle.passcheck_db_name` (имя базы) сюда не попадает.
const SECRET_SETTING_ENDINGS: [&str; 3] = ["pass", "pwd", "salt"];
/// Параметры ядра с «секретным» словом в имени, значение которых — не секрет.
const PLAIN_SETTINGS: [&str; 3] = [
    "password_encryption",
    "md5_password_warnings",
    "ssl_passphrase_command_supports_reload",
];
/// Значения-флаги: секретом не бывают (`credcheck.password_contain_username = on`).
const FLAGS: [&str; 6] = ["on", "off", "true", "false", "yes", "no"];
/// Ключи строки подключения libpq, значение которых — пароль (в парах и в запросе URI); сравниваются без регистра.
const CONNINFO_SECRETS: [&str; 2] = ["password", "sslpassword"];

/// Значение параметра без секретов и признак «значение изменено» (`hidden` в ответе `get_settings`).
pub fn redact_setting(name: &str, value: &str) -> (String, bool) {
    if value.is_empty() {
        return (String::new(), false);
    }
    let lower = name.to_ascii_lowercase();
    let command = lower.ends_with("_command");
    let out = if secret_setting(&lower) && (command || !password_knob(&lower, value)) {
        // Спец. §6.2: вывод `ssl_passphrase_command` — пароль ключа, а сама команда обычно его и содержит
        // (`echo FooBaR1`, `printf %s s3cret`) — поэтому команда с секретом в имени скрывается целиком.
        MASK.to_string()
    } else if command {
        redact_command(value)
    } else if lower.contains("conninfo") {
        redact_conninfo(value)
    } else {
        // Строка подключения или команда бывает и в параметре расширения с другим именем (`myext.connstr`).
        redact_command(value)
    };
    let hidden = out != value;
    (out, hidden)
}

/// Секрет в имени параметра: пароль, фраза-пароль, секрет, токен, учётные данные (`SECRET_SETTING_WORDS`), слово на
/// `pass`, `pwd`, `salt` или ключ — слово последней части имени (после точки), кончающееся на `key` (`openai_key`,
/// `apikey`), но не путь к файлу ключа (`ssl_key_file`, `krb_server_keyfile`) и не `max_index_keys`.
fn secret_setting(lower: &str) -> bool {
    if PLAIN_SETTINGS.contains(&lower) {
        return false;
    }
    let key = lower
        .rsplit('.')
        .next()
        .unwrap_or(lower)
        .split(['_', '-'])
        .any(|part| part.ends_with("key"));
    let ending = lower
        .split(['.', '_', '-'])
        .any(|w| SECRET_SETTING_ENDINGS.iter().any(|e| w.ends_with(e)));
    SECRET_SETTING_WORDS.iter().any(|w| lower.contains(w))
        || ending
        || (key && !lower.ends_with("file"))
}

/// Параметр с секретным словом в имени — настройка паролей, а не пароль: значение — флаг (`on`, `off`, `true`,
/// `false`, `yes`, `no`), или имя — ручка (`enable_…`, `…_length`, `…_check`, слово `min` или `max` в последней части)
/// и значение — число или одно слово строчными латинскими буквами (перечисление: `require`). Иначе значение
/// скрывается: лишнее спрятать безопаснее, чем отдать пароль (`myext.dbpass = 123456`, `anon.salt = 42`).
fn password_knob(lower: &str, value: &str) -> bool {
    if FLAGS.iter().any(|f| value.eq_ignore_ascii_case(f)) {
        return true;
    }
    let words: Vec<&str> = lower
        .rsplit('.')
        .next()
        .unwrap_or(lower)
        .split(['_', '-'])
        .collect();
    let knob = words.first() == Some(&"enable")
        || matches!(words.last(), Some(&("length" | "check")))
        || words.iter().any(|w| matches!(*w, "min" | "max"));
    let number = value.parse::<f64>().is_ok_and(f64::is_finite)
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+'));
    let keyword = value.len() <= 32
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || matches!(c, '-' | '_'));
    knob && (number || keyword)
}

fn secret_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    SECRET_WORDS.iter().any(|w| n.contains(w))
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// Длина значения строки подключения в начале `s` (правила libpq): в кавычках `'…'` — до закрывающей кавычки
/// включительно (незакрытая — до конца), без кавычек — до пробела; обратная косая экранирует следующий символ.
fn conninfo_value_len(s: &str) -> usize {
    let quoted = s.starts_with(['\'', '"']);
    let quote = s.chars().next().filter(|_| quoted);
    let mut chars = s.char_indices().skip(usize::from(quoted));
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if Some(c) == quote {
            return i + c.len_utf8();
        } else if quote.is_none() && c.is_whitespace() {
            return i;
        }
    }
    s.len()
}

/// Длина слова оболочки в начале `s`: до пробела или разделителя (`;`, `&`, `|`) вне кавычек; `'…'` и `"…"` внутри
/// слова — его часть (`ab"c d"e` — одно слово), обратная косая вне `'…'` экранирует следующий символ. Незакрытая
/// кавычка тянет слово до конца.
fn shell_word_len(s: &str) -> usize {
    let mut quote: Option<char> = None;
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if c == '\\' => {
                chars.next();
            }
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c == '\\' => {
                chars.next();
            }
            None if c.is_whitespace() || matches!(c, ';' | '&' | '|') => return i,
            None => {}
        }
    }
    s.len()
}

/// Пары `key = value` строки подключения libpq (значение — как в тексте, с кавычками); не пары — `None`.
fn conninfo_pairs(s: &str) -> Option<Vec<(&str, &str)>> {
    let mut out = Vec::new();
    let mut rest = s.trim_start();
    while !rest.is_empty() {
        let key_len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if key_len == 0 {
            return None;
        }
        let key = &rest[..key_len];
        let after = rest[key_len..].trim_start().strip_prefix('=')?.trim_start();
        let len = conninfo_value_len(after);
        out.push((key, &after[..len]));
        rest = after[len..].trim_start();
    }
    Some(out)
}

/// Тело URI после `://`: пароль в `user:password@` и параметры запроса `password=` / `sslpassword=` убираются.
/// `user[:password]@` libpq ищет до первого `/`: пароль без кодирования может содержать `?`, `#` и `@`, поэтому
/// берётся последний `@` перед `/` (лишнее убрать безопаснее, чем отдать пароль), а узел, путь и запрос — после него.
fn redact_uri_body(body: &str) -> String {
    let path_start = body.find('/').unwrap_or(body.len());
    let (user, rest) = match body[..path_start].rfind('@') {
        Some(at) => (
            Some(body[..at].split(':').next().unwrap_or_default()),
            &body[at + 1..],
        ),
        None => (None, body),
    };
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, tail) = rest.split_at(host_end);
    let tail = match tail.split_once('?') {
        Some((path, query)) => {
            let kept: Vec<&str> = query
                .split('&')
                .filter(|kv| {
                    let kv = kv.to_ascii_lowercase();
                    !CONNINFO_SECRETS
                        .iter()
                        .any(|k| kv.strip_prefix(k).is_some_and(|r| r.starts_with('=')))
                })
                .collect();
            if kept.is_empty() {
                path.to_string()
            } else {
                format!("{path}?{}", kept.join("&"))
            }
        }
        None => tail.to_string(),
    };
    match user {
        Some(user) => format!("{user}@{host}{tail}"),
        None => format!("{host}{tail}"),
    }
}

/// Все URI в тексте (`postgresql://…`, `https://…`): тело — до пробела или кавычки, чистится `redact_uri_body`.
fn redact_uris(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"'))
            .unwrap_or(tail.len());
        out.push_str(&redact_uri_body(&tail[..end]));
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Строка подключения: URI или пары `key=value` без `password` и `sslpassword` (значение без пароля не
/// переписывается); разобрать не удалось — как команда.
fn redact_conninfo(value: &str) -> String {
    let head = value.trim_start();
    if head.starts_with("postgresql://") || head.starts_with("postgres://") {
        return redact_uris(value);
    }
    let secret = |k: &str| CONNINFO_SECRETS.iter().any(|s| k.eq_ignore_ascii_case(s));
    match conninfo_pairs(value) {
        Some(pairs) if pairs.iter().any(|(k, _)| secret(k)) => pairs
            .iter()
            .filter(|(k, _)| !secret(k))
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" "),
        Some(_) => value.to_string(),
        None => redact_command(value),
    }
}

/// Команда (и значение параметра, не узнанного по имени): пароли в URI, значения пар `ИМЯ=значение` и опций
/// `--имя значение` с секретным именем.
fn redact_command(value: &str) -> String {
    redact_option_values(&redact_assignments(&redact_uris(value)))
}

/// Пары `ИМЯ=значение` в команде и `имя = значение` (libpq разрешает пробелы вокруг `=`): значение секретного имени
/// (`SECRET_WORDS`, без учёта регистра) — `***` (слово оболочки целиком), остальное как есть.
fn redact_assignments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(eq) = rest.find('=') {
        let (before, after) = rest.split_at(eq);
        let name = before.trim_end();
        let name_start = name
            .char_indices()
            .rev()
            .find(|(_, c)| !is_name_char(*c))
            .map_or(0, |(i, c)| i + c.len_utf8());
        out.push_str(before);
        out.push('=');
        let after = &after[1..];
        if secret_name(&name[name_start..]) {
            // Пробелы (и лишние `=`) перед значением остаются, значение — слово после них.
            let value = after.trim_start_matches(|c: char| c.is_whitespace() || c == '=');
            out.push_str(&after[..after.len() - value.len()]);
            out.push_str(MASK);
            rest = &value[shell_word_len(value)..];
        } else {
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Значение опции через пробел (`--s3-secret-access-key KEY`, `-passin pass:…`): слово после опции с секретным именем
/// (если это не другая опция) — `***`.
fn redact_option_values(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    let mut secret_next = false;
    loop {
        let word_start = rest.len() - rest.trim_start().len();
        out.push_str(&rest[..word_start]);
        rest = &rest[word_start..];
        let Some(c) = rest.chars().next() else {
            break;
        };
        if matches!(c, ';' | '&' | '|') {
            out.push(c);
            rest = &rest[c.len_utf8()..];
            secret_next = false;
            continue;
        }
        let word = &rest[..shell_word_len(rest)];
        if secret_next && !word.starts_with('-') {
            out.push_str(MASK);
            secret_next = false;
        } else {
            out.push_str(word);
            secret_next = word.starts_with('-') && !word.contains('=') && secret_name(word);
        }
        rest = &rest[word.len()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conninfo_loses_only_the_password() {
        let (v, hidden) = redact_setting(
            "primary_conninfo",
            "user=replicator password=replicator-secret channel_binding=prefer host=pg-primary port=5432 sslmode=prefer",
        );
        assert_eq!(
            v,
            "user=replicator channel_binding=prefer host=pg-primary port=5432 sslmode=prefer"
        );
        assert!(hidden);
        let (v, hidden) = redact_setting(
            "primary_conninfo",
            r"host=db password = 'a b\'c' application_name=pg2",
        );
        assert_eq!(v, "host=db application_name=pg2");
        assert!(hidden);
        let plain = "host=db port=5432 user=r";
        assert_eq!(
            redact_setting("primary_conninfo", plain),
            (plain.to_string(), false),
            "без пароля значение не меняется (и пробелы тоже)"
        );
    }

    #[test]
    fn conninfo_uri_loses_the_password() {
        let (v, hidden) = redact_setting(
            "primary_conninfo",
            "postgresql://rep:s3cret@db:5432/postgres?sslmode=require&password=x",
        );
        assert_eq!(v, "postgresql://rep@db:5432/postgres?sslmode=require");
        assert!(hidden);
    }

    #[test]
    fn commands_lose_secret_values() {
        let (v, hidden) = redact_setting(
            "archive_command",
            "PGPASSWORD=s3cret pg_receivewal --password=x -D /arch && AWS_SECRET_ACCESS_KEY=abc aws s3 cp %p s3://b/%f",
        );
        assert_eq!(
            v,
            "PGPASSWORD=*** pg_receivewal --password=*** -D /arch && AWS_SECRET_ACCESS_KEY=*** aws s3 cp %p s3://b/%f"
        );
        assert!(hidden);
        let (v, _) = redact_setting("restore_command", "API_TOKEN=\"a b\" fetch %f %p");
        assert_eq!(v, "API_TOKEN=*** fetch %f %p");
        let plain = "cp /arch/%f %p";
        assert_eq!(
            redact_setting("restore_command", plain),
            (plain.to_string(), false)
        );
        assert_eq!(
            redact_setting("ssl_passphrase_command", "echo key=topsecret"),
            ("***".to_string(), true),
            "команда пароля ключа — целиком"
        );
    }

    #[test]
    fn secret_named_settings_are_hidden_and_others_kept() {
        assert_eq!(
            redact_setting("myext.api_token", "abc"),
            ("***".to_string(), true)
        );
        assert_eq!(
            redact_setting("ssl_key_file", "server.key"),
            ("server.key".to_string(), false),
            "путь к ключу — не секрет"
        );
        assert_eq!(
            redact_setting("work_mem", "4096"),
            ("4096".to_string(), false)
        );
        assert_eq!(redact_setting("myext.password", ""), (String::new(), false));
    }

    // Сверх плана: другие ключи паролей libpq, экранирование, URI и опции в командах, имена параметров.

    #[test]
    fn conninfo_loses_the_ssl_key_password_and_escaped_values_stay_whole() {
        let (v, hidden) = redact_setting(
            "primary_conninfo",
            "host=db sslpassword=k3y sslkey=/k/client.key user=r",
        );
        assert_eq!(v, "host=db sslkey=/k/client.key user=r");
        assert!(hidden);
        // Без кавычек обратная косая libpq экранирует пробел: «b» — часть пароля, а не следующий ключ.
        let (v, _) = redact_setting("primary_conninfo", r"host=db password=a\ b user=r");
        assert_eq!(v, "host=db user=r");
        let (v, _) = redact_setting(
            "primary_conninfo",
            "postgres://rep@db/postgres?sslpassword=k3y&application_name=x",
        );
        assert_eq!(v, "postgres://rep@db/postgres?application_name=x");
        // Строку не разобрать парами — она чистится как команда.
        let (v, hidden) = redact_setting("primary_conninfo", "host=db password=s3cret ???");
        assert!(!v.contains("s3cret") && hidden, "{v}");
    }

    #[test]
    fn commands_lose_uri_passwords_option_values_and_shell_words() {
        let (v, hidden) = redact_setting(
            "archive_command",
            "pg_receivewal -d 'postgresql://rep:s3cret@db/postgres' -D /arch",
        );
        assert_eq!(
            v,
            "pg_receivewal -d 'postgresql://rep@db/postgres' -D /arch"
        );
        assert!(hidden);
        let (v, _) = redact_setting(
            "restore_command",
            "curl -s https://u:pw@backup.example/%f -o %p",
        );
        assert_eq!(v, "curl -s https://u@backup.example/%f -o %p");
        let (v, _) = redact_setting(
            "archive_command",
            "rclone copyto %p remote:wal/%f --s3-secret-access-key abc123 --s3-region eu",
        );
        assert_eq!(
            v,
            "rclone copyto %p remote:wal/%f --s3-secret-access-key *** --s3-region eu"
        );
        let (v, _) = redact_setting(
            "archive_command",
            "pgbackrest --repo1-cipher-pass=xyz --stanza=main archive-push %p",
        );
        assert_eq!(
            v,
            "pgbackrest --repo1-cipher-pass=*** --stanza=main archive-push %p"
        );
        // Слово оболочки целиком: кавычки внутри значения не обрывают его.
        let (v, _) = redact_setting("archive_command", r#"PGPASSWORD=ab"c d"e;cp %p /a"#);
        assert_eq!(v, "PGPASSWORD=***;cp %p /a");
        let (v, _) = redact_setting("archive_command", "pg_receivewal --no-password -D /arch");
        assert_eq!(
            v, "pg_receivewal --no-password -D /arch",
            "опция без значения"
        );
    }

    #[test]
    fn setting_names_decide_what_is_a_secret() {
        for plain in [
            ("password_encryption", "scram-sha-256"),
            ("md5_password_warnings", "on"),
            ("krb_server_keyfile", "FILE:/etc/krb5.keytab"),
            ("max_index_keys", "32"),
            ("ssl_passphrase_command_supports_reload", "off"),
            ("tcp_keepalives_idle", "60"),
            ("log_line_prefix", "%m [%p] user=%u,db=%d,app=%a "),
            ("search_path", "\"$user\", public"),
        ] {
            assert_eq!(
                redact_setting(plain.0, plain.1),
                (plain.1.to_string(), false),
                "{plain:?}"
            );
        }
        for secret in [
            "vectorize.openai_key",
            "ai.openai_api_key",
            "myext.secret",
            "myext.credentials",
        ] {
            assert_eq!(
                redact_setting(secret, "v"),
                ("***".to_string(), true),
                "{secret}"
            );
        }
        // Строка подключения в параметре расширения с другим именем.
        let (v, hidden) = redact_setting("myext.connstr", "host=x password=y dbname=z");
        assert_eq!((v.as_str(), hidden), ("host=x password=*** dbname=z", true));
        let (v, _) = redact_setting("myext.dsn", "postgresql://u:pw@h/db");
        assert_eq!(v, "postgresql://u@h/db");
    }

    // Исправления ревью (раунд 1).

    /// libpq ищет `@` до первого `/`, а не до `?` или `#`: в пароле без кодирования они не обрывают `user:password@`.
    #[test]
    fn uri_password_with_reserved_characters_is_removed() {
        for (raw, clean) in [
            (
                "postgresql://rep:pa?ss@db:5432/postgres",
                "postgresql://rep@db:5432/postgres",
            ),
            (
                "postgresql://rep:pa#ss@db:5432/postgres?sslmode=require",
                "postgresql://rep@db:5432/postgres?sslmode=require",
            ),
            (
                "postgresql://rep:p@ss@db/postgres",
                "postgresql://rep@db/postgres",
            ),
            (
                "postgres://rep:pa?ss@db?sslmode=require&password=x",
                "postgres://rep@db?sslmode=require",
            ),
            (
                "postgresql://rep:pa?s#s@db1:5432,db2:5432/postgres?target_session_attrs=any",
                "postgresql://rep@db1:5432,db2:5432/postgres?target_session_attrs=any",
            ),
        ] {
            assert_eq!(
                redact_setting("primary_conninfo", raw),
                (clean.to_string(), true),
                "{raw}"
            );
        }
        for plain in [
            "postgresql://rep@db:5432/postgres?application_name=a",
            "postgresql://db:5432/postgres?sslmode=require#frag",
            "postgresql://db/postgres",
        ] {
            assert_eq!(
                redact_setting("primary_conninfo", plain),
                (plain.to_string(), false),
                "без пароля значение не меняется"
            );
        }
        let (v, hidden) = redact_setting(
            "archive_command",
            "pg_receivewal -d postgresql://rep:pa?ss@db/postgres -D /arch",
        );
        assert_eq!(
            (v.as_str(), hidden),
            (
                "pg_receivewal -d postgresql://rep@db/postgres -D /arch",
                true
            )
        );
    }

    /// Спец. §6.2 сильнее правила пар: `ssl_passphrase_command` обычно и есть пароль (`echo FooBaR1`), поэтому
    /// непустое значение скрывается целиком; так же — команда с секретным словом в имени.
    #[test]
    fn passphrase_command_is_hidden_whole() {
        for v in [
            "echo FooBaR1",
            "printf %s s3cret",
            "/usr/local/bin/get-ssl-pass.sh %p",
        ] {
            assert_eq!(
                redact_setting("ssl_passphrase_command", v),
                ("***".to_string(), true),
                "{v}"
            );
        }
        assert_eq!(
            redact_setting("ssl_passphrase_command", ""),
            (String::new(), false)
        );
        assert_eq!(
            redact_setting("myext.key_passphrase_command", "echo k"),
            ("***".to_string(), true)
        );
        assert_eq!(
            redact_setting("ssl_passphrase_command_supports_reload", "on"),
            ("on".to_string(), false)
        );
    }

    /// Параметры расширений: пароль (`dbpass`, `pwd`), фраза-пароль и соль скрываются; ручки «про» пароли (длина,
    /// проверка, флаг) — нет. Строка под секретным именем скрывается, даже если похожа на настройку.
    #[test]
    fn extension_secret_names_are_hidden_and_password_knobs_are_kept() {
        for (name, value) in [
            ("myext.dbpass", "s3cret"),
            ("myext.dbpass", "123456"),
            ("myext.db_pwd", "s3cret"),
            ("myext.dbpwd", "s3cret"),
            ("myext.passphrase", "correct horse"),
            ("myext.key_passphrase", "x"),
            ("anon.salt", "a1b2c3"),
            ("anon.salt", "42"),
            ("myext.password_check", "hunter2!"),
            ("credcheck.password_not_contain", "admin,root"),
        ] {
            assert_eq!(
                redact_setting(name, value),
                ("***".to_string(), true),
                "{name} = {value}"
            );
        }
        for (name, value) in [
            ("passwordcheck.min_password_length", "8"),
            ("pgtle.enable_password_check", "require"),
            ("pgtle.enable_password_check", "on"),
            ("pgtle.passcheck_db_name", "postgres"),
            ("credcheck.password_min_length", "12"),
            ("credcheck.password_min_digit", "1"),
            ("credcheck.password_contain_username", "on"),
            ("credcheck.encrypted_password_allowed", "off"),
            ("credcheck.password_ignore_case", "false"),
            ("myext.rls_bypass", "true"),
            ("password_encryption", "scram-sha-256"),
        ] {
            assert_eq!(
                redact_setting(name, value),
                (value.to_string(), false),
                "{name} = {value}"
            );
        }
    }

    /// libpq разрешает пробелы вокруг `=`, а ключи сравниваются без регистра: ни разбор пар, ни запасная чистка
    /// команды не отдают пароль.
    #[test]
    fn spaces_around_equals_and_uppercase_keys_do_not_leak() {
        let (v, hidden) = redact_setting("myext.connstr", "host=db password = s3cret");
        assert_eq!((v.as_str(), hidden), ("host=db password = ***", true));
        let (v, hidden) = redact_setting("primary_conninfo", "host=db password = s3cret ???");
        assert!(!v.contains("s3cret") && hidden, "{v}");
        let (v, hidden) = redact_setting("primary_conninfo", "host=db PASSWORD=s3cret user=r");
        assert_eq!((v.as_str(), hidden), ("host=db user=r", true));
        let (v, hidden) = redact_setting("primary_conninfo", "host=db SslPassword =k3y user=r");
        assert_eq!((v.as_str(), hidden), ("host=db user=r", true));
        let (v, hidden) = redact_setting("myext.connstr", "host=db PASSWORD=s3cret");
        assert_eq!((v.as_str(), hidden), ("host=db PASSWORD=***", true));
        let (v, _) = redact_setting("myext.connstr", "host=db password =\ts3cret port=5432");
        assert_eq!(v, "host=db password =\t*** port=5432");
        let (v, _) = redact_setting("archive_command", "PGPASSWORD = s3cret cmd %p");
        assert_eq!(v, "PGPASSWORD = *** cmd %p");
        let (v, _) = redact_setting("myext.connstr", "user=a password= next=1");
        assert!(!v.contains("next=1") && v.contains("password="), "{v}");
    }
}
