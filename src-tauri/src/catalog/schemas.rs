use serde::Serialize;
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::AppResult;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectCounts {
    /// Обычные, партиционированные и внешние таблицы без партиций.
    pub tables: i64,
    pub views: i64,
    pub matviews: i64,
    pub sequences: i64,
    /// Функции и процедуры.
    pub functions: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaInfo {
    pub name: String,
    pub system: bool,
    pub counts: ObjectCounts,
}

pub async fn list(c: &Client, include_system: bool) -> AppResult<Vec<SchemaInfo>> {
    let rows = c
        .query(
            "SELECT n.nspname::text, n.nspname IN ('pg_catalog', 'information_schema'), \
               count(cl.oid) FILTER (WHERE cl.relkind IN ('r', 'p', 'f') AND NOT cl.relispartition), \
               count(cl.oid) FILTER (WHERE cl.relkind = 'v'), \
               count(cl.oid) FILTER (WHERE cl.relkind = 'm'), \
               count(cl.oid) FILTER (WHERE cl.relkind = 'S'), \
               (SELECT count(*) FROM pg_proc p WHERE p.pronamespace = n.oid AND p.prokind IN ('f', 'p')) \
             FROM pg_namespace n LEFT JOIN pg_class cl ON cl.relnamespace = n.oid \
             WHERE n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp\\_%' \
               AND ($1 OR n.nspname NOT IN ('pg_catalog', 'information_schema')) \
             GROUP BY n.oid, n.nspname ORDER BY 2, 1",
            &[&include_system],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(rows
        .iter()
        .map(|r| SchemaInfo {
            name: r.get(0),
            system: r.get(1),
            counts: ObjectCounts {
                tables: r.get(2),
                views: r.get(3),
                matviews: r.get(4),
                sequences: r.get(5),
                functions: r.get(6),
            },
        })
        .collect())
}
