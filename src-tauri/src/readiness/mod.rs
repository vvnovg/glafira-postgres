//! Сбор фактов для чек-листа готовности (спец. P1 §10); оценка — `pg_advisor::readiness`.

pub mod check;

use pg_advisor::readiness::{ExtensionFact, OPTIONAL_EXTENSIONS, ReadinessFacts};
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::AppResult;

/// Загружен ли `pg_stat_statements` на узле.
pub async fn preload_of(c: &Client) -> AppResult<bool> {
    let v: String = c
        .query_one("SELECT current_setting('shared_preload_libraries')", &[])
        .await
        .map_err(|e| from_pg(&e))?
        .get(0);
    Ok(v.split(',')
        .any(|lib| lib.trim().trim_matches('"') == "pg_stat_statements"))
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
             AND pg_has_role(current_user, b.oid, 'MEMBER')",
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
        let (name, setting): (String, String) = (r.get(0), r.get(1));
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
