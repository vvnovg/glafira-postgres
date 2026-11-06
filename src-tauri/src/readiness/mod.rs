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

/// Имена библиотек из значения `shared_preload_libraries` (пустые элементы отброшены).
pub fn preload_libraries(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(library_name)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// Есть ли `pg_stat_statements` в значении `shared_preload_libraries`.
pub fn preloads_pg_stat_statements(value: &str) -> bool {
    preload_libraries(value)
        .iter()
        .any(|lib| lib == "pg_stat_statements")
}

/// Библиотеки `shared_preload_libraries` узла. `None` — значение не видно роли: `shared_preload_libraries`
/// доступна суперпользователю и членам `pg_read_all_settings` (входит в `pg_monitor`); остальным
/// `current_setting` отвечает 42501, а `pg_settings` строку не отдаёт (проверено на PostgreSQL 15). Это не
/// ошибка проверки: именно такую роль чек-лист и должен разобрать, поэтому читаем через `pg_settings`.
pub async fn preload_of(c: &Client) -> AppResult<Option<Vec<String>>> {
    let rows = c
        .query(
            marked_sql!("SELECT setting FROM pg_settings WHERE name = 'shared_preload_libraries'"),
            &[],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(rows
        .first()
        .and_then(|r| r.get::<_, Option<String>>(0))
        .map(|v| preload_libraries(&v)))
}

pub async fn collect_facts(
    c: &Client,
    preload_libraries_by_member: Vec<(String, Vec<String>)>,
    database: &str,
) -> AppResult<ReadinessFacts> {
    let preload_by_member = preload_libraries_by_member
        .iter()
        .map(|(m, libs)| (m.clone(), libs.iter().any(|l| l == "pg_stat_statements")))
        .collect();
    let pg = |e: tokio_postgres::Error| from_pg(&e);
    let r = c
        .query_one(
            marked_sql!("SELECT current_setting('server_version_num')::int, current_user::text, r.rolsuper, r.rolreplication \
             FROM pg_roles r WHERE r.rolname = current_user"),
            &[],
        )
        .await
        .map_err(pg)?;
    let (server_version_num, user, is_superuser, has_replication): (i32, String, bool, bool) =
        (r.get(0), r.get(1), r.get(2), r.get(3));
    let member_of: Vec<String> = c
        .query(
            marked_sql!("SELECT b.rolname::text FROM pg_roles b \
             WHERE b.rolname IN ('pg_monitor', 'pg_signal_backend', 'pg_maintain', 'pg_read_all_stats', 'pg_read_all_data', \
             'pg_write_all_data', 'pg_read_server_files', 'pg_execute_server_program') \
             AND pg_has_role(current_user, b.oid, 'USAGE')"),
            &[],
        )
        .await
        .map_err(pg)?
        .iter()
        .map(|r| r.get(0))
        .collect();
    // На PostgreSQL 13 роли нет — `false`: EXPLAIN запросов приложения зависит от прав на их таблицы.
    let read_all_data = member_of.iter().any(|r| r == "pg_read_all_data");
    // Права, опасные для `run_select` (P4 §4.5); пункт `llmRole` появится, если `llm_data` выставит проверка.
    let role_dangerous = pg_advisor::readiness::dangerous_rights(is_superuser, &member_of);
    // Версия расширения (не сервера) определяет колонки топа запросов (P3); `None` — расширения нет.
    let pgss_version: Option<String> = c
        .query_opt(
            marked_sql!(
                "SELECT extversion::text FROM pg_extension WHERE extname = 'pg_stat_statements'"
            ),
            &[],
        )
        .await
        .map_err(pg)?
        .map(|r| r.get(0));
    let pgss_installed = pgss_version.is_some();
    let mut compute_query_id = None;
    let mut pgss_track_planning = None;
    let mut track_io_timing = String::from("off");
    let mut track_activity_query_size = 1024;
    for r in c
        .query(
            marked_sql!(
                "SELECT name::text, setting FROM pg_settings \
             WHERE name IN ('compute_query_id', 'track_io_timing', 'track_activity_query_size', \
             'pg_stat_statements.track_planning')"
            ),
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
            "pg_stat_statements.track_planning" => pgss_track_planning = Some(setting),
            // В pg_settings — байты.
            _ => track_activity_query_size = setting.parse().unwrap_or(1024),
        }
    }
    let names: Vec<&str> = OPTIONAL_EXTENSIONS.to_vec();
    let extensions = c
        .query(
            marked_sql!(
                "SELECT a.name::text, e.extversion::text FROM pg_available_extensions a \
             LEFT JOIN pg_extension e ON e.extname = a.name WHERE a.name = ANY($1)"
            ),
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
        has_replication,
        member_of,
        preload_by_member,
        preload_libraries_by_member,
        pgss_installed,
        pgss_version,
        pgss_track_planning,
        read_all_data,
        role_dangerous,
        llm_data: false,
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

    #[test]
    fn splits_preload_list_into_library_names() {
        assert_eq!(
            preload_libraries(
                "pg_stat_statements, \"$libdir/pg_stat_kcache.so\" ,pg_wait_sampling,"
            ),
            ["pg_stat_statements", "pg_stat_kcache", "pg_wait_sampling"]
        );
        assert!(preload_libraries("").is_empty());
    }
}
