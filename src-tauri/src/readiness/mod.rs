//! Сбор фактов для чек-листа готовности (спец. P1 §10); оценка — `pg_advisor::readiness`.

pub mod check;

use pg_advisor::readiness::{ExtensionFact, OPTIONAL_EXTENSIONS, ReadinessFacts};
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::AppResult;

/// Библиотека в элементе `shared_preload_libraries`: без кавычек и пробелов, без каталога
/// (`$libdir/…`, абсолютный путь) и без расширения файла (`.so`, `.dll`, `.dylib`).
fn library_name(item: &str) -> &str {
    let name = item.trim().trim_matches('"').trim();
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    [".so", ".dll", ".dylib"]
        .iter()
        .find_map(|ext| name.strip_suffix(ext))
        .unwrap_or(name)
}

/// Есть ли `pg_stat_statements` в значении `shared_preload_libraries`.
pub fn preloads_pg_stat_statements(value: &str) -> bool {
    value
        .split(',')
        .any(|lib| library_name(lib) == "pg_stat_statements")
}

/// Загружен ли `pg_stat_statements` на узле. `None` — значение не видно роли: `shared_preload_libraries`
/// доступна суперпользователю и членам `pg_read_all_settings` (входит в `pg_monitor`); остальным
/// `current_setting` отвечает 42501, а `pg_settings` строку не отдаёт (проверено на PostgreSQL 15). Это не
/// ошибка проверки: именно такую роль чек-лист и должен разобрать, поэтому читаем через `pg_settings`.
pub async fn preload_of(c: &Client) -> AppResult<Option<bool>> {
    let rows = c
        .query(
            "SELECT setting FROM pg_settings WHERE name = 'shared_preload_libraries'",
            &[],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(rows
        .first()
        .and_then(|r| r.get::<_, Option<String>>(0))
        .map(|v| preloads_pg_stat_statements(&v)))
}

pub async fn collect_facts(
    c: &Client,
    preload_by_member: Vec<(String, bool)>,
    database: &str,
) -> AppResult<ReadinessFacts> {
    let pg = |e: tokio_postgres::Error| from_pg(&e);
    let r = c
        .query_one(
            "SELECT current_setting('server_version_num')::int, current_user::text, r.rolsuper \
             FROM pg_roles r WHERE r.rolname = current_user",
            &[],
        )
        .await
        .map_err(pg)?;
    let (server_version_num, user, is_superuser): (i32, String, bool) =
        (r.get(0), r.get(1), r.get(2));
    let member_of = c
        .query(
            "SELECT b.rolname::text FROM pg_roles b \
             WHERE b.rolname IN ('pg_monitor', 'pg_signal_backend', 'pg_maintain', 'pg_read_all_stats') \
             AND pg_has_role(current_user, b.oid, 'USAGE')",
            &[],
        )
        .await
        .map_err(pg)?
        .iter()
        .map(|r| r.get(0))
        .collect();
    let pgss_installed: bool = c
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_stat_statements')",
            &[],
        )
        .await
        .map_err(pg)?
        .get(0);
    let mut compute_query_id = None;
    let mut track_io_timing = String::from("off");
    let mut track_activity_query_size = 1024;
    for r in c
        .query(
            "SELECT name::text, setting FROM pg_settings \
             WHERE name IN ('compute_query_id', 'track_io_timing', 'track_activity_query_size')",
            &[],
        )
        .await
        .map_err(pg)?
    {
        // Скрытая от роли настройка приходит без значения: остаются значения по умолчанию.
        let (name, Some(setting)): (String, Option<String>) = (r.get(0), r.get(1)) else {
            continue;
        };
        match name.as_str() {
            "compute_query_id" => compute_query_id = Some(setting),
            "track_io_timing" => track_io_timing = setting,
            // В pg_settings — байты.
            _ => track_activity_query_size = setting.parse().unwrap_or(1024),
        }
    }
    let names: Vec<&str> = OPTIONAL_EXTENSIONS.to_vec();
    let extensions = c
        .query(
            "SELECT a.name::text, e.extversion::text FROM pg_available_extensions a \
             LEFT JOIN pg_extension e ON e.extname = a.name WHERE a.name = ANY($1)",
            &[&names],
        )
        .await
        .map_err(pg)?
        .iter()
        .map(|r| ExtensionFact {
            name: r.get(0),
            installed: r.get(1),
            available: true,
        })
        .collect();
    Ok(ReadinessFacts {
        server_version_num,
        user,
        database: database.to_string(),
        is_superuser,
        member_of,
        preload_by_member,
        pgss_installed,
        compute_query_id,
        track_io_timing,
        track_activity_query_size,
        extensions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_pg_stat_statements_in_preload_list() {
        for v in [
            "pg_stat_statements",
            "pg_cron, pg_stat_statements",
            "pg_stat_statements,pg_cron",
            "\"pg_stat_statements\"",
            " pg_stat_statements ",
            "$libdir/pg_stat_statements",
            "\"$libdir/pg_stat_statements\"",
            "pg_stat_statements.so",
            "$libdir/pg_stat_statements.so",
            "/usr/lib/postgresql/17/lib/pg_stat_statements.so",
            "C:\\pg\\lib\\pg_stat_statements.dll",
            "pg_stat_statements.dylib",
            "timescaledb, \"$libdir/pg_stat_statements.so\", auto_explain",
        ] {
            assert!(preloads_pg_stat_statements(v), "{v}");
        }
        for v in [
            "",
            "pg_cron",
            "pg_stat_statements_ext",
            "my_pg_stat_statements",
            "pg_stat_statements.so.1",
            "pg_stat_kcache",
            "auto_explain, pg_stat_kcache",
        ] {
            assert!(!preloads_pg_stat_statements(v), "{v}");
        }
    }
}
