use serde::Serialize;
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::AppResult;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInfo {
    pub name: String,
    /// `None` — нет права `CONNECT` (и роли `pg_read_all_stats`).
    pub size_bytes: Option<i64>,
}

/// Базы с `datallowconn`; при фильтре профиля — только перечисленные, в его порядке.
pub async fn list(c: &Client, filter: Option<&[String]>) -> AppResult<Vec<DatabaseInfo>> {
    let rows = c
        .query(
            "SELECT datname::text, \
               CASE WHEN has_database_privilege(oid, 'CONNECT') OR pg_has_role('pg_read_all_stats', 'MEMBER') \
                    THEN pg_database_size(oid) END \
             FROM pg_database WHERE NOT datistemplate AND datallowconn ORDER BY datname",
            &[],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    let all: Vec<DatabaseInfo> = rows
        .iter()
        .map(|r| DatabaseInfo {
            name: r.get(0),
            size_bytes: r.get(1),
        })
        .collect();
    Ok(match filter {
        None => all,
        Some(names) => names
            .iter()
            .filter_map(|n| all.iter().find(|d| &d.name == n).cloned())
            .collect(),
    })
}
